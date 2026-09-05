//! Editing a file in an external editor. cian does not embed one — a usable
//! vim/nvim is tens of megabytes with its runtime, which would blow the
//! single-binary size for little gain — so `E` in the viewer (and `:edit`)
//! shells out to the user's editor: `cian.set_option("editor", …)` if set, then
//! `$VISUAL`/`$EDITOR`, then the first of nvim → vim → vi found on `PATH`.
//!
//! The main loop does the actual suspend/run/restore around the terminal (see
//! [`crate::run_loop`]); here we resolve *which* editor and expose the trigger.

use std::path::{Path, PathBuf};

use crate::{tr, App, Popup};

/// A queued edit: the file the editor opens, and what to do when it returns.
pub(crate) struct PendingEdit {
    pub path: PathBuf,
    pub kind: EditKind,
}

/// What the queued edit is for, i.e. what happens after the editor exits.
pub(crate) enum EditKind {
    /// A real file: refresh the panes and, if the edit came from the viewer,
    /// re-open it there under `title`.
    File { title: String, reopen_viewer: bool },
    /// A bulk rename (vidir-style): `path` is a temp file holding one name per
    /// line. On return the lines are matched 1:1 against `names` and each
    /// changed line renames that file inside `dir`.
    BulkRename { dir: PathBuf, names: Vec<String> },
}

impl App {
    /// `E` in the viewer: edit the file being viewed, then return to the viewer.
    pub(crate) fn edit_viewed_file(&mut self) {
        if let Popup::Viewer { path, title, .. } = &self.popup {
            self.pending_edit = Some(PendingEdit {
                path: path.clone(),
                kind: EditKind::File { title: title.clone(), reopen_viewer: true },
            });
        }
    }

    /// `:edit` / `:e`: edit the file under the cursor in the active pane.
    pub(crate) fn edit_selected_file(&mut self) {
        let Some(p) = self.active_pane() else { return };
        match p.selected().filter(|e| !e.is_parent) {
            Some(e) if !e.is_dir => {
                self.pending_edit = Some(PendingEdit {
                    path: e.path.clone(),
                    kind: EditKind::File { title: e.name.clone(), reopen_viewer: false },
                });
            }
            Some(_) => self.message = Some(tr(self.lang, "that is a directory", "ディレクトリです").into()),
            None => self.message = Some(tr(self.lang, "nothing to edit", "編集対象がありません").into()),
        }
    }

    /// `:renamelist`: rename by editing a list of names.
    ///
    /// The marked entries — or, with nothing marked, everything in the pane —
    /// are written one name per line to a temp file and opened in the editor
    /// (vidir's interface, familiar from ranger and yazi). On save-and-quit,
    /// each changed line renames that file; a swap (a↔b) works because the
    /// renames go through unique temporary names first.
    pub(crate) fn start_editor_rename(&mut self) {
        if self.active_pane().map(|p| p.is_remote()).unwrap_or(false) {
            self.message = Some(tr(
                self.lang,
                "bulk rename works on local panes only",
                "一括リネームはローカルペインのみ対応",
            ).into());
            return;
        }
        let Some(p) = self.active_pane() else { return };
        let dir = p.cwd.clone();
        // The pane's visible order, so the buffer reads like the pane. Marks
        // narrow it; otherwise the whole listing is up for renaming, like vidir.
        let names: Vec<String> = if p.mark_count() > 0 {
            p.entries
                .iter()
                .filter(|e| !e.is_parent && p.marks.contains(&e.path))
                .map(|e| e.name.clone())
                .collect()
        } else {
            p.entries.iter().filter(|e| !e.is_parent).map(|e| e.name.clone()).collect()
        };
        if names.is_empty() {
            self.message = Some(tr(self.lang, "nothing to rename", "リネーム対象がありません").into());
            return;
        }
        // pid alone is not unique enough: two panes (or two tests) in one
        // process must not share a list file, so a counter joins it.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let list = std::env::temp_dir()
            .join(format!("cian-rename-{}-{}.txt", std::process::id(), seq));
        let body = names.join("\n") + "\n";
        if std::fs::write(&list, body).is_err() {
            self.message = Some(tr(self.lang, "cannot write temp file", "一時ファイルを作れません").into());
            return;
        }
        self.pending_edit = Some(PendingEdit {
            path: list,
            kind: EditKind::BulkRename { dir, names },
        });
    }

    /// Apply an edited rename list: validate, then rename the changed entries.
    /// Runs after the editor exits zero; a non-zero exit (`:cq`) already
    /// cancelled. Any validation problem cancels the whole batch — a rename is
    /// cheap to redo and half-applied batches are miserable to untangle.
    pub(crate) fn finish_editor_rename(&mut self, list: &Path, dir: &Path, old: &[String]) {
        let edited = match std::fs::read_to_string(list) {
            Ok(s) => s,
            Err(e) => {
                self.message = Some(format!("bulk rename: {e}"));
                return;
            }
        };
        let _ = std::fs::remove_file(list);
        let new: Vec<String> =
            edited.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
        let pairs = match plan_bulk_rename(old, &new) {
            Ok(p) => p,
            Err(e) => {
                self.message = Some(tr_plan_error(self.lang, &e));
                return;
            }
        };
        if pairs.is_empty() {
            self.message = Some(tr(self.lang, "no names changed", "変更はありません").into());
            return;
        }
        // A target may exist on disk only if it is itself being renamed away
        // (a swap); anything else would clobber a bystander.
        let moving_away: std::collections::HashSet<&str> =
            pairs.iter().map(|(from, _)| from.as_str()).collect();
        for (_, to) in &pairs {
            if dir.join(to).symlink_metadata().is_ok() && !moving_away.contains(to.as_str()) {
                self.message = Some(format!(
                    "{}: {}",
                    tr(self.lang, "bulk rename cancelled. a name already exists", "一括リネームを中止しました。既に存在する名前があります"),
                    to
                ));
                return;
            }
        }
        // Two phases via unique temp names, so a↔b swaps cannot collide.
        let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (n, (from, _)) in pairs.iter().enumerate() {
            let tmp = dir.join(format!(".cian-rename-{}-{}", std::process::id(), n));
            if let Err(e) = std::fs::rename(dir.join(from), &tmp) {
                // Roll back what was staged so nothing is left half-moved.
                for (orig, t) in &staged {
                    let _ = std::fs::rename(t, orig);
                }
                self.message = Some(format!("bulk rename: {from}: {e}"));
                self.reload_both();
                return;
            }
            staged.push((dir.join(from), tmp));
        }
        let mut done = 0usize;
        for ((_, tmp), (_, to)) in staged.iter().zip(&pairs) {
            match std::fs::rename(tmp, dir.join(to)) {
                Ok(()) => done += 1,
                // Landing failed (permissions, name invalid on this FS): put
                // the original name back rather than leaving a temp file.
                Err(e) => {
                    let (orig, _) = &staged[done];
                    let _ = std::fs::rename(tmp, orig);
                    self.message = Some(format!("bulk rename: {to}: {e}"));
                }
            }
        }
        if done == pairs.len() {
            self.message = Some(if self.lang == crate::Lang::Ja {
                format!("{} 件リネームしました", done)
            } else {
                format!("renamed {} file(s)", done)
            });
        }
        if let Some(p) = self.active_pane_mut() {
            p.clear_marks();
        }
        self.reload_both();
    }

    /// Open the file under the cursor in a fresh shell tab, so cian keeps
    /// running. `forced` names a specific editor (`:vi` / `:vim` / `:nvim`),
    /// which must be on PATH; `None` (the menu) resolves the configured editor,
    /// else nvim → vim → vi. Explains and does nothing if the editor is missing.
    pub(crate) fn edit_in_new_tab(&mut self, forced: Option<&str>) {
        let path = match self.active_pane().and_then(|p| p.selected()) {
            Some(e) if e.is_parent => None,
            Some(e) if e.is_dir => {
                self.message = Some(tr(self.lang, "that is a directory", "ディレクトリです").into());
                return;
            }
            Some(e) => Some(e.path.clone()),
            None => None,
        };
        let Some(path) = path else {
            self.message = Some(tr(self.lang, "nothing to edit", "編集対象がありません").into());
            return;
        };
        let words = match forced {
            // A named editor: use it only if it is actually on PATH.
            Some(name) if cian_core::editor::on_path(name) => vec![name.to_string()],
            Some(name) => {
                self.message = Some(format!("{name} not found on PATH"));
                return;
            }
            None => match crate::edit::resolve_editor(&self.config) {
                Some(w) => w,
                None => {
                    self.message = Some(tr(
                        self.lang,
                        "no editor found — install nvim/vim/vi, or set cian.set_option(\"editor\", …)",
                        "エディタが見つかりません — nvim/vim/vi を入れるか cian.set_option(\"editor\", …) を設定",
                    ).into());
                    return;
                }
            },
        };
        // Quote the path so a name with spaces survives the shell.
        let cmd = format!("{} \"{}\"", words.join(" "), path.display());
        let cwd = self.shell_cwd();
        self.shell.new_tab_running(&cwd, cmd);
        self.focus(crate::FocusedPane::Shell);
        self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("{} を新しいタブで編集します", path.display())
        } else {
            format!("editing {} in a new tab", path.display())
        });
    }
}

/// Why an edited rename list cannot be applied. Carried as data so the
/// user-facing message can be localized at the edge.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PlanError {
    /// Line count changed — the 1:1 mapping to files is gone.
    CountMismatch { was: usize, now: usize },
    /// A line is empty or still `.`/`..`.
    BadName(String),
    /// A name contains a path separator; this is rename, not move.
    HasSeparator(String),
    /// Two lines want the same final name.
    Duplicate(String),
}

/// Match the edited lines 1:1 against the original names and return the pairs
/// that actually changed, or the reason the batch must be rejected. Pure, so
/// the whole contract is testable without a filesystem.
pub(crate) fn plan_bulk_rename(
    old: &[String],
    new: &[String],
) -> Result<Vec<(String, String)>, PlanError> {
    if old.len() != new.len() {
        return Err(PlanError::CountMismatch { was: old.len(), now: new.len() });
    }
    let mut seen = std::collections::HashSet::new();
    for n in new {
        if n.is_empty() || n == "." || n == ".." {
            return Err(PlanError::BadName(n.clone()));
        }
        if n.contains('/') || n.contains('\\') {
            return Err(PlanError::HasSeparator(n.clone()));
        }
        if !seen.insert(n.as_str()) {
            return Err(PlanError::Duplicate(n.clone()));
        }
    }
    Ok(old
        .iter()
        .zip(new)
        .filter(|(o, n)| o != n)
        .map(|(o, n)| (o.clone(), n.clone()))
        .collect())
}

/// The user-facing message for a rejected rename list.
pub(crate) fn tr_plan_error(lang: crate::Lang, e: &PlanError) -> String {
    let ja = lang == crate::Lang::Ja;
    match e {
        PlanError::CountMismatch { was, now } => {
            if ja {
                format!("一括リネーム中止 — 行数が変わっています（{was} → {now}）。行の追加・削除はできません")
            } else {
                format!("bulk rename cancelled — line count changed ({was} → {now}); add or remove no lines")
            }
        }
        PlanError::BadName(n) => {
            if ja {
                format!("一括リネーム中止 — 無効な名前: {n:?}")
            } else {
                format!("bulk rename cancelled — invalid name: {n:?}")
            }
        }
        PlanError::HasSeparator(n) => {
            if ja {
                format!("一括リネーム中止 — パス区切りは使えません: {n}")
            } else {
                format!("bulk rename cancelled — path separators not allowed: {n}")
            }
        }
        PlanError::Duplicate(n) => {
            if ja {
                format!("一括リネーム中止 — 名前が重複しています: {n}")
            } else {
                format!("bulk rename cancelled — duplicate name: {n}")
            }
        }
    }
}

/// Resolve the editor command (without the file argument) for `config`,
/// honouring `$VISUAL`/`$EDITOR` and finally the nvim → vim → vi fallback.
pub(crate) fn resolve_editor(config: &cian_lua::Config) -> Option<Vec<String>> {
    // **判断は `cian_core::editor`。** ここに持っていたときは、窓版のエンジンが
    // `$VISUAL`/`$EDITOR` しか見ておらず、`cian.set_option("editor", …)` が
    // 端末版でだけ効いていた（2026-09-06、`configcover.py` が見つけた）。
    cian_core::editor::resolve(config.options.editor.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    // エディタの選び方のテストは `cian-core/src/editor.rs` へ ── 判断が
    // あちらへ移ったので、検査もあちらに置く（両前端が同じものを見る）。

    fn v(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// The whole rename-list contract: unchanged lines are skipped, changed
    /// ones pair up, and anything that breaks the 1:1 mapping rejects the
    /// batch — renaming again is cheap, untangling half a batch is not.
    #[test]
    fn rename_plan_pairs_changes_and_rejects_broken_lists() {
        let old = v(&["a.txt", "b.txt", "c.txt"]);
        assert_eq!(
            plan_bulk_rename(&old, &v(&["a.txt", "b2.txt", "c.txt"])),
            Ok(vec![("b.txt".into(), "b2.txt".into())]),
        );
        // A swap is a valid plan; ordering is the executor's problem.
        assert_eq!(
            plan_bulk_rename(&old, &v(&["b.txt", "a.txt", "c.txt"])),
            Ok(vec![("a.txt".into(), "b.txt".into()), ("b.txt".into(), "a.txt".into())]),
        );
        assert_eq!(plan_bulk_rename(&old, &old), Ok(vec![]), "nothing changed");

        assert_eq!(
            plan_bulk_rename(&old, &v(&["a.txt", "b.txt"])),
            Err(PlanError::CountMismatch { was: 3, now: 2 }),
            "a deleted line is not a delete command"
        );
        assert_eq!(
            plan_bulk_rename(&old, &v(&["a.txt", "", "c.txt"])),
            Err(PlanError::BadName(String::new())),
        );
        assert_eq!(
            plan_bulk_rename(&old, &v(&["a.txt", "sub/b.txt", "c.txt"])),
            Err(PlanError::HasSeparator("sub/b.txt".into())),
            "rename, not move"
        );
        assert_eq!(
            plan_bulk_rename(&old, &v(&["same", "same", "c.txt"])),
            Err(PlanError::Duplicate("same".into())),
        );
    }
}
