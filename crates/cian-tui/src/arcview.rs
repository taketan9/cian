//! Browsing an archive as a folder (`Enter` on a zip/tar): the pane switches
//! to a synthetic listing of the members under a directory inside the
//! archive. Navigation mirrors a real directory — `Enter`/`l` descends, the
//! `..` row (or `h`) climbs, and climbing past the root lands back on the
//! archive file itself. `F3` extracts the member to a temp file and opens the
//! normal viewer on it; copying to the other pane extracts.
//!
//! Member rows are [`cian_core::Entry`]s with a *synthetic* path — the
//! archive's path joined with the member path, which does not exist on disk.
//! That is the same trick the SFTP pane uses, and like there, operations that
//! would touch the path are either mapped to archive equivalents here or
//! refused with a message. This phase is read-only; zip writing is next.

use std::path::{Path, PathBuf};

use cian_core::archive::Member;

use crate::{theme::Lang, tr, App};

// The two pure ones live in cian-core: the GUI's engine browses the same zips,
// and two front ends disagreeing about what is inside one would be strange.
use cian_core::archive::{archive_rows, members_under};

/// Members are listed once per archive and kept while browsing inside it —
/// tar.gz listing decompresses the whole stream, which must not happen per
/// keystroke. Invalidated by mtime, so an archive rebuilt underneath is
/// noticed on the next navigation.
pub(crate) struct ArchiveCache {
    pub path: PathBuf,
    mtime: Option<std::time::SystemTime>,
    pub members: Vec<Member>,
}

impl App {
    /// The cached member list for `archive`, (re)read when missing or stale.
    fn archive_members(&mut self, archive: &Path) -> Result<Vec<Member>, String> {
        let mtime = std::fs::metadata(archive).ok().and_then(|m| m.modified().ok());
        if let Some(c) = &self.archive_cache {
            if c.path == archive && c.mtime == mtime {
                return Ok(c.members.clone());
            }
        }
        let members = cian_core::archive::list(archive).map_err(|e| e.to_string())?;
        self.archive_cache =
            Some(ArchiveCache { path: archive.to_path_buf(), mtime, members: members.clone() });
        Ok(members)
    }

    /// `Enter` on an archive file: browse into it (at `sub`, `""` = root).
    pub(crate) fn enter_archive(&mut self, archive: PathBuf, sub: String) {
        let members = match self.archive_members(&archive) {
            Ok(m) => m,
            Err(e) => {
                self.message = Some(if self.lang == Lang::Ja {
                    format!("書庫として読めません: {e}")
                } else {
                    format!("not a readable archive: {e}")
                });
                return;
            }
        };
        let rows = archive_rows(&archive, &members, &sub);
        if let Some(p) = self.active_pane_mut() {
            p.enter_archive(archive, sub, rows);
        }
    }

    /// Climb one level inside the archive; past the root, leave it and put
    /// the cursor back on the archive file.
    pub(crate) fn archive_go_up(&mut self) {
        let Some((archive, sub)) =
            self.active_pane().and_then(|p| p.archive_view()).map(|(a, s)| (a.to_path_buf(), s.to_string()))
        else {
            return;
        };
        if sub.is_empty() {
            // Leave: back to the real directory, cursor on the archive.
            let dir = archive.parent().map(|p| p.to_path_buf());
            let name = archive.file_name().map(|s| s.to_string_lossy().into_owned());
            if let (Some(dir), Some(p)) = (dir, self.active_pane_mut()) {
                let _ = p.jump_to(dir);
                if let Some(name) = name {
                    if let Some(i) = p.entries.iter().position(|e| e.name == name) {
                        p.cursor = i;
                    }
                }
            }
            return;
        }
        // "a/b/" → "a/"; "a/" → "".
        let parent = sub
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|(head, _)| format!("{head}/"))
            .unwrap_or_default();
        let child = sub.trim_end_matches('/').rsplit('/').next().map(|s| s.to_string());
        self.enter_archive(archive, parent);
        // Land on the directory we just left, as real navigation does.
        if let (Some(child), Some(p)) = (child, self.active_pane_mut()) {
            if let Some(i) = p.entries.iter().position(|e| e.name == child) {
                p.cursor = i;
            }
        }
    }

    /// `Enter` on a row inside the archive: descend into a directory, or view
    /// a file member (same as F3 — there is no OS association for something
    /// that exists only inside an archive).
    pub(crate) fn archive_activate(&mut self) {
        let Some((archive, sub)) =
            self.active_pane().and_then(|p| p.archive_view()).map(|(a, s)| (a.to_path_buf(), s.to_string()))
        else {
            return;
        };
        let Some(e) = self.active_pane().and_then(|p| p.selected()).cloned() else { return };
        if e.is_parent {
            self.archive_go_up();
            return;
        }
        if e.is_dir {
            self.enter_archive(archive, format!("{}{}/", sub, e.name));
        } else {
            self.archive_view_member(&archive, &format!("{}{}", sub, e.name));
        }
    }

    /// F3 on a member: extract it to a temp file and open the normal viewer —
    /// text, image, office, whatever it turns out to be.
    pub(crate) fn archive_view_member(&mut self, archive: &Path, member: &str) {
        const MAX_VIEW: u64 = 64 * 1024 * 1024;
        let size = self
            .archive_cache
            .as_ref()
            .and_then(|c| c.members.iter().find(|m| m.name.trim_end_matches('/') == member))
            .map(|m| m.size)
            .unwrap_or(0);
        if size > MAX_VIEW {
            self.message = Some(tr(
                self.lang,
                "member too large to view. copy it out instead",
                "大きすぎて閲覧できません。展開してから開いてください",
            ).into());
            return;
        }
        if cian_core::archive::zip_needs_password(archive) {
            self.message = Some(tr(
                self.lang,
                "encrypted zip. use :unzip to extract with the password",
                "暗号化zip。:unzip でパスワード指定で展開してください",
            ).into());
            return;
        }
        // A per-process temp dir; one subdir per view keeps names collision-free.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dest =
            std::env::temp_dir().join(format!("cian-arcview-{}-{}", std::process::id(), seq));
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut sink = |_: &cian_core::progress::Progress| {};
        let mut ctl = cian_core::progress::Ctl { cancel: &cancel, on_progress: &mut sink };
        // Strip the member's own directory so the temp file is dest/<basename>.
        let strip = member.rsplit_once('/').map(|(h, _)| format!("{h}/")).unwrap_or_default();
        let report = cian_core::archive::extract(
            archive,
            &[member.to_string()],
            &dest,
            None,
            &strip,
            &mut ctl,
        );
        if let Some(err) = report.errors.first() {
            self.message = Some(if self.lang == Lang::Ja {
                format!("書庫の中のファイルを閲覧できません: {err}")
            } else {
                format!("view member: {err}")
            });
            return;
        }
        let base = member.rsplit('/').next().unwrap_or(member);
        let path = dest.join(base);
        let title = format!(
            "{}:{}",
            archive.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
            member
        );
        // Remember where this came from, so saving puts it back rather than
        // leaving the edit in a temp file.
        self.arc_edits
            .insert(path.clone(), (archive.to_path_buf(), member.to_string()));
        self.open_viewer_at(&path, &title, 0);
    }

    /// Put an edited member back into its archive.
    ///
    /// The member is dropped and the edited file added under the same name —
    /// which is what `zip_modify` already does safely, writing a whole new
    /// archive beside the old one and swapping it in only on success. An edit
    /// that cannot be written back has to say so loudly: the temp file still
    /// holds the work, and the archive is what the user thinks they saved.
    pub(crate) fn write_back_to_archive(&mut self, temp: &Path) -> Option<String> {
        let (archive, member) = self.arc_edits.get(temp)?.clone();
        let prefix = member.rsplit_once('/').map(|(h, _)| format!("{h}/")).unwrap_or_default();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut sink = |_: &cian_core::progress::Progress| {};
        let mut ctl = cian_core::progress::Ctl { cancel: &cancel, on_progress: &mut sink };
        let report = cian_core::archive::zip_modify(
            &archive,
            std::slice::from_ref(&member),
            &[],
            std::slice::from_ref(&temp.to_path_buf()),
            &prefix,
            &mut ctl,
        );
        match report.errors.first() {
            Some(e) => Some(format!(
                "{}: not written back — {e} (the edit is still in {})",
                archive.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
                temp.display(),
            )),
            None => {
                self.archive_cache = None; // the listing has a new size and time
                None
            }
        }
    }

    /// Copy the marked members (or the cursor's) out to `dest`, extracting
    /// relative to the directory being browsed. Runs on the op worker with
    /// progress, like any transfer.
    pub(crate) fn archive_copy_out(&mut self, dest: PathBuf) {
        let Some((archive, sub)) =
            self.active_pane().and_then(|p| p.archive_view()).map(|(a, s)| (a.to_path_buf(), s.to_string()))
        else {
            return;
        };
        let Some(pane) = self.active_pane() else { return };
        // Selected rows → member prefixes (dirs expand to everything under).
        let arc_prefix = format!("{}/", archive.display());
        let picked: Vec<(String, bool)> = if pane.mark_count() > 0 {
            pane.entries
                .iter()
                .filter(|e| !e.is_parent && pane.marks.contains(&e.path))
                .map(|e| (format!("{}{}", sub, e.name), e.is_dir))
                .collect()
        } else {
            match pane.selected().filter(|e| !e.is_parent) {
                Some(e) => vec![(format!("{}{}", sub, e.name), e.is_dir)],
                None => Vec::new(),
            }
        };
        let _ = arc_prefix;
        if picked.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択がありません").into());
            return;
        }
        let members: Vec<String> = match self.archive_members(&archive) {
            Ok(all) => picked
                .iter()
                .flat_map(|(prefix, is_dir)| {
                    if *is_dir {
                        members_under(&all, prefix)
                            .into_iter()
                            .map(|m| m.name.clone())
                            .collect::<Vec<_>>()
                    } else {
                        vec![prefix.clone()]
                    }
                })
                .collect(),
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        // Extract relative to the browsed directory, so copying `c/` from
        // inside `a/b/` produces dest/c/…, not dest/a/b/c/….
        self.start_extract_stripped(archive, members, dest, sub);
    }
}


impl App {
    /// Queue the zip mutation on the op worker; the archive panes refresh
    /// when it lands (see [`App::refresh_archive_panes`]).
    pub(crate) fn run_zip_modify(
        &mut self,
        archive: PathBuf,
        drop_members: Vec<String>,
        rename_members: Vec<(String, String)>,
        add_sources: Vec<PathBuf>,
        add_prefix: String,
    ) {
        self.start_op("zip", move |ctl| {
            cian_core::archive::zip_modify(
                &archive,
                &drop_members,
                &rename_members,
                &add_sources,
                &add_prefix,
                ctl,
            )
        });
    }

    /// The other pane wants these files inside the zip this pane browses:
    /// show what is about to happen, then do it on `y`.
    pub(crate) fn confirm_zip_add(&mut self) -> anyhow::Result<()> {
        if let crate::Popup::ConfirmZipAdd { archive, sub, sources } = &self.popup {
            let (archive, sub, sources) = (archive.clone(), sub.clone(), sources.clone());
            self.popup = crate::Popup::None;
            self.run_zip_modify(archive, Vec::new(), Vec::new(), sources, sub);
        }
        Ok(())
    }

    pub(crate) fn confirm_zip_delete(&mut self) -> anyhow::Result<()> {
        if let crate::Popup::ConfirmZipDelete { archive, members, .. } = &self.popup {
            let (archive, members) = (archive.clone(), members.clone());
            self.popup = crate::Popup::None;
            self.run_zip_modify(archive, members, Vec::new(), Vec::new(), String::new());
        }
        Ok(())
    }

    /// Delete inside the archive: expand the picked rows to their members and
    /// confirm. Deletion rewrites the zip, so it always asks.
    pub(crate) fn archive_delete(&mut self) {
        let Some((archive, sub)) = self
            .active_pane()
            .and_then(|p| p.archive_view())
            .map(|(a, s)| (a.to_path_buf(), s.to_string()))
        else {
            return;
        };
        if !self.require_zip_writable(&archive) {
            return;
        }
        let Some(pane) = self.active_pane() else { return };
        let picked: Vec<(String, bool)> = if pane.mark_count() > 0 {
            pane.entries
                .iter()
                .filter(|e| !e.is_parent && pane.marks.contains(&e.path))
                .map(|e| (format!("{}{}", sub, e.name), e.is_dir))
                .collect()
        } else {
            match pane.selected().filter(|e| !e.is_parent) {
                Some(e) => vec![(format!("{}{}", sub, e.name), e.is_dir)],
                None => Vec::new(),
            }
        };
        if picked.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択がありません").into());
            return;
        }
        let shown: Vec<String> = picked.iter().map(|(n, _)| n.clone()).collect();
        let members: Vec<String> = match self.archive_members(&archive) {
            Ok(all) => picked
                .iter()
                .flat_map(|(prefix, is_dir)| {
                    if *is_dir {
                        members_under(&all, prefix).into_iter().map(|m| m.name.clone()).collect()
                    } else {
                        vec![prefix.clone()]
                    }
                })
                .collect(),
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        self.popup = crate::Popup::ConfirmZipDelete { archive, members, shown };
    }

    /// Modifying only works for zip-family archives; tar would need a full
    /// re-pack and stays read-only. Encrypted zips are refused by the core.
    pub(crate) fn require_zip_writable(&mut self, archive: &Path) -> bool {
        let name = archive.to_string_lossy().to_lowercase();
        let zip_like = [".zip", ".jar", ".xpi", ".whl", ".epub"].iter().any(|e| name.ends_with(e));
        if !zip_like {
            self.message = Some(tr(
                self.lang,
                "only zip archives can be modified (tar is read-only)",
                "変更できるのは zip のみです（tar は読み取り専用）",
            ).into());
        }
        zip_like
    }

    /// F2 inside the archive: rename the member (or directory) under the
    /// cursor. A directory rename rewrites every member beneath it.
    pub(crate) fn archive_rename_start(&mut self) {
        let Some((archive, sub)) = self
            .active_pane()
            .and_then(|p| p.archive_view())
            .map(|(a, s)| (a.to_path_buf(), s.to_string()))
        else {
            return;
        };
        if !self.require_zip_writable(&archive) {
            return;
        }
        let Some(e) = self.active_pane().and_then(|p| p.selected()).cloned() else { return };
        if e.is_parent {
            return;
        }
        self.popup = crate::text_input(
            "rename in zip",
            "new name:",
            e.name.clone(),
            crate::InputKind::RenameZipMember {
                archive,
                sub,
                from: e.name.clone(),
                is_dir: e.is_dir,
            },
        );
    }

    /// Apply a member rename typed into the F2 prompt.
    pub(crate) fn finish_zip_rename(
        &mut self,
        archive: PathBuf,
        sub: String,
        from: String,
        is_dir: bool,
        to: String,
    ) {
        if to.is_empty() || to == from || to.contains('/') || to.contains('\\') {
            self.message = Some(tr(self.lang, "invalid name", "無効な名前です").into());
            return;
        }
        let old_prefix = format!("{}{}", sub, from);
        let new_prefix = format!("{}{}", sub, to);
        let pairs: Vec<(String, String)> = if is_dir {
            match self.archive_members(&archive) {
                Ok(all) => members_under(&all, &old_prefix)
                    .into_iter()
                    .map(|m| {
                        let tail = m.name[old_prefix.len()..].to_string();
                        (m.name.clone(), format!("{}{}", new_prefix, tail))
                    })
                    .collect(),
                Err(e) => {
                    self.message = Some(e);
                    return;
                }
            }
        } else {
            vec![(old_prefix, new_prefix)]
        };
        self.run_zip_modify(archive, Vec::new(), pairs, Vec::new(), String::new());
    }

    /// After a zip op lands: re-list and rebuild any pane still browsing the
    /// archive, clamping to the nearest directory that still exists.
    pub(crate) fn refresh_archive_panes(&mut self) {
        use crate::FocusedPane;
        self.archive_cache = None;
        for side in [FocusedPane::Left, FocusedPane::Right] {
            let Some((archive, mut sub)) = self
                .side_pane(side)
                .archive_view()
                .map(|(a, s)| (a.to_path_buf(), s.to_string()))
            else {
                continue;
            };
            let members = match self.archive_members(&archive) {
                Ok(m) => m,
                Err(_) => {
                    // The archive itself is gone: fall back to its directory.
                    if let Some(dir) = archive.parent().map(|p| p.to_path_buf()) {
                        let _ = self.side_pane_mut(side).jump_to(dir);
                    }
                    continue;
                }
            };
            while !sub.is_empty() && !members.iter().any(|m| m.name.starts_with(sub.as_str())) {
                sub = sub
                    .trim_end_matches('/')
                    .rsplit_once('/')
                    .map(|(h, _)| format!("{h}/"))
                    .unwrap_or_default();
            }
            let rows = archive_rows(&archive, &members, &sub);
            self.side_pane_mut(side).enter_archive(archive, sub, rows);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

// The two pure ones live in cian-core: the GUI's engine browses the same
// zips, and two front ends disagreeing about what is inside one would be a
// strange thing to ship.
use cian_core::archive::{archive_rows, members_under};

    fn m(name: &str, is_dir: bool, size: u64) -> Member {
        Member { name: name.to_string(), is_dir, size, compressed: 0 }
    }

    /// The listing contract: direct children only, implied directories shown,
    /// dirs before files, `..` first.
    #[test]
    fn rows_show_direct_children_with_implied_dirs() {
        let members = vec![
            m("top.txt", false, 3),
            m("a/", true, 0),
            m("a/b.txt", false, 1),
            m("a/c/d.txt", false, 2), // `a/c/` never appears explicitly
            m("z.txt", false, 9),
        ];
        let arc = Path::new("/tmp/x.zip");
        let names: Vec<String> =
            archive_rows(arc, &members, "").iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["..", "a", "top.txt", "z.txt"]);

        let inside: Vec<(String, bool)> = archive_rows(arc, &members, "a/")
            .iter()
            .map(|e| (e.name.clone(), e.is_dir))
            .collect();
        assert_eq!(
            inside,
            vec![
                ("..".to_string(), true),
                ("c".to_string(), true), // implied by a/c/d.txt
                ("b.txt".to_string(), false),
            ]
        );
    }

    #[test]
    fn members_under_expands_a_directory_prefix() {
        let members =
            vec![m("a/", true, 0), m("a/b.txt", false, 1), m("a/c/d.txt", false, 2), m("ab.txt", false, 1)];
        let names: Vec<&str> = members_under(&members, "a").iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["a/", "a/b.txt", "a/c/d.txt"], "ab.txt is not under a/");
    }
}
