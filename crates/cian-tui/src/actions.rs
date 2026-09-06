//! Actions on the App that don't belong to a bigger feature module: transfers,
//! delete/rename/create, shortcuts (bookmarks), filter, sort, destination
//! picker, opening the viewer/diff/archive/attributes/hash, recursive
//! find/grep, jump-to-path, the manual, config reload, and the worker-backed
//! op job (progress, elevation retry, external-change polling). Split out of
//! lib.rs as an `impl App` block.
use super::*;

impl App {
    /// Send the clipboard's text to the shell, as typing it would. Raw, with
    /// newlines: pasting a command line into a shell is meant to run it, and
    /// this does not know whether the child enabled bracketed paste, so it
    /// adds no wrapper (a stray `\x1b[200~` would otherwise print as garbage).
    pub(crate) fn paste_text_to_shell(&mut self) {
        match self.clipboard_text() {
            Some(t) => {
                if let Some(s) = self.shell.active_session_mut() {
                    s.write_input(t.as_bytes());
                } else {
                    self.message = Some(tr(self.lang, "no shell to paste into", "貼り付け先のシェルがありません").into());
                }
            }
            None => self.message = Some(tr(self.lang, "clipboard has no text", "クリップボードにテキストがありません").into()),
        }
    }

    // ------- Visual mode -------
    pub(crate) fn visual_start(&mut self) {
        if let Some(p) = self.active_pane() {
            self.visual_anchor = Some(p.cursor);
            self.mode = Mode::Visual;
        }
    }
    pub(crate) fn visual_commit(&mut self) {
        let anchor = match self.visual_anchor.take() {
            Some(a) => a,
            None => { self.mode = Mode::Normal; return; }
        };
        if let Some(p) = self.active_pane_mut() {
            let cur = p.cursor;
            let (a, b) = if anchor <= cur { (anchor, cur) } else { (cur, anchor) };
            for i in a..=b { p.set_mark_at(i); }
        }
        self.mode = Mode::Normal;
    }
    pub(crate) fn visual_cancel_and_clear_all(&mut self) {
        self.visual_anchor = None;
        if let Some(p) = self.active_pane_mut() { p.clear_marks(); }
        self.mode = Mode::Normal;
    }

    // ------- Confirmation flows -------
    pub(crate) fn start_transfer(&mut self, op: PendingOp) {
        // Copying out of an archive is an extraction; moving would mean
        // deleting members, which waits for the zip-write phase.
        if self.in_archive() {
            if matches!(op, PendingOp::Move) {
                self.message = Some(tr(
                    self.lang,
                    "an archive cannot be moved out of. copy extracts instead",
                    "アーカイブ内から移動はできません。コピーで展開してください",
                ).into());
                return;
            }
            if let Some(dest) = self.opposite_pane_cwd() {
                self.archive_copy_out(dest);
            }
            return;
        }
        // Copying toward a pane that is browsing a zip adds the files to it.
        if let Some((archive, sub)) = self
            .opposite_pane_ref()
            .and_then(|p| p.archive_view())
            .map(|(a, s)| (a.to_path_buf(), s.to_string()))
        {
            if matches!(op, PendingOp::Move) {
                self.message = Some(tr(
                    self.lang,
                    "copy adds to the zip; move is not supported",
                    "zipへはコピー（追加）のみ — 移動は未対応",
                ).into());
                return;
            }
            if !self.require_zip_writable(&archive) {
                return;
            }
            let sources = self.target_paths();
            if sources.is_empty() {
                self.message = Some(tr(self.lang, "nothing to operate on", "操作する対象がありません").into());
                return;
            }
            self.open_popup(Popup::ConfirmZipAdd { archive, sub, sources });
            return;
        }
        // Copying to/from a remote pane is an SFTP transfer, not a local copy.
        if self.try_remote_pane_transfer(matches!(op, PendingOp::Move)) {
            return;
        }
        let Some(dest) = self.opposite_pane_cwd() else { return };
        let targets = match self.active_pane() {
            Some(p) => p.target_paths(),
            None => return,
        };
        if targets.is_empty() { self.message = Some(tr(self.lang, "nothing to operate on", "操作する対象がありません").into()); return; }
        self.confirm_transfer(op, targets, dest);
    }

    /// The one way a copy/move confirmation is opened.
    ///
    /// It exists so the collision list is worked out in exactly one place. Six
    /// keys and menu items reach this popup (`c`, `m`, `:cp`, `:mv`, a drag
    /// between panes, a drop from the desktop); a `clashes` field each of them
    /// filled in for itself would be six chances for one of them to pass an
    /// empty list and quietly promise "nothing here will be overwritten".
    pub(crate) fn confirm_transfer(&mut self, op: PendingOp, targets: Vec<PathBuf>, dest: PathBuf) {
        let popup = transfer_popup(op, targets, dest);
        self.open_popup(popup);
    }
    pub(crate) fn start_delete(&mut self) {
        if self.in_archive() {
            self.archive_delete();
            return;
        }
        let targets = match self.active_pane() {
            Some(p) => p.target_paths(),
            None => return,
        };
        if targets.is_empty() { self.message = Some(tr(self.lang, "nothing to delete", "削除する対象がありません").into()); return; }
        self.open_popup(Popup::ConfirmDelete { targets });
    }
    pub(crate) fn start_rename(&mut self) {
        if self.in_archive() {
            self.archive_rename_start();
            return;
        }
        let Some(p) = self.active_pane() else { return };
        let Some(e) = p.selected() else { return };
        self.open_popup(text_input(
                "rename",
                "new name:",
                e.name.clone(),
                InputKind::Rename { original: e.path.clone() },
            ));
    }
    /// `:renamepattern` / right-click: pattern-based bulk rename of the marked files
    /// (or the one under the cursor). Prompts for the pattern; the proposed
    /// names are shown for review (the same checklist the AI rename uses)
    /// before anything touches disk.
    pub(crate) fn start_bulk_rename(&mut self) {
        let targets = self.target_paths();
        if targets.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected to rename", "リネーム対象がありません").into());
            return;
        }
        self.open_popup(text_input(
            "bulk rename",
            "pattern:  {name}_{n3}.{ext}   or   s/regex/replacement/gi",
            String::new(),
            InputKind::BulkRenamePattern { targets },
        ));
    }

    /// Build the rename review from a bulk pattern applied to `targets`. Reports
    /// a bad pattern rather than opening an empty review.
    pub(crate) fn build_bulk_rename(&mut self, targets: &[PathBuf], pattern: &str) {
        let names: Vec<String> = targets
            .iter()
            .map(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
            .collect();
        let news = match cian_core::rename::plan_batch(pattern, &names, cian_core::rename::Numbering::default()) {
            Ok(v) => v,
            Err(e) => {
                self.message = Some(format!("rename pattern: {}", e));
                return;
            }
        };
        // Only offer entries that actually change to a non-empty, path-free name.
        let items: Vec<RenameItem> = targets
            .iter()
            .zip(names.iter())
            .zip(news.iter())
            .filter(|((_, old), new)| new.as_str() != old.as_str() && !new.is_empty() && !new.contains('/') && !new.contains('\\'))
            .map(|((path, old), new)| RenameItem {
                path: path.clone(),
                old: old.clone(),
                new: new.clone(),
                selected: true,
            })
            .collect();
        if items.is_empty() {
            self.message = Some(tr(self.lang, "the pattern changed no names", "パターンで変わる名前がありません").into());
            return;
        }
        self.open_popup(Popup::RenameReview { items, cursor: 0, scroll: 0 });
    }

    pub(crate) fn start_new_file(&mut self) {
        let Some(p) = self.active_pane() else { return };
        self.open_popup(text_input(
                "new file",
                "name:",
                String::new(),
                InputKind::NewFile { parent: p.cwd.clone() },
            ));
    }
    pub(crate) fn start_new_dir(&mut self) {
        let Some(p) = self.active_pane() else { return };
        self.open_popup(text_input(
                "new directory",
                "name:",
                String::new(),
                InputKind::NewDir { parent: p.cwd.clone() },
            ));
    }

    // ------- Search -------
    pub(crate) fn start_search(&mut self) {
        self.open_popup(Popup::Search { buffer: String::new() });
        self.mode = Mode::Search;
    }

    pub(crate) fn finish_search(&mut self) {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let buffer = if let Popup::Search { buffer } = popup { buffer } else { return };
        self.mode = Mode::Normal;
        let q = buffer.trim().to_string();
        if q.is_empty() { return; }
        self.last_search_query = Some(q.clone());
        let ql = q.to_lowercase();
        if let Some(p) = self.active_pane_mut() {
            if let Some(i) = p.entries.iter().position(|e| e.name.to_lowercase().contains(&ql)) {
                p.cursor = i;
            } else {
                self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("パターンが見つかりません: {}", q)
        } else {
            format!("pattern not found: {}", q)
        });
            }
        }
    }

    /// `:where` — show exactly which config files cian is reading and writing.
    /// The one question this answers: when an edit "isn't reflected", which
    /// `shortcuts.lua` (or init.lua …) is cian actually using? A portable copy
    /// next to the executable silently wins over `~/.config/cian`.
    pub(crate) fn show_config_paths(&mut self) {
        let mut lines: Vec<String> = Vec::new();
        let portable = cian_lua::is_portable();
        lines.push(format!(
            "portable mode: {}",
            if portable { "YES — files next to the .exe win over ~/.config/cian" } else { "no" }
        ));
        lines.push(String::new());
        // Each config file: the path cian resolves for reading, and its status.
        for name in ["init.lua", "ssh.lua", "keymap.lua", "shortcuts.lua", "macro.lua", "count.lua", "state.toml"] {
            let p = cian_lua::config_read_path(name);
            let (path_str, status) = match &p {
                Some(p) if p.exists() => {
                    // For shortcuts.lua, check it actually parses — a bad
                    // hand-edit is otherwise ignored in silence.
                    let st = if name == "shortcuts.lua" {
                        match cian_lua::shortcuts::load(p) {
                            Ok(n) => format!("loaded, {} entr{}", n.len(), if n.len() == 1 { "y" } else { "ies" }),
                            Err(e) => format!("PARSE ERROR: {}", e),
                        }
                    } else {
                        "present".into()
                    };
                    (p.display().to_string(), st)
                }
                Some(p) => (p.display().to_string(), "not present".into()),
                None => ("(unresolved)".into(), String::new()),
            };
            lines.push(format!("{:<14} {}", format!("{}:", name), path_str));
            if !status.is_empty() {
                lines.push(format!("               → {}", status));
            }
        }
        lines.push(String::new());
        // Where diagnostics are going, and whether they are going anywhere.
        //
        // Asked here because this is the command someone runs when nothing is
        // where they expected it, and because the answer may not be the path
        // they asked for: a `CIAN_LOG` that cannot be written to falls back to
        // the temp directory rather than to silence. An evening was lost to a
        // log that was being written to a Desktop folder which, on a machine
        // whose Desktop is OneDrive's, does not exist.
        match cian_core::log::destination() {
            Some(p) => {
                let asked = std::env::var("CIAN_LOG").unwrap_or_default();
                lines.push(format!("log:             {}", p.display()));
                if !asked.is_empty() && std::path::Path::new(&asked) != p.as_path() {
                    lines.push(format!("               → asked for {asked}, which could not be written"));
                }
            }
            None => lines.push("log:             off (set CIAN_LOG=<file> to turn it on)".into()),
        }
        lines.push(String::new());
        lines.push(format!(
            "exe dir:         {}",
            cian_lua::exe_dir().map(|p| p.display().to_string()).unwrap_or_else(|| "?".into())
        ));
        lines.push(format!(
            "user config dir: {}",
            cian_lua::user_config_dir().map(|p| p.display().to_string()).unwrap_or_else(|| "?".into())
        ));
        // The home confusion: a stray HOME (Git Bash/MSYS) redirects ~/.config.
        let home = std::env::var("HOME").unwrap_or_default();
        let up = std::env::var("USERPROFILE").unwrap_or_default();
        if !home.is_empty() {
            lines.push(format!("HOME=           {}", home));
        }
        if !up.is_empty() {
            lines.push(format!("USERPROFILE=    {}", up));
        }
        if !home.is_empty() && !up.is_empty() && home != up {
            lines.push("note: HOME differs from USERPROFILE — cian uses HOME for ~/.config/cian.".into());
        }
        self.open_popup(Popup::Notice { lines });
    }

    // ------- Shortcuts -------
    pub(crate) fn start_shortcuts(&mut self) {
        self.open_popup(Popup::Shortcuts {
            entries: self.shortcuts.entries.clone(),
            cursor: 0,
            path: Vec::new(),
        });
    }

    /// Re-open the shortcuts popup at `path`/`cursor` from the saved store (used
    /// after an add/edit/delete so the view reflects the change).
    /// Write the bookmarks out and put the list back on screen — or say why
    /// it could not be written.
    ///
    /// One function because there were three copies of this and only one of
    /// them reported a failure: adding a bookmark said "save failed", while
    /// deleting one and making a group silently did not persist. A bookmark
    /// is something the user made, not a preference — losing one quietly is
    /// worse than an interruption.
    pub(crate) fn save_shortcuts(&mut self, path: Vec<usize>, cursor: usize, said: &str) {
        match self.shortcuts.save() {
            Ok(()) => {
                if !said.is_empty() {
                    self.message = Some(said.to_string());
                }
                self.reopen_shortcuts(path, cursor);
            }
            Err(e) => {
                self.open_popup(Popup::Notice {
                    lines: vec![
                        tr(self.lang, "the bookmarks could not be saved:", "ブックマークを保存できませんでした:")
                            .to_string(),
                        e.to_string(),
                        String::new(),
                        tr(
                            self.lang,
                            "The change is in this session only. `:where` says which file it would go to",
                            "変更はこのセッション限りです。保存場所は `:where` で確認できます",
                        )
                        .to_string(),
                    ],
                })}
        }
    }

    /// `:image` — draw pictures with the terminal's own protocol, or with
    /// half-blocks.
    ///
    /// A terminal can advertise a protocol and then show nothing: the escape
    /// sequences go out through the same pipe as the rest of the drawing, and
    /// whatever swallowed them is invisible from in here. Rather than leave
    /// someone with no pictures at all, this turns the offer down. Remembered
    /// between sessions, since a terminal that did it once will do it again.
    pub(crate) fn toggle_image_protocol(&mut self) {
        // auto → iterm2 → kitty → sixel → half-blocks → auto. A terminal can
        // be wrong about itself: iTerm2 answering the kitty query and then
        // drawing nothing is what this is for, and its own protocol is one
        // step away rather than a config file away.
        let next = match state_get("images").as_deref() {
            None | Some("auto") => "iterm2",
            Some("iterm2") => "kitty",
            Some("kitty") => "sixel",
            Some("sixel") => "blocks",
            _ => "auto",
        };
        state_set("images", next);
        self.gfx_picker = image_picker(next);
        self.gfx_failed = false;
        self.preview_gfx = None;
        self.img_proto = None;
        self.full_clear = true;
        self.message = Some(match self.gfx_picker.as_ref() {
            Some(p) => format!(
                "{} {:?} ({})",
                tr(self.lang, "pictures:", "画像:"),
                p.protocol_type(),
                tr(self.lang, ":image for the next one", ":image で次の方式へ"),
            ),
            None => format!(
                "{} ({})",
                tr(self.lang, "pictures: half-blocks", "画像: 半角ブロック"),
                tr(self.lang, ":image for the next one", ":image で次の方式へ"),
            ),
        });
    }

    /// `:version` — which build is running.
    ///
    /// Unanswerable from inside a session until it existed, and a cian left
    /// open across a rebuild looks exactly like a fix that did not work.
    pub(crate) fn show_version(&mut self) {
        // What the terminal said it can do, because "images do not show" has
        // two very different causes — a terminal that offered no picture
        // protocol (half-blocks, and they should still appear) and one that
        // offered a protocol that then draws nothing.
        let gfx = match self.gfx_picker.as_ref() {
            Some(p) => format!("{:?}", p.protocol_type()),
            None => tr(self.lang, "half-blocks (no protocol offered)", "半角ブロック（プロトコルなし）")
                .to_string(),
        };
        self.open_popup(Popup::Notice {
            lines: vec![
                crate::version_text(),
                format!("{}: {}", tr(self.lang, "images", "画像"), gfx),
            ],
        });
    }

    pub(crate) fn reopen_shortcuts(&mut self, path: Vec<usize>, cursor: usize) {
        let n = sc_level(&self.shortcuts.entries, &path).len();
        self.open_popup(Popup::Shortcuts {
            entries: self.shortcuts.entries.clone(),
            cursor: cursor.min(n.saturating_sub(1)),
            path,
        });
    }

    /// Prompt for a new shortcut's name in the group at `path`. `group` makes a
    /// folder (name only, no target step).
    pub(crate) fn start_shortcut_add(&mut self, path: Vec<usize>, group: bool) {
        let title = if group { "new folder — name" } else { "new shortcut — name" };
        self.open_popup(text_input(
            title,
            "name:",
            String::new(),
            InputKind::ShortcutName { path, edit_idx: None, group },
        ));
    }

    pub(crate) fn start_shortcut_edit(&mut self, path: Vec<usize>, idx: usize) {
        let Some(s) = sc_level(&self.shortcuts.entries, &path).get(idx).cloned() else { return };
        let group = s.is_group();
        self.open_popup(text_input(
            "edit shortcut — name",
            "name:",
            s.name,
            InputKind::ShortcutName { path, edit_idx: Some(idx), group },
        ));
    }

    pub(crate) fn copy_paths_to_clipboard(&mut self) {
        let paths = match self.active_pane() {
            Some(p) => p.target_paths(),
            None => return,
        };
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing to copy", "コピーする対象がありません").into());
            return;
        }
        let Some(cb) = self.clipboard.as_mut() else {
            self.message = Some(tr(self.lang, "clipboard unavailable", "クリップボードを利用できません").into());
            return;
        };
        let text = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n");
        match cb.set_text(text) {
            Ok(()) => self.message = Some(format!("◂ copied {} path(s) to clipboard", paths.len())),
            Err(e) => self.message = Some(format!("clipboard error: {}", e)),
        }
    }

    pub(crate) fn copy_file_refs_to_clipboard(&mut self) {
        let paths = match self.active_pane() {
            Some(p) => p.target_paths(),
            None => return,
        };
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing to copy", "コピーする対象がありません").into());
            return;
        }
        match os_clipboard_file_refs(&paths) {
            Ok(()) => self.message = Some(format!("◂ copied {} file ref(s) to clipboard", paths.len())),
            Err(e) => self.message = Some(format!("file-ref clipboard failed: {}", e)),
        }
    }

    pub(crate) fn copy_shortcut_target_to_clipboard(&mut self, path: &[usize], idx: usize) {
        let Some(entry) = sc_level(&self.shortcuts.entries, path).get(idx).cloned() else { return };
        let target = entry.target_str().to_string();
        let Some(cb) = self.clipboard.as_mut() else {
            self.message = Some(tr(self.lang, "clipboard unavailable", "クリップボードを利用できません").into());
            return;
        };
        match cb.set_text(target.clone()) {
            Ok(()) => self.message = Some(format!("◂ copied: {}", truncate(&target, 50))),
            Err(e) => self.message = Some(format!("clipboard error: {}", e)),
        }
    }

    pub(crate) fn execute_shortcut(&mut self, path: &[usize], idx: usize) -> Result<()> {
        let Some(entry) = sc_level(&self.shortcuts.entries, path).get(idx).cloned() else { return Ok(()) };
        // Groups are descended in the key handler, not executed.
        if entry.is_group() {
            return Ok(());
        }
        let target = entry.target_str().to_string();

        // URL?
        if target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("file://")
        {
            let _ = os_open(&target);
            self.message = Some(format!("◂ {}", entry.name));
            return Ok(());
        }

        let path = expand_tilde(Path::new(&target));

        // macOS .app bundles are technically directories. Always hand them to
        // `open` so the app launches instead of cd-ing into the package.
        let is_app_bundle = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("app"))
            .unwrap_or(false);
        if is_app_bundle && path.exists() {
            match os_open(&path) {
                Ok(()) => self.message = Some(format!("◂ {}", entry.name)),
                Err(e) => self.message = Some(format!("shortcut failed: {}", e)),
            }
            return Ok(());
        }

        // Plain directory → navigate.
        if path.is_dir() {
            if let Some(p) = self.active_pane_mut() {
                p.jump_to(path)?;
            }
            self.message = Some(format!("◂ {}", entry.name));
            return Ok(());
        }

        // File or other existing entity → OS default.
        if path.exists() {
            let _ = os_open(&path);
            self.message = Some(format!("◂ {}", entry.name));
            return Ok(());
        }

        // Fallback: hand off the raw string to the OS opener (e.g. unknown protocols).
        match os_open(&target) {
            Ok(()) => self.message = Some(format!("◂ {}", entry.name)),
            Err(e) => self.message = Some(format!("shortcut failed: {}", e)),
        }
        Ok(())
    }

    // ------- History -------
    /// `Alt+←` / `Alt+h` — the browser's back arrow, over this pane's
    /// directory history. Says so when there is nowhere to go, rather than
    /// swallowing the key.
    pub(crate) fn pane_go_back(&mut self) {
        let moved = self.active_pane_mut().map(|p| p.go_back().unwrap_or(false)).unwrap_or(false);
        self.message = Some(if moved {
            let cwd = self.active_pane().map(|p| p.cwd.display().to_string()).unwrap_or_default();
            format!("◀ {cwd}")
        } else {
            tr(self.lang, "no earlier directory", "戻れる履歴がありません").into()
        });
    }

    /// `Alt+→` / `Alt+l` — forward again, undoing a back step.
    pub(crate) fn pane_go_forward(&mut self) {
        let moved = self.active_pane_mut().map(|p| p.go_forward().unwrap_or(false)).unwrap_or(false);
        self.message = Some(if moved {
            let cwd = self.active_pane().map(|p| p.cwd.display().to_string()).unwrap_or_default();
            format!("▶ {cwd}")
        } else {
            tr(self.lang, "nothing to go forward to", "進める履歴がありません").into()
        });
    }

    pub(crate) fn start_history(&mut self) {
        let entries = self.active_pane().map(|p| p.history.clone()).unwrap_or_default();
        if entries.is_empty() {
            self.message = Some(tr(self.lang, "no history yet", "履歴がまだありません").into());
            return;
        }
        self.open_popup(Popup::History { entries, cursor: 0 });
    }

    pub(crate) fn finish_history(&mut self) -> Result<()> {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let (entries, cursor) = if let Popup::History { entries, cursor } = popup {
            (entries, cursor)
        } else { return Ok(()) };
        let Some(target) = entries.get(cursor).cloned() else { return Ok(()) };
        if let Some(p) = self.active_pane_mut() {
            p.jump_to(target)?;
        }
        Ok(())
    }

    // ------- Incremental filter -------
    /// Start filtering, seeded with the pane's current filter so `/` reopens
    /// and edits an existing narrowing rather than discarding it.
    pub(crate) fn start_filter(&mut self) {
        self.filter_buffer = self.active_pane().map(|p| p.filter.clone()).unwrap_or_default();
        self.mode = Mode::Filter;
    }

    /// Push the buffer into the pane, narrowing the listing as the user types.
    pub(crate) fn apply_filter_buffer(&mut self) {
        let buf = self.filter_buffer.clone();
        if let Some(p) = self.active_pane_mut() {
            p.set_filter(buf);
        }
    }

    pub(crate) fn handle_filter_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            // Esc abandons the narrowing entirely and restores the full list.
            KeyCode::Esc => {
                self.filter_buffer.clear();
                if let Some(p) = self.active_pane_mut() {
                    p.clear_filter();
                }
                self.mode = Mode::Normal;
            }
            // Enter keeps the filter applied and returns to normal keys, so the
            // narrowed list can be marked and operated on.
            KeyCode::Enter => self.mode = Mode::Normal,
            KeyCode::Backspace => {
                self.filter_buffer.pop();
                self.apply_filter_buffer();
            }
            KeyCode::Up => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(-1); }
            }
            KeyCode::Down => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(1); }
            }
            // `//` opens the fuzzy finder. A slash cannot appear in a filename
            // on any platform — it is the separator on Unix and an illegal
            // character on Windows — and the filter matches names alone, so a
            // leading slash was input that could never match anything. It costs
            // nothing to give it a meaning, and the meaning writes itself: one
            // slash narrows what is here, two goes looking underneath.
            KeyCode::Char('/') if self.filter_buffer.is_empty() => {
                self.filter_buffer.clear();
                if let Some(p) = self.active_pane_mut() {
                    p.clear_filter();
                }
                self.mode = Mode::Normal;
                self.start_file_finder();
            }
            KeyCode::Char(c) => {
                self.filter_buffer.push(c);
                self.apply_filter_buffer();
            }
            _ => {}
        }
        Ok(())
    }

    // ------- SSH -------

    /// Type an AI-suggested command at the shell prompt WITHOUT running it —
    /// the user reviews it and presses Enter. Focuses the shell.
    pub(crate) fn insert_ai_command_at_prompt(&mut self, cmd: &str) {
        let cwd = self.shell_cwd();
        self.shell.ensure(&cwd);
        self.focus(FocusedPane::Shell);
        match self.shell.active_session_mut() {
            Some(s) => s.write_input(cmd.as_bytes()),
            None => self.pending_shell_input = Some(cmd.to_string()),
        }
        self.message = Some(tr(self.lang, "command at prompt. review and press Enter", "プロンプトに入れました。確認して Enter").into());
    }

    /// Send a command line to the shell panel, starting the shell if needed.
    pub(crate) fn run_in_shell(&mut self, mut cmd: String) {
        cmd.push('\n');
        let cwd = self.shell_cwd();
        self.shell.ensure(&cwd);
        self.focus(FocusedPane::Shell);
        match self.shell.active_session_mut() {
            Some(s) => s.write_input(cmd.as_bytes()),
            // Still spawning: hand it to `poll_pending`'s follow-up.
            None => self.pending_shell_input = Some(cmd),
        }
    }

    /// `:each <template>` — run a shell command once per marked file (or the
    /// file under the cursor when nothing is marked). `{}` in the template is
    /// replaced by each file's path, double-quoted; with no `{}` the quoted path
    /// is appended. The commands are sent to the active shell in order, so they
    /// run in the user's own environment and are visible as they go.
    pub(crate) fn run_each(&mut self, template: &str) {
        let template = template.trim();
        if template.is_empty() {
            self.message = Some(tr(
                self.lang,
                "usage: :each <command with {} for each file>",
                "使い方: :each <コマンド（{} が各ファイルに展開）>",
            ).into());
            return;
        }
        let paths = self.target_paths();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択なし").into());
            return;
        }
        let (lines, skipped) = each_lines(template, &paths);
        if lines.is_empty() {
            self.message = Some(tr(
                self.lang,
                "no usable paths — a name holds \" $ ` or a line break, which the shell would read",
                "使えるパスがありません — 名前に \" $ ` か改行があり、シェルが解釈してしまいます",
            ).into());
            return;
        }
        let n = lines.len();
        self.run_in_shell(lines.join("\n"));
        self.message = Some(if skipped > 0 {
            format!("each: sent {n} command(s), skipped {skipped}")
        } else {
            format!("each: sent {n} command(s)")
        });
    }

    // ------- Snippets -------

    /// `:snip` / right-click: open the command-snippet launcher.
    pub(crate) fn start_snippets(&mut self) {
        if self.config.snippets.is_empty() {
            self.message = Some(
                tr(self.lang, "no snippets configured (cian.snippets{…} in init.lua)",
                   "スニペット未設定（init.lua の cian.snippets{…}）").into(),
            );
            return;
        }
        self.open_popup(Popup::Snippets { cursor: 0, filter: String::new() });
    }

    /// The snippets matching `filter` (case-insensitive, over name and command),
    /// paired with their index in `config.snippets`.
    pub(crate) fn snippet_matches(&self, filter: &str) -> Vec<(usize, &cian_lua::Snippet)> {
        let needle = filter.to_lowercase();
        self.config
            .snippets
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                needle.is_empty()
                    || s.name.to_lowercase().contains(&needle)
                    || s.cmd.to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// Send the snippet at `index` to the shell — after a confirm when it is
    /// flagged. Splits from `deliver_snippet` so the confirm path shares it.
    pub(crate) fn send_snippet(&mut self, index: usize) {
        let Some(s) = self.config.snippets.get(index) else { return };
        let (name, cmd, enter, confirm) = (s.name.clone(), s.cmd.clone(), s.enter, s.confirm);
        if confirm {
            self.open_popup(Popup::ConfirmSnippet { name, cmd, enter });
        } else {
            self.deliver_snippet(&cmd, enter);
        }
    }

    /// Actually hand a snippet to the shell: run it (with a newline) when
    /// `enter`, otherwise type it at the prompt for review.
    pub(crate) fn deliver_snippet(&mut self, cmd: &str, enter: bool) {
        if enter {
            self.run_in_shell(cmd.to_string());
            self.message = Some(tr(self.lang, "snippet sent", "スニペットを送信").into());
        } else {
            self.insert_ai_command_at_prompt(cmd);
        }
    }

    /// Deliver a command queued while the shell was still starting.
    pub(crate) fn flush_pending_shell_input(&mut self) {
        let Some(cmd) = self.pending_shell_input.take() else { return };
        match self.shell.active_session_mut() {
            Some(s) => s.write_input(cmd.as_bytes()),
            // Not ready yet — put it back and try again next tick.
            None => self.pending_shell_input = Some(cmd),
        }
    }

    // ------- Sorting -------
    pub(crate) fn start_sort_picker(&mut self) {
        // Open on the pane's current key, so the picker shows where you are.
        let cur = self
            .active_pane()
            .and_then(|p| SortKey::ALL.iter().position(|k| *k == p.sort.key))
            .unwrap_or(0);
        self.open_popup(Popup::SortPicker { cursor: cur });
    }

    /// Apply a sort key. Choosing the key that is already active flips the
    /// direction, which is how column headers behave everywhere else.
    pub(crate) fn apply_sort_key(&mut self, key: SortKey) {
        let Some(p) = self.active_pane_mut() else { return };
        let reverse = if p.sort.key == key { !p.sort.reverse } else { false };
        p.set_sort(Sort { key, reverse });
        let arrow = if reverse { "▼" } else { "▲" };
        let lang = self.lang;
        self.message = Some(format!(
            "{}: {} {}",
            tr(lang, "sort", "並び"),
            crate::render::sort_label(key, lang),
            arrow
        ));
    }

    /// Note a directory as a copy/move destination.
    ///
    /// Most transfers go to the other pane, but the ones that do not tend to
    /// repeat — a build output, a share, a scratch folder — and retyping the
    /// path each time is the tedious part.
    pub(crate) fn remember_dest(&mut self, dest: &Path) {
        self.dest_history.retain(|p| p != dest);
        self.dest_history.insert(0, dest.to_path_buf());
        self.dest_history.truncate(DEST_HISTORY_CAP);
    }

    /// Offer somewhere other than the opposite pane to send the selection.
    pub(crate) fn start_dest_picker(&mut self, op: PendingOp) {
        let targets = self.target_paths();
        if targets.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        }
        self.open_popup(Popup::DestPicker { op, targets, cursor: 0 });
    }

    /// Rows of the destination picker: the opposite pane first, then history.
    pub(crate) fn dest_choices(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        if let Some(other) = self.opposite_pane_cwd() {
            out.push(("other pane".to_string(), other));
        }
        for p in &self.dest_history {
            if out.iter().any(|(_, q)| q == p) {
                continue;
            }
            out.push(("recent".to_string(), p.clone()));
        }
        out
    }

    // ------- Looking inside things -------

    /// F3: show what is in the highlighted entry.
    ///
    /// One key for both because the question is the same — "what is in here" —
    /// and the answer's shape follows from the file: an archive lists its
    /// members, anything else is read.
    /// `F3` — read the file in the *other* pane.
    ///
    /// It used to mean "the same file, but over the whole window", which F12
    /// now does to any surface, panel included. Rather than leave the key
    /// meaning nothing, it takes the shape the two panes are for: the listing
    /// stays where it is and the file opens beside it — what `o` and
    /// `Shift+O` already say about a directory, said about a file.
    pub(crate) fn look_inside_other(&mut self) {
        let here = match self.focused {
            FocusedPane::Shell => self.last_file_pane,
            p => p,
        };
        let there = match here {
            FocusedPane::Left => FocusedPane::Right,
            _ => FocusedPane::Left,
        };
        // The file is the one under *this* pane's cursor; it is read over
        // there. If a file is already open over there, this one joins it as
        // another tab — replacing what someone is reading, unasked, is the
        // one thing this key must not do.
        self.focus(here);
        let joining = matches!(self.popup, Popup::Viewer { .. }) && self.viewer_dock == Some(there);
        let stashed = joining.then(|| std::mem::replace(&mut self.popup, Popup::None));
        self.look_inside();
        if matches!(self.popup, Popup::Viewer { .. }) {
            if let Some(old) = stashed {
                // The one that was there goes into the strip in front of the
                // new one, which is the one now being read.
                let at = self.viewer_tab_idx.min(self.viewer_tabs.len());
                self.viewer_tabs.insert(at, old);
                self.viewer_tab_idx = at + 1;
            }
            self.viewer_dock = Some(there);
            self.focus(there);
            self.full_clear = true;
        } else if let Some(old) = stashed {
            // Nothing opened — put back what was being read.
            self.popup = old;
        }
    }

    pub(crate) fn look_inside(&mut self) {
        // On a remote pane, fetch the file first and view the local copy.
        if self.active_pane().map(|p| p.is_remote()).unwrap_or(false) {
            self.remote_pane_view();
            return;
        }
        // Inside an archive, F3 on a member extracts it to a temp file and
        // opens the normal viewer on that.
        if let Some((archive, sub)) = self
            .active_pane()
            .and_then(|p| p.archive_view())
            .map(|(a, s)| (a.to_path_buf(), s.to_string()))
        {
            match self.active_pane().and_then(|p| p.selected()).cloned() {
                Some(e) if e.is_parent || e.is_dir => {
                    self.message = Some(tr(self.lang, "that is a directory. Enter to go in", "ディレクトリです。Enter で入れます").into());
                }
                Some(e) => self.archive_view_member(&archive, &format!("{}{}", sub, e.name)),
                None => self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into()),
            }
            return;
        }
        // Marked files open together, as tabs. Having marked them is the way
        // of saying "these ones", and opening the first and forgetting the
        // rest answers a question that was not asked.
        let marked: Vec<PathBuf> = self
            .active_pane()
            .map(|p| {
                p.entries
                    .iter()
                    .filter(|e| !e.is_dir && !e.is_parent && p.marks.contains(&e.path))
                    .map(|e| e.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        if marked.len() > 1 {
            // A ceiling, because each one is read into memory as it opens.
            const MAX_TABS: usize = 12;
            let shown = marked.len().min(MAX_TABS);
            self.open_viewer_tabs(&marked[..shown]);
            if marked.len() > shown {
                self.message = Some(format!(
                    "{shown} of {} opened — the viewer holds {MAX_TABS}",
                    marked.len()
                ));
            }
            return;
        }
        let Some(entry) = self.active_pane().and_then(|p| p.selected().cloned()) else {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        };
        if entry.is_dir {
            self.message = Some(tr(self.lang, "that is a directory. Enter to go in", "ディレクトリです。Enter で入れます").into());
            return;
        }
        if cian_core::archive::is_archive(&entry.path) {
            match cian_core::archive::list(&entry.path) {
                Ok(members) => {
                    self.open_popup(Popup::Archive {
                        path: entry.path,
                        members,
                        cursor: 0,
                        scroll: 0,
                    });
                    return;
                }
                // Named like an archive but unreadable as one: fall through to
                // the viewer rather than refusing outright.
                Err(e) => self.message = Some(format!("not a readable archive: {}", e)),
            }
        }
        self.open_viewer_at(&entry.path, &entry.name, 0);
    }

    /// Open the F3 viewer on `path`, with the cursor on `line0` (0-based). Used
    /// by F3 (line 0) and by "open a grep hit at its line".
    pub(crate) fn open_viewer_at(&mut self, path: &Path, title: &str, line0: usize) {
        // Remember it for the recent-files finder (skip the remote temp copies).
        self.note_recent_file(path);
        // Images preview as half-block cells rather than a hex dump.
        if cian_core::image::is_image(path) {
            self.open_popup(Popup::ImageView {
                path: path.to_path_buf(),
                title: title.to_string(),
                shown: None,
                error: None,
            });
            return;
        }
        // Office/PDF documents are extracted to text first (fully in-process, no
        // external converter), then shown in the ordinary viewer so search,
        // selection and copy all work over them.
        if cian_core::office::classify(path).is_some() {
            self.open_document_viewer(path, title, line0);
            return;
        }
        match cian_core::viewer::view_file(path) {
            Ok(view) => {
                // The change gutter: which lines differ from the VCS base. Best
                // effort — git first, then svn; empty when the file is not
                // version-controlled or is unchanged.
                let git_lines = path
                    .parent()
                    .and_then(|dir| {
                        cian_core::git::line_changes(dir, path)
                            .or_else(|| cian_core::svn::line_changes(dir, path))
                    })
                    .unwrap_or_default();
                let last = view.lines.len().saturating_sub(1);
                let line = line0.min(last);
                // Markdown files open in rendered preview; `p` toggles the source.
                let markdown = matches!(
                    path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
                    Some("md") | Some("markdown") | Some("mkd") | Some("mdown")
                );
                let source = view.lines.clone();
                // A truncated view (file over VIEW_LIMIT) must never be
                // editable: saving would write back only the visible 4MB and
                // silently destroy the rest of the file. Binary files edit as
                // hex (overwrite-only), under the same truncation rule.
                let editable = matches!(
                    view.kind,
                    cian_core::viewer::ViewKind::Text | cian_core::viewer::ViewKind::Binary
                ) && !view.truncated;
                // Highlight recognised code (Markdown keeps its rendered preview).
                let hl_lang = (!markdown && editable)
                    .then(|| cian_core::highlight::detect(path))
                    .flatten();
                // Read the file's shape from the source, not from `view.lines`
                // — for Markdown those are about to become the rendered
                // preview, whose headings have had their `#` marks taken off.
                let shape = crate::Shape::read(path, &source, None);
                self.popup = Popup::Viewer {
                    title: title.to_string(),
                    path: path.to_path_buf(),
                    stamp: cian_core::stamp::of(path),
                    view: Box::new(view),
                    scroll: line.saturating_sub(4), // show a little context above
                    line,
                    col: 0,
                    goal: 0,
                    visual: None,
                    anchor: (0, 0),
                    find_input: None,
                    sub_input: None,
                    block_input: None,
                    shape,
                    sub_walk: None,
                    find_query: None,
                    count: None,
                    pending: None,
                    git_lines,
                    markdown,
                    preview: markdown,
                    source,
                    md_styles: Vec::new(),
                    md_map: Vec::new(),
                    md_width: 0,
                    md_gen: 0,
                    md_seek: None,
                    blame: Vec::new(),
                    hl_lang,
                    hl: Vec::new(),
                    // A real text file (not a hex dump) can be edited in place.
                    editable,
                    editing: false,
                    dirty: false,
                    undo: Vec::new(),
                    hscroll: 0,
                    block_eol: false,
                    replacing: false,
                    replace: None,
                    redo: Vec::new(),
                };
                // Notepad has no mode to be out of: an editable file opens
                // already taking text. Vim opens where vi opens.
                self.sync_edit_style();
            }
            Err(e) => self.message = Some(format!("cannot view: {}", e)),
        }
    }

    /// Open an Office/PDF document as extracted text in the viewer.
    pub(crate) fn open_document_viewer(&mut self, path: &Path, title: &str, line0: usize) {
        let total_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        match cian_core::office::extract(path) {
            Ok((doc, mut lines)) => {
                // A one-line header naming the format, and — for the legacy
                // binary formats — an honest note that the text is approximate.
                let mut header = vec![format!("── {} ──", doc.label())];
                if doc.is_best_effort() {
                    header.push(tr(
                        self.lang,
                        "(legacy binary — best-effort text; re-save as the modern format for a faithful view)",
                        "(旧バイナリ形式 — テキスト抽出は簡易です。正確な表示には新形式で保存し直してください)",
                    ).to_string());
                }
                header.push(String::new());
                lines.splice(0..0, header);

                let text = lines.join("\n");
                let view = cian_core::viewer::View::from_text(text, total_bytes, false);
                let last = view.lines.len().saturating_sub(1);
                let line = line0.min(last);
                let source = view.lines.clone();
                self.popup = Popup::Viewer {
                    title: format!("{}  ·  {}", title, doc.label()),
                    path: path.to_path_buf(),
                    stamp: cian_core::stamp::of(path),
                    view: Box::new(view),
                    scroll: line.saturating_sub(4),
                    line,
                    col: 0,
                    goal: 0,
                    visual: None,
                    anchor: (0, 0),
                    find_input: None,
                    sub_input: None,
                    block_input: None,
                    shape: None,
                    sub_walk: None,
                    find_query: None,
                    count: None,
                    pending: None,
                    git_lines: std::collections::HashMap::new(),
                    markdown: false,
                    preview: false,
                    source,
                    md_styles: Vec::new(),
                    md_map: Vec::new(),
                    md_width: 0,
                    md_gen: 0,
                    md_seek: None,
                    blame: Vec::new(),
                    hl_lang: None,
                    hl: Vec::new(),
                    // Extracted document text is not the file on disk; read-only.
                    editable: false,
                    editing: false,
                    dirty: false,
                    undo: Vec::new(),
                    hscroll: 0,
                    block_eol: false,
                    replacing: false,
                    replace: None,
                    redo: Vec::new(),
                };
                self.sync_edit_style();
            }
            Err(e) => self.message = Some(format!("cannot read document: {}", e)),
        }
    }

    /// Compare the file under the left pane's cursor with the right pane's.
    ///
    /// Deliberately not "the focused pane against the other one": the whole
    /// gesture is to put A on the left and B on the right, and which pane the
    /// cursor happens to be in at the moment of pressing the key should not
    /// silently swap the two sides of the result.
    pub(crate) fn open_diff(&mut self) {
        // The `..` row is never a comparison subject; treat it as no selection.
        let pick = |t: &PaneTabs| t.active_ref().selected().filter(|e| !e.is_parent).cloned();
        let (Some(a), Some(b)) = (pick(&self.left), pick(&self.right)) else {
            self.message = Some(tr(self.lang, "select a file (or a folder) in each pane to compare", "比較するには各ペインでファイル（またはディレクトリ）を選んでください").into());
            return;
        };
        // Two directories: a recursive tree comparison. Two files: a line diff.
        if a.is_dir && b.is_dir {
            self.start_dir_compare(a.path.clone(), b.path.clone(), a.name.clone(), b.name.clone());
            return;
        }
        if a.is_dir || b.is_dir {
            self.message = Some(tr(self.lang, "compare two files, or two folders. not one of each", "ファイル同士かディレクトリ同士で比較してください（混在は不可）").into());
            return;
        }
        match cian_core::diff::diff_files(&a.path, &b.path) {
            Ok(result) => {
                // Identical files get a clear notice rather than a diff of
                // nothing — the same feedback the folder compare now gives.
                if !result.rows.iter().any(|r| r.is_difference()) {
                    self.open_popup(Popup::Notice {
                        lines: vec![
                            tr(self.lang, "The two files are identical", "2つのファイルは同一です").to_string(),
                            String::new(),
                            format!("{}  ↔  {}", a.name, b.name),
                        ],
                    });
                    return;
                }
                let folded = cian_core::diff::fold(&result.rows, cian_core::diff::CONTEXT);
                self.open_popup(Popup::Diff {
                    left: a.name.clone(),
                    right: b.name.clone(),
                    left_path: a.path.clone(),
                    right_path: b.path.clone(),
                    encoding: cian_core::viewer::TextEncoding::Utf8,
                    result,
                    folded,
                    // Folded to begin with: the differences are what was asked
                    // for, and on two near-identical files the unfolded view
                    // opens on a screen of agreement.
                    fold: true,
                    scroll: 0,
                    find: None,
                    find_input: None,
                });
            }
            Err(e) => self.message = Some(format!("cannot compare: {}", e)),
        }
    }

    /// Compare two directory trees on a worker thread, showing the differing
    /// paths when it finishes. Esc cancels a long walk.
    pub(crate) fn start_dir_compare(&mut self, left: PathBuf, right: PathBuf, ln: String, rn: String) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_cancel = Arc::clone(&cancel);
        let (l, r) = (left.clone(), right.clone());
        std::thread::spawn(move || {
            // Rate-limit the ticks: the core already reports every 16 entries,
            // but a huge tree still produces plenty; forward at most ~30/s.
            let mut last = Instant::now() - Duration::from_secs(1);
            let mut on_progress = |p: &cian_core::progress::Progress| {
                if last.elapsed() >= Duration::from_millis(33) {
                    last = Instant::now();
                    let _ = tx.send(DiffMsg::Tick(p.clone()));
                }
            };
            let diff = cian_core::dirdiff::compare(&l, &r, &worker_cancel, &mut on_progress);
            let _ = tx.send(DiffMsg::Done(diff));
        });
        self.diff_job = Some(DiffJob {
            rx,
            cancel,
            left_root: left,
            right_root: right,
            left: ln,
            right: rn,
            latest: cian_core::progress::Progress::default(),
            label: "comparing folders",
            started: Instant::now(),
        });
    }

    /// Drain progress and install the result when the worker finishes.
    pub(crate) fn poll_diff_job(&mut self) -> bool {
        let Some(job) = &mut self.diff_job else { return false };
        let mut done = None;
        let mut changed = false;
        loop {
            match job.rx.try_recv() {
                Ok(DiffMsg::Tick(p)) => {
                    job.latest = p;
                    changed = true;
                }
                Ok(DiffMsg::Done(d)) => {
                    done = Some(d);
                    changed = true;
                    break;
                }
                Err(_) => break,
            }
        }
        let Some(diff) = done else { return changed };
        let job = self.diff_job.take().unwrap();
        if diff.cancelled {
            self.message = Some(tr(self.lang, "comparison cancelled", "比較を中止しました").into());
            return true;
        }
        if diff.is_identical() {
            // A clear notice, not just a status line — the compare felt
            // unresponsive when identical folders only whispered a message.
            self.open_popup(Popup::Notice {
                lines: vec![
                    tr(self.lang, "The two folders are identical", "2つのディレクトリは同一です").to_string(),
                    String::new(),
                    format!("{}  ↔  {}", job.left, job.right),
                ],
            });
            return true;
        }
        self.open_popup(Popup::DirCompare {
            left: job.left,
            right: job.right,
            left_root: job.left_root,
            right_root: job.right_root,
            entries: diff.entries,
            cursor: 0,
            scroll: 0,
            truncated: diff.truncated,
        });
        true
    }

    /// Jump both panes to the highlighted diff entry (whichever side has it),
    /// putting the cursor on it, and close the comparison.
    pub(crate) fn dir_compare_goto(&mut self) {
        let Popup::DirCompare { entries, cursor, left_root, right_root, .. } = &self.popup else {
            return;
        };
        let Some(e) = entries.get(*cursor) else { return };
        use cian_core::dirdiff::Status;
        let rel = e.rel.clone();
        let (status, lr, rr) = (e.status, left_root.clone(), right_root.clone());
        self.popup = Popup::None;
        let go = |pane: &mut PaneTabs, root: &Path, rel: &Path| {
            let full = root.join(rel);
            let dir = if full.is_dir() { full.clone() } else { full.parent().map(|p| p.to_path_buf()).unwrap_or(full.clone()) };
            let p = pane.active_mut();
            if p.jump_to(dir).is_ok() {
                if let Some(i) = p.entries.iter().position(|x| x.path == full) {
                    p.cursor = i;
                }
            }
        };
        if status != Status::OnlyRight {
            go(&mut self.left, &lr, &rel);
        }
        if status != Status::OnlyLeft {
            go(&mut self.right, &rr, &rel);
        }
        self.message = Some(format!("→ {}", rel.display()));
    }

    /// The open diff (file or folder) rendered as plain text, for copy/save.
    pub(crate) fn diff_as_text(&self) -> Option<String> {
        use cian_core::diff::Row;
        match &self.popup {
            Popup::Diff { left, right, result, .. } => {
                let mut out = format!("--- {}\n+++ {}\n", left, right);
                if result.binary {
                    out.push_str(if result.identical {
                        "(binary files, identical)\n"
                    } else {
                        "(binary files differ)\n"
                    });
                    return Some(out);
                }
                for r in &result.rows {
                    match r {
                        Row::Same { left: l, .. } => out.push_str(&format!("  {}\n", l.text)),
                        Row::Removed { left: l } => out.push_str(&format!("- {}\n", l.text)),
                        Row::Added { right: rr } => out.push_str(&format!("+ {}\n", rr.text)),
                        Row::Changed { left: l, right: rr } => {
                            out.push_str(&format!("- {}\n+ {}\n", l.text, rr.text));
                        }
                        Row::Skipped { lines } => out.push_str(&format!("  … {} identical lines\n", lines)),
                    }
                }
                Some(out)
            }
            Popup::DirCompare { left, right, entries, .. } => {
                use cian_core::dirdiff::Status;
                let mut out = format!("# compare  {}  ↔  {}\n", left, right);
                for e in entries {
                    let mark = match e.status {
                        Status::OnlyLeft => "-",
                        Status::OnlyRight => "+",
                        Status::Differ => "~",
                    };
                    out.push_str(&format!("{} {}\n", mark, e.rel.display()));
                }
                Some(out)
            }
            _ => None,
        }
    }

    /// The comparison on screen as a WinMerge-style HTML report (side by side),
    /// or `None` if no comparison is open.
    pub(crate) fn diff_as_html(&self) -> Option<String> {
        match &self.popup {
            Popup::Diff { left, right, result, .. } => {
                Some(cian_core::diff::to_html(result, left, right))
            }
            Popup::DirCompare { left, right, entries, truncated, .. } => {
                Some(cian_core::dirdiff::to_html(entries, left, right, *truncated))
            }
            _ => None,
        }
    }

    /// The comparison on screen as a side-by-side Markdown table, or `None`.
    pub(crate) fn diff_as_markdown(&self) -> Option<String> {
        match &self.popup {
            Popup::Diff { left, right, result, .. } => {
                Some(cian_core::diff::to_markdown(result, left, right))
            }
            Popup::DirCompare { left, right, entries, truncated, .. } => {
                Some(cian_core::dirdiff::to_markdown(entries, left, right, *truncated))
            }
            _ => None,
        }
    }

    /// Move the diff view to the next/previous row whose text matches the
    /// active search (case-insensitive). `from_here` includes the current row
    /// (used right after confirming a search).
    pub(crate) fn diff_search_jump(&mut self, forward: bool, from_here: bool) {
        use cian_core::diff::Row;
        let Popup::Diff { result, folded, fold, scroll, find, .. } = &mut self.popup else { return };
        let Some(q) = find.as_ref().map(|s| s.to_lowercase()) else { return };
        let rows: &[Row] = if *fold { folded } else { &result.rows };
        let hit = |r: &Row| -> bool {
            let txt = |o: Option<&cian_core::diff::Line>| o.map(|l| l.text.to_lowercase()).unwrap_or_default();
            match r {
                Row::Same { left, right } => txt(Some(left)).contains(&q) || txt(Some(right)).contains(&q),
                Row::Changed { left, right } => txt(Some(left)).contains(&q) || txt(Some(right)).contains(&q),
                Row::Removed { left } => txt(Some(left)).contains(&q),
                Row::Added { right } => txt(Some(right)).contains(&q),
                Row::Skipped { .. } => false,
            }
        };
        let n = rows.len();
        if n == 0 {
            return;
        }
        let found = if forward {
            let start = if from_here { *scroll } else { *scroll + 1 };
            (start..n).find(|&i| hit(&rows[i]))
        } else {
            (0..*scroll).rev().find(|&i| hit(&rows[i]))
        };
        match found {
            Some(i) => *scroll = i,
            None => self.message = Some(tr(self.lang, "no more matches", "一致なし").into()),
        }
    }

    /// Copy the diff/compare result to the clipboard.
    pub(crate) fn copy_diff(&mut self) {
        let Some(text) = self.diff_as_text() else { return };
        match self.clipboard.as_mut() {
            Some(cb) => {
                let _ = cb.set_text(text);
                self.message = Some(tr(self.lang, "◂ diff copied", "◂ 差分をコピーしました").into());
            }
            None => self.message = Some(tr(self.lang, "clipboard unavailable", "クリップボードを利用できません").into()),
        }
    }

    /// Prompt for a filename and save the diff/compare result into the active
    /// pane's directory.
    pub(crate) fn start_diff_save_as(&mut self) {
        let Some(text) = self.diff_as_text() else { return };
        let html = self.diff_as_html().unwrap_or_default();
        let md = self.diff_as_markdown().unwrap_or_default();
        self.open_popup(text_input(
            tr(self.lang, "save comparison as", "比較結果を保存"),
            tr(self.lang,
                ".html / .md = side by side; .txt = plain  (in the active pane):",
                ".html / .md = 左右並び、.txt = プレーン（アクティブペインに保存）:"),
            "diff.html".to_string(),
            InputKind::DiffSaveAs { text, html, md },
        ));
    }

    /// `>` / `<` in the folder compare: copy the highlighted entry across to the
    /// other tree at the same relative path. `to_right` copies left→right.
    /// A file or a whole directory; creates missing parent folders; confirms
    /// before overwriting an existing target.
    pub(crate) fn dir_compare_copy(&mut self, to_right: bool) {
        let Popup::DirCompare { entries, cursor, left_root, right_root, .. } = &self.popup else {
            return;
        };
        use cian_core::dirdiff::Status;
        let Some(e) = entries.get(*cursor) else { return };
        // The source must exist on the side we copy *from*.
        let missing_source = (to_right && e.status == Status::OnlyRight)
            || (!to_right && e.status == Status::OnlyLeft);
        if missing_source {
            self.message = Some(
                tr(self.lang, "nothing to copy on that side", "その側にコピー元がありません").into(),
            );
            return;
        }
        let (from_root, into_root) =
            if to_right { (left_root, right_root) } else { (right_root, left_root) };
        let src = from_root.join(&e.rel);
        let dst = into_root.join(&e.rel);
        let is_dir = e.is_dir;
        self.begin_diff_copy(src, dst, is_dir);
    }

    /// `]` / `[` in the folder compare: one-way sync the whole tree. `to_right`
    /// makes the right match the left — copying every path the left has that the
    /// right lacks or differs on. It never deletes, so anything that exists only
    /// on the destination is reported but left in place. Confirms first, listing
    /// the counts; the copy then runs on the worker (progress + done bell).
    pub(crate) fn dir_compare_sync(&mut self, to_right: bool) {
        use cian_core::dirdiff::Status;
        let Popup::DirCompare { entries, left_root, right_root, .. } = &self.popup else {
            return;
        };
        let (from_root, into_root) =
            if to_right { (left_root, right_root) } else { (right_root, left_root) };
        // The status that means "source-only" for this direction.
        let source_only = if to_right { Status::OnlyLeft } else { Status::OnlyRight };
        let dest_only = if to_right { Status::OnlyRight } else { Status::OnlyLeft };
        let mut ops = Vec::new();
        let mut extra = 0usize;
        for e in entries {
            match e.status {
                s if s == source_only => {}
                Status::Differ => {}
                s if s == dest_only => {
                    extra += 1;
                    continue;
                }
                _ => continue,
            }
            let src = from_root.join(&e.rel);
            // copy_one places the source under a destination *directory* at the
            // same relative parent, matching the per-entry copy.
            let dst_parent = into_root.join(&e.rel);
            let Some(dest_dir) = dst_parent.parent().map(Path::to_path_buf) else { continue };
            ops.push((src, dest_dir, e.is_dir));
        }
        if ops.is_empty() {
            self.message = Some(tr(
                self.lang,
                "already in sync in that direction",
                "その向きでは既に同期済みです",
            ).into());
            return;
        }
        let back = std::mem::replace(&mut self.popup, Popup::None);
        self.open_popup(Popup::ConfirmDirSync { to_right, ops, extra, back: Box::new(back) });
    }

    /// Confirmed folder sync: run every queued copy on the worker thread.
    pub(crate) fn confirm_dir_sync(&mut self) {
        let Popup::ConfirmDirSync { ops, .. } =
            std::mem::replace(&mut self.popup, Popup::None)
        else {
            return;
        };
        let total = ops.len();
        self.start_op("syncing", move |ctl| {
            let mut report = OpReport::default();
            let mut p = cian_core::progress::Progress {
                files_total: total,
                ..Default::default()
            };
            // Borrowed, not consumed: op closures are FnMut so a retryable
            // transfer can run twice (this one has no retries, but the bound
            // is shared).
            for (src, dest_dir, _is_dir) in ops.iter() {
                if ctl.cancel.load(Ordering::Relaxed) {
                    break;
                }
                p.current = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                (ctl.on_progress)(&p);
                if let Err(e) = std::fs::create_dir_all(dest_dir) {
                    report.note_error(format!("{}: {}", dest_dir.display(), e));
                } else {
                    match cian_core::ops::copy_one(src, dest_dir, Conflict::Overwrite) {
                        Ok(_) => report.ok += 1,
                        Err(e) => report.note_error(format!("{}: {}", src.display(), e)),
                    }
                }
                p.files_done += 1;
            }
            report
        });
    }

    /// Cancelled folder sync: put the comparison back.
    pub(crate) fn cancel_dir_sync(&mut self) {
        if let Popup::ConfirmDirSync { back, .. } =
            std::mem::replace(&mut self.popup, Popup::None)
        {
            self.popup = *back;
        }
    }

    /// `>` / `<` in the file diff: copy one side's file over the other. Always an
    /// overwrite (both files exist), so it always confirms.
    pub(crate) fn diff_copy(&mut self, to_right: bool) {
        let Popup::Diff { left_path, right_path, .. } = &self.popup else { return };
        let (src, dst) = if to_right {
            (left_path.clone(), right_path.clone())
        } else {
            (right_path.clone(), left_path.clone())
        };
        self.begin_diff_copy(src, dst, false);
    }

    /// Shared entry point: copy `src` onto `dst`. Copies straight away when the
    /// destination does not exist (nothing is lost); otherwise stashes the
    /// comparison popup and asks before overwriting.
    fn begin_diff_copy(&mut self, src: PathBuf, dst: PathBuf, is_dir: bool) {
        if dst.exists() {
            let back = std::mem::replace(&mut self.popup, Popup::None);
            self.open_popup(Popup::ConfirmDiffCopy { src, dst, is_dir, back: Box::new(back) });
        } else {
            self.perform_diff_copy(&src, &dst, is_dir);
            self.after_diff_copy(&dst);
        }
    }

    /// Confirmed overwrite: restore the comparison, do the copy, refresh it.
    pub(crate) fn confirm_diff_copy(&mut self) {
        let Popup::ConfirmDiffCopy { src, dst, is_dir, back } =
            std::mem::replace(&mut self.popup, Popup::None)
        else {
            return;
        };
        self.popup = *back;
        self.perform_diff_copy(&src, &dst, is_dir);
        self.after_diff_copy(&dst);
    }

    /// Cancelled overwrite: just put the comparison back.
    pub(crate) fn cancel_diff_copy(&mut self) {
        if let Popup::ConfirmDiffCopy { back, .. } =
            std::mem::replace(&mut self.popup, Popup::None)
        {
            self.popup = *back;
        }
    }

    /// Do the copy, reporting success/failure to the status line. Uses
    /// `ops::copy_one` (a file or a whole directory), creating the destination's
    /// parent directories first so a deep only-on-one-side path lands.
    fn perform_diff_copy(&mut self, src: &Path, dst: &Path, _is_dir: bool) {
        let Some(dest_dir) = dst.parent() else {
            self.message = Some(tr(self.lang, "bad destination", "宛先が不正です").into());
            return;
        };
        if let Err(e) = std::fs::create_dir_all(dest_dir) {
            self.message = Some(format!("copy failed: {}", e));
            return;
        }
        match cian_core::ops::copy_one(src, dest_dir, Conflict::Overwrite) {
            Ok(_) => {
                let name = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.message = Some(format!("◂ copied {} → {}", name, dst.display()));
            }
            Err(e) => self.message = Some(format!("copy failed: {}", e)),
        }
    }

    /// After a successful copy-across, refresh the comparison so it reflects the
    /// now-reconciled state: the folder compare drops the entry (both sides
    /// match now); the file diff closes (the two files are identical). Both
    /// panes are reloaded in case the copy landed in a visible directory.
    fn after_diff_copy(&mut self, _dst: &Path) {
        match &mut self.popup {
            Popup::DirCompare { entries, cursor, .. } => {
                if *cursor < entries.len() {
                    entries.remove(*cursor);
                }
                if !entries.is_empty() {
                    *cursor = (*cursor).min(entries.len() - 1);
                } else {
                    self.popup = Popup::None;
                    self.message = Some(
                        tr(self.lang, "folders reconciled", "ディレクトリを同期しました").into(),
                    );
                }
            }
            Popup::Diff { .. } => {
                self.popup = Popup::None;
            }
            _ => {}
        }
        self.invalidate_git();
        // Reload both listings — the copy may have landed in either pane.
        let _ = self.left.active_mut().reload();
        let _ = self.right.active_mut().reload();
    }

    /// Stash the current file diff and open the encoding picker for it.
    pub(crate) fn open_diff_encoding_picker(&mut self) {
        if !matches!(self.popup, Popup::Diff { .. }) {
            return;
        }
        let cur = if let Popup::Diff { encoding, .. } = &self.popup {
            cian_core::viewer::TextEncoding::ALL.iter().position(|e| e == encoding).unwrap_or(0)
        } else {
            0
        };
        let diff = std::mem::replace(&mut self.popup, Popup::None);
        self.open_popup(Popup::EncodingPicker { cursor: cur, target: EncTarget::Diff(Box::new(diff)) });
    }

    /// Pull members out of the open archive into the opposite pane.
    pub(crate) fn extract_from_archive(&mut self, all: bool) {
        let Popup::Archive { path, members, cursor, .. } = &self.popup else { return };
        let (path, chosen) = (
            path.clone(),
            if all {
                Vec::new()
            } else {
                match members.get(*cursor) {
                    Some(m) => vec![m.name.clone()],
                    None => return,
                }
            },
        );
        let Some(dest) = self.opposite_pane_cwd() else {
            self.message = Some(tr(self.lang, "no destination pane", "宛先のペインがありません").into());
            return;
        };
        self.popup = Popup::None;
        self.remember_dest(&dest);
        self.start_extract(path, chosen, dest);
    }

    /// `:unzip` / `:extract` (and the right-click menu): extract the archive
    /// under the cursor into a fresh sub-folder of the active pane, named after
    /// the archive. Works for zip and tar/tar.gz.
    pub(crate) fn extract_selected(&mut self) {
        let Some(p) = self.active_pane() else { return };
        let Some(e) = p.selected().filter(|e| !e.is_parent) else {
            self.message = Some(tr(self.lang, "select an archive to extract", "解凍するアーカイブを選択してください").into());
            return;
        };
        if e.is_dir || !cian_core::archive::is_archive(&e.path) {
            self.message = Some(format!("{}: {}", tr(self.lang, "not an archive", "アーカイブではありません"), e.name));
            return;
        }
        let archive = e.path.clone();
        let dest = unique_dir(&p.cwd, &archive_stem(&e.name));
        self.start_extract(archive, Vec::new(), dest);
    }

    /// Extract `members` (empty = all) of `archive` into `dest`, asking for a
    /// password first when the zip is encrypted.
    pub(crate) fn start_extract(&mut self, archive: PathBuf, members: Vec<String>, dest: PathBuf) {
        self.start_extract_stripped(archive, members, dest, String::new());
    }

    /// Like [`Self::start_extract`], with a member-path prefix stripped on
    /// write — the copy-out path for browsing inside an archive, where "copy
    /// c/ to the other pane" must not rebuild the archive's whole tree.
    pub(crate) fn start_extract_stripped(
        &mut self,
        archive: PathBuf,
        members: Vec<String>,
        dest: PathBuf,
        strip: String,
    ) {
        if cian_core::archive::zip_needs_password(&archive) {
            self.open_popup(text_input(
                "encrypted zip",
                "password:",
                String::new(),
                InputKind::ExtractPassword { archive, members, dest, strip },
            ));
        } else {
            self.run_extract(archive, members, dest, None, strip);
        }
    }

    /// Kick off the extraction on a worker (after any password was collected).
    pub(crate) fn run_extract(
        &mut self,
        archive: PathBuf,
        members: Vec<String>,
        dest: PathBuf,
        password: Option<String>,
        strip: String,
    ) {
        self.start_op("extracting", move |ctl| {
            let _ = std::fs::create_dir_all(&dest);
            cian_core::archive::extract(&archive, &members, &dest, password.as_deref(), &strip, ctl)
        });
    }

    // ------- Hidden files, attributes, checksums -------
    pub(crate) fn toggle_hidden(&mut self) {
        let Some(p) = self.active_pane_mut() else { return };
        let show = !p.show_hidden;
        p.set_show_hidden(show);
        self.message =
            Some(if show { "showing dotfiles".into() } else { "hiding dotfiles".to_string() });
    }

    pub(crate) fn show_attributes(&mut self) {
        let paths = self.target_paths();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        }
        self.open_popup(Popup::Notice { lines: self.attributes_lines(&paths, 40) });
    }

    /// Build the Attributes listing (permissions, size, owner) for `paths`,
    /// capped at `limit` rows. Shared by the Attributes menu/`:attr` and `:ls`.
    pub(crate) fn attributes_lines(&self, paths: &[PathBuf], limit: usize) -> Vec<String> {
        let ja = self.lang == Lang::Ja;
        let mut lines = Vec::new();
        for path in paths.iter().take(limit) {
            let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            match cian_core::attrs::read_attrs(path) {
                Ok(a) => {
                    // A folder is labelled as such; a file shows its byte size,
                    // right-aligned so the sizes form a readable column.
                    let size = if a.is_dir {
                        format!("{:>10}", tr(self.lang, "<dir>", "<ディレクトリ>"))
                    } else {
                        format!("{:>10}", cian_core::human_size(a.size.unwrap_or(0)))
                    };
                    let owner = a.owner.as_ref().map(|o| format!("  owner {}", o)).unwrap_or_default();
                    // What the thing *is*, next to what may be done to it.
                    // `:file` used to answer this on its own, one letter away
                    // from `:files` and doing something else entirely; folded
                    // in here, one command answers "tell me about this".
                    let kind = if a.is_dir {
                        String::new()
                    } else {
                        cian_core::inspect::classify(path)
                            .map(|d| format!("  {d}"))
                            .unwrap_or_default()
                    };
                    lines.push(format!(
                        "{} {}  {}{}{}",
                        fit(&name, 28),
                        a.describe(),
                        size,
                        owner,
                        kind
                    ));
                }
                Err(e) => lines.push(format!("{} {}", fit(&name, 28), e)),
            }
        }
        if paths.len() > limit {
            lines.push(if ja {
                format!("... 他 {} 件", paths.len() - limit)
            } else {
                format!("... and {} more", paths.len() - limit)
            });
        }
        lines.push(String::new());
        lines.push(tr(self.lang,
            "change with  :chmod 644   or  :readonly on|off",
            "変更:  :chmod 644   または  :readonly on|off").to_string());
        lines
    }

    /// Checksum the selection on a worker thread — the files worth hashing are
    /// the big ones, which is exactly when doing it inline would freeze.
    pub(crate) fn start_hash(&mut self, kind: cian_core::attrs::HashKind) {
        let paths: Vec<PathBuf> = self
            .active_pane()
            .map(|p| p.target_paths())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.is_file())
            .collect();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "no files selected", "ファイルが選択されていません").into());
            return;
        }
        self.start_op("hashing", move |ctl| {
            let mut report = OpReport::default();
            let total = paths.len();
            for (i, path) in paths.iter().enumerate() {
                if ctl.cancel.load(Ordering::Relaxed) {
                    break;
                }
                let p = cian_core::progress::Progress {
                    files_done: i,
                    files_total: total,
                    current: path.display().to_string(),
                    ..Default::default()
                };
                (ctl.on_progress)(&p);
                match cian_core::attrs::hash_file(path, kind, ctl.cancel) {
                    Ok(Some(sum)) => {
                        let name = path
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        // Carried on the report so the result survives to be
                        // shown; there is no other channel back.
                        report.note_error(format!("{}  {}  {}", kind.label(), sum, name));
                    }
                    Ok(None) => break,
                    Err(e) => report.note_error(format!("{}: {}", path.display(), e)),
                }
            }
            report
        });
    }

    pub(crate) fn set_attr_command(&mut self, arg: &str) {
        let paths = self.target_paths();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        }
        // Every path is attempted, and both halves are reported. It used to
        // stop at the first failure and say only "chmod failed" — discarding
        // the count of what it had already changed, so a partial application
        // read as "nothing happened" and the files it *had* touched went
        // unmentioned.
        let (ok, err) = self.apply_to_each(&paths, |p| cian_core::attrs::set_mode(p, arg));
        self.reload_both();
        self.message = Some(self.each_report(&format!("chmod {arg}"), ok, paths.len(), err));
    }

    /// Run `f` over every path, counting what worked and keeping the first
    /// complaint. Nothing stops early: a sweep that gives up halfway leaves the
    /// selection in two states and tells you about neither.
    fn apply_to_each<F>(&self, paths: &[PathBuf], mut f: F) -> (usize, Option<String>)
    where
        F: FnMut(&PathBuf) -> anyhow::Result<()>,
    {
        let mut ok = 0;
        let mut err = None;
        for path in paths {
            match f(path) {
                Ok(()) => ok += 1,
                Err(e) => {
                    if err.is_none() {
                        err = Some(format!("{}: {}", path.display(), e));
                    }
                }
            }
        }
        (ok, err)
    }

    /// What a sweep over several files did, in one line: how many took, how
    /// many did not, and why the first one did not.
    fn each_report(&self, what: &str, ok: usize, total: usize, err: Option<String>) -> String {
        let failed = total.saturating_sub(ok);
        let ja = self.lang == crate::theme::Lang::Ja;
        match (failed, err) {
            (0, _) => {
                if ja {
                    format!("{what}: {ok} 件")
                } else {
                    format!("{what} on {ok} item(s)")
                }
            }
            (n, Some(e)) if ok == 0 => {
                if ja {
                    format!("{what}: {n} 件すべて失敗 — {e}")
                } else {
                    format!("{what}: all {n} failed — {e}")
                }
            }
            (n, Some(e)) => {
                if ja {
                    format!("{what}: {ok} 件成功、{n} 件失敗 — {e}")
                } else {
                    format!("{what} on {ok} item(s) — {n} failed: {e}")
                }
            }
            (n, None) => {
                if ja {
                    format!("{what}: {ok} 件成功、{n} 件失敗")
                } else {
                    format!("{what} on {ok} item(s) — {n} failed")
                }
            }
        }
    }

    pub(crate) fn set_readonly_command(&mut self, on: bool) {
        let paths = self.target_paths();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        }
        // It threw every error away and always reported success — so a run
        // where nothing at all could be changed said "on 0 item(s)", which is
        // true and reads as though it worked.
        let (ok, err) = self.apply_to_each(&paths, |p| cian_core::attrs::set_readonly(p, on));
        self.reload_both();
        let what = if self.lang == crate::theme::Lang::Ja {
            format!("読み取り専用を{}", if on { "設定" } else { "解除" })
        } else {
            format!("read-only {}", if on { "set" } else { "cleared" })
        };
        self.message = Some(self.each_report(&what, ok, paths.len(), err));
    }

    // ------- Recursive search -------
    pub(crate) fn start_find_prompt(&mut self) {
        self.open_popup(text_input(
                "find (recursive)",
                "name contains   (Ctrl+V paste, Ctrl+A select all):",
                String::new(),
                InputKind::FindRecursive,
            ));
    }

    /// The synced libraries from `init.lua`, as the core wants them.
    fn sync_maps(&self) -> Vec<cian_core::office::SyncMap> {
        self.config
            .sharepoint
            .iter()
            .map(|(l, u)| cian_core::office::SyncMap {
                local: crate::expand_path(l),
                url: u.clone(),
            })
            .collect()
    }

    /// The file under the cursor, its document kind, and its address in the
    /// cloud — everything the two Office commands need, or a reason why not.
    fn office_target(&self) -> Result<(std::path::PathBuf, cian_core::office::Doc, String), String> {
        let path = self
            .active_pane()
            .and_then(|p| p.selected())
            .filter(|e| !e.is_dir && !e.is_parent)
            .map(|e| e.path.clone())
            .ok_or_else(|| tr(self.lang, "select a document first", "先にドキュメントを選んでください").to_string())?;
        let doc = cian_core::office::classify(&path).ok_or_else(|| {
            tr(self.lang, "not an Office document", "Office のドキュメントではありません").to_string()
        })?;
        let maps = self.sync_maps();
        if maps.is_empty() {
            return Err(tr(self.lang, "no synced libraries configured. see cian.sharepoint{} in init.lua", "同期ライブラリが未設定です。init.lua の cian.sharepoint{} を参照").to_string());
        }
        let url = cian_core::office::cloud_url(&path, &maps).ok_or_else(|| {
            tr(self.lang,
               "that file is not inside a configured library",
               "そのファイルは設定済みライブラリの中にありません").to_string()
        })?;
        Ok((path, doc, url))
    }

    /// Whether the two Office entries would do anything here.
    pub(crate) fn office_target_ok(&self) -> bool {
        self.office_target().is_ok()
    }

    /// `:office` — hand the *cloud* copy to the desktop application.
    ///
    /// Not the synced local file: opening that gets a copy to reconcile later,
    /// while the `ofe|u|` URI is what makes check-out and co-authoring work.
    pub(crate) fn open_in_office(&mut self) {
        let (_, doc, url) = match self.office_target() {
            Ok(t) => t,
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        let Some(uri) = cian_core::office::app_uri(doc, &url) else {
            self.message = Some(tr(self.lang,
                "a PDF has no Office application to hand it to",
                "PDF に渡せる Office アプリはありません").into());
            return;
        };
        match crate::os_open(std::path::Path::new(&uri)) {
            Ok(()) => self.message = Some(format!("{}: {}", doc.label(), url)),
            Err(e) => self.message = Some(format!("could not open: {e}")),
        }
    }

    /// `:officelink` — write a `.url` shortcut to the cloud copy.
    ///
    /// The thing to paste into a mail or a ticket: it points at the library
    /// rather than at one machine's sync folder, so it still works for whoever
    /// receives it.
    pub(crate) fn write_office_link(&mut self) {
        let (path, _, url) = match self.office_target() {
            Ok(t) => t,
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let dest = self
            .opposite_pane_cwd()
            .or_else(|| path.parent().map(|p| p.to_path_buf()))
            .unwrap_or_default()
            .join(format!("{stem}.url"));
        match std::fs::write(&dest, cian_core::office::url_shortcut(&url)) {
            Ok(()) => {
                self.message = Some(format!("{}", dest.display()));
                self.reload_both();
            }
            Err(e) => self.message = Some(format!("could not write the shortcut: {e}")),
        }
    }

    /// Mark everything here — or, in the viewer, select the whole file.
    ///
    /// One action for both, because "select all" is one idea and which of the
    /// two it means is simply which one is in front of you.
    pub(crate) fn mark_all(&mut self) {
        if matches!(self.popup, Popup::Viewer { .. }) {
            let mut n = 0usize;
            if let Popup::Viewer { view, visual, anchor, line, col, goal, .. } = &mut self.popup {
                n = view.lines.len();
                if n == 0 {
                    return;
                }
                *anchor = (0, 0);
                *line = n - 1;
                *col = 0;
                *goal = 0;
                *visual = Some(ViewVisual::Line);
            }
            self.message = Some(if self.lang == Lang::Ja {
                format!("{n} 行すべて選択（y でコピー、Esc で解除）")
            } else {
                format!("all {n} line(s) selected — y copies, Esc clears")
            });
            return;
        }
        let mut n = 0usize;
        if let Some(p) = self.active_pane_mut() {
            // `..` is not a file; marking it would put the parent directory
            // into every operation the marks are for.
            for e in p.entries.iter().filter(|e| !e.is_parent) {
                p.marks.insert(e.path.clone());
                n += 1;
            }
        }
        self.message = Some(if self.lang == Lang::Ja {
            format!("{n} 件マーク（Space で個別解除、Esc で全解除）")
        } else {
            format!("{n} marked — Space unmarks one, Esc clears them")
        });
    }

    pub(crate) fn start_grep_prompt(&mut self) {
        self.open_popup(text_input(
                "grep (recursive)",
                "text inside files   (Ctrl+V paste, Ctrl+A select all):",
                String::new(),
                InputKind::GrepRecursive,
            ));
    }

    /// `r` in the grep results: ask what the matched text should become.
    ///
    /// The pattern is not asked for again — it is the one that produced the
    /// list on screen, and re-typing it is the easiest way to replace
    /// something other than what you are looking at.
    pub(crate) fn start_grep_replace(&mut self) -> Result<()> {
        let Some(job) = self.find_job.as_ref() else { return Ok(()) };
        if job.mode != cian_core::search::Mode::Content {
            self.message =
                Some(tr(self.lang, "replace works on a grep, not a name search", "置換は grep 結果に対して行う（名前検索では不可）").into());
            return Ok(());
        }
        let pattern = job.query.clone();
        // One entry per file, in the order the grep reached them: a file with
        // twenty matching lines is still one file to open and rewrite.
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Popup::FindResults { hits, .. } = &self.popup {
            for h in hits {
                if !h.is_dir && !paths.contains(&h.path) {
                    paths.push(h.path.clone());
                }
            }
        }
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing to replace in", "置換対象がありません").into());
            return Ok(());
        }
        self.stop_find();
        self.open_popup(text_input(
            format!("replace in {} file(s)", paths.len()),
            format!("replace {pattern:?} with   (blank = delete it):"),
            String::new(),
            InputKind::GrepReplaceWith { paths, pattern },
        ));
        Ok(())
    }

    /// Build the preview: read every matched file and work out the exact lines
    /// that would change. Still nothing written.
    pub(crate) fn build_grep_replace(&mut self, paths: Vec<PathBuf>, pattern: &str, with: &str) {
        // The grep already decided what `pattern` means — a bare needle is a
        // case-insensitive substring, `/re/` a regex — so reuse its matcher
        // rather than re-parsing through `s/old/new/` and risking a different
        // answer to the one the user is looking at.
        let matcher = match cian_core::search::Matcher::parse(pattern) {
            Ok(m) => m,
            Err(e) => {
                self.message = Some(format!("bad pattern: {e}"));
                return;
            }
        };
        let sub = cian_core::substitute::Substitution {
            matcher,
            replacement: cian_core::substitute::unescape(with),
            confirm: false,
            // Every occurrence: a grep hit is a line, and replacing only the
            // first match on a line that has three is never what was meant.
            global: true,
        };
        let (changes, skipped) = cian_core::grepedit::plan(&paths, &sub);
        if changes.is_empty() {
            self.popup = Popup::None;
            self.message = Some(match skipped.len() {
                0 => format!("nothing to change: no line becomes different from {pattern:?}"),
                n => format!("nothing to change ({n} file(s) unreadable)"),
            });
            return;
        }
        let what = format!("{pattern} → {}", if with.is_empty() { "(nothing)" } else { with });
        self.open_popup(Popup::GrepReplace(Box::new(crate::ReplacePlan {
            changes,
            skipped,
            cursor: 0,
            scroll: 0,
            what,
        })));
    }

    /// Enter on the preview: write the checked lines and report what happened.
    pub(crate) fn commit_grep_replace(&mut self) -> Result<()> {
        let Popup::GrepReplace(plan) = std::mem::replace(&mut self.popup, Popup::None) else {
            return Ok(());
        };
        if !plan.changes.iter().any(|c| c.picked) {
            self.message = Some(tr(self.lang, "nothing checked. nothing was written", "チェックが無いので何も書いていません").into());
            return Ok(());
        }
        let report = cian_core::grepedit::apply(&plan.changes);
        let mut msg = format!("replaced {} line(s) in {} file(s)", report.lines, report.files);
        if report.stale > 0 {
            msg.push_str(&format!(
                " — {} line(s) skipped: the file changed since the preview",
                report.stale
            ));
        }
        if let Some(first) = report.errors.first() {
            msg.push_str(&format!(" — {first}"));
            if report.errors.len() > 1 {
                msg.push_str(&format!(" (+{} more)", report.errors.len() - 1));
            }
        }
        self.message = Some(msg);
        self.reload_both();
        Ok(())
    }

    /// Walk the tree below the focused pane on a worker thread.
    pub(crate) fn start_find(&mut self, needle: &str, mode: cian_core::search::Mode) {
        self.begin_find(needle, mode, None);
    }

    /// Flatten the focused pane's whole subtree into that pane — "branch view".
    /// It reuses the search walker (an empty name query matches everything) and
    /// routes the result into the pane instead of a popup. Pressing it again on
    /// a flat pane leaves the view (handled by the caller).
    pub(crate) fn start_branch_view(&mut self) {
        let label = tr(self.lang, "branch", "ブランチ").to_string();
        self.begin_find("", cian_core::search::Mode::Name, Some(label));
    }

    /// Shared worker spawn for [`start_find`] and [`start_branch_view`]. With
    /// `to_pane` set, the streamed hits accumulate in the (hidden) results popup
    /// and are panelized into the active pane when the walk completes.
    fn begin_find(&mut self, needle: &str, mode: cian_core::search::Mode, to_pane: Option<String>) {
        self.find_return = None; // a fresh search invalidates any stashed list
        let Some(root) = self.cwd() else { return };
        // `/re/` compiles to a regex; anything else is a literal substring. A
        // bad pattern stops here with its reason — searching for the wrong
        // thing silently would be worse.
        let mut query = match cian_core::search::Query::parse(needle, mode) {
            Ok(q) => q,
            Err(e) => {
                self.message = Some(e);
                return;
            }
        };
        query.include_hidden =
            self.active_pane().map(|p| p.show_hidden).unwrap_or(false);
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_cancel = Arc::clone(&cancel);
        let worker_root = root.clone();
        std::thread::spawn(move || {
            let mut on_hit = |h: cian_core::search::Hit| {
                let _ = tx.send(FindMsg::Hit(h));
            };
            let outcome =
                cian_core::search::search(&worker_root, &query, &worker_cancel, &mut on_hit);
            let _ = tx.send(FindMsg::Done(outcome));
        });
        self.find_job = Some(FindJob {
            rx,
            cancel,
            root_label: root
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string()),
            query: needle.to_string(),
            mode,
            done: None,
            to_pane,
        });
        self.open_popup(Popup::FindResults { hits: Vec::new(), cursor: 0, scroll: 0 });
    }

    /// Load a set of search hits into the active pane as a flat listing so the
    /// normal cursor, marks and file operations act on them. Shared by branch
    /// view (all files) and "panelize" from the grep results (`files_only`
    /// off — keep whatever matched, one row per path).
    pub(crate) fn panelize_active(
        &mut self,
        label: String,
        hits: &[cian_core::search::Hit],
        files_only: bool,
    ) {
        let mut seen = std::collections::HashSet::new();
        let mut entries = Vec::new();
        for h in hits {
            if files_only && h.is_dir {
                continue;
            }
            // One row per file: a content grep reports every matching line.
            if !seen.insert(h.path.clone()) {
                continue;
            }
            entries.push(cian_core::Entry::flat(&h.rel, h.path.clone(), h.is_dir));
        }
        if entries.is_empty() {
            self.message = Some(tr(self.lang, "nothing to show", "表示するものがありません").into());
            return;
        }
        let n = entries.len();
        if let Some(t) = self.active_file_tabs_mut() {
            t.active_mut().enter_flat(label, entries);
        }
        self.message = Some(format!("{} {}", n, tr(self.lang, "entries", "件")));
    }

    /// `b`: toggle branch view for the focused pane.
    pub(crate) fn toggle_branch_view(&mut self) {
        if self.active_pane().map(|p| p.is_flat()).unwrap_or(false) {
            if let Some(p) = self.active_pane_mut() {
                let _ = p.leave_flat();
            }
            return;
        }
        self.start_branch_view();
    }

    /// Collect whatever the search has produced. Returns true to repaint.
    pub(crate) fn poll_find_job(&mut self) -> bool {
        let Some(job) = &mut self.find_job else { return false };
        let mut changed = false;
        let mut batch = Vec::new();
        loop {
            match job.rx.try_recv() {
                Ok(FindMsg::Hit(h)) => batch.push(h),
                Ok(FindMsg::Done(o)) => {
                    job.done = Some(o);
                    changed = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if job.done.is_none() {
                        job.done = Some(cian_core::search::Outcome::Complete);
                        changed = true;
                    }
                    break;
                }
            }
        }
        if !batch.is_empty() {
            changed = true;
            if let Popup::FindResults { hits, .. } = &mut self.popup {
                hits.extend(batch);
            }
        }
        // A branch view routes its walk into the pane, not the popup: once the
        // walk finishes, panelize the accumulated hits and drop the popup.
        let finished_to_pane = self
            .find_job
            .as_ref()
            .map(|j| j.done.is_some() && j.to_pane.is_some())
            .unwrap_or(false);
        if finished_to_pane {
            let label = self.find_job.as_ref().and_then(|j| j.to_pane.clone()).unwrap_or_default();
            let hits = match std::mem::replace(&mut self.popup, Popup::None) {
                Popup::FindResults { hits, .. } => hits,
                other => {
                    self.popup = other;
                    Vec::new()
                }
            };
            self.stop_find();
            // Branch view lists files only — a flat listing of folders is not
            // what "flatten the tree" means.
            self.panelize_active(label, &hits, true);
            changed = true;
        }
        changed
    }

    /// Go to the highlighted result: into the directory, or onto the file.
    pub(crate) fn open_find_hit(&mut self) -> Result<()> {
        let Popup::FindResults { hits, cursor, .. } = &self.popup else { return Ok(()) };
        let Some(hit) = hits.get(*cursor).cloned() else { return Ok(()) };

        // A grep hit (content match) opens the viewer right on the matched
        // line — the whole reason you grepped. The results list is stashed so
        // Esc from the viewer returns to it, for scanning hit after hit. A name
        // match just navigates to the file.
        if let Some((lineno, _)) = &hit.line {
            let results = std::mem::replace(&mut self.popup, Popup::None);
            self.find_return = Some(Box::new(results));
            self.stop_find(); // freeze the list; the stash already holds the hits
            let name = hit
                .path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| hit.rel.display().to_string());
            self.open_viewer_at(&hit.path, &name, lineno.saturating_sub(1));
            self.message = Some(tr(self.lang, "Esc → back to results", "Esc で結果一覧へ戻ります").into());
            return Ok(());
        }
        self.popup = Popup::None;
        self.stop_find();

        let (dir, name) = if hit.is_dir {
            (hit.path.clone(), None)
        } else {
            match hit.path.parent() {
                Some(p) => (p.to_path_buf(), Some(hit.path.clone())),
                None => return Ok(()),
            }
        };
        if let Some(p) = self.active_pane_mut() {
            p.jump_to(dir)?;
            if let Some(target) = name {
                if let Some(i) = p.entries.iter().position(|e| e.path == target) {
                    p.cursor = i;
                }
            }
        }
        self.message = Some(format!("→ {}", hit.rel.display()));
        Ok(())
    }

    pub(crate) fn stop_find(&mut self) {
        if let Some(job) = self.find_job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    // ------- Jump to a typed path -------
    pub(crate) fn start_jump_path(&mut self) {
        // Seed with the current directory: most jumps are edits of where you
        // already are, and it doubles as a reminder of the expected form.
        let here = self.active_pane().map(|p| p.cwd.display().to_string()).unwrap_or_default();
        self.open_popup(text_input(
                "go to path",
                "directory to enter, or file to open:",
                here,
                InputKind::JumpPath,
            ));
    }

    /// Enter a typed directory, or open a typed file with its usual program.
    pub(crate) fn finish_jump_path(&mut self, raw: &str) -> Result<()> {
        let raw = raw.trim();
        if raw.is_empty() {
            self.message = Some(tr(self.lang, "cancelled", "中止しました").into());
            return Ok(());
        }
        let path = expand_path(raw);
        if !path.exists() {
            self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("そのようなパスはありません: {}", path.display())
        } else {
            format!("no such path: {}", path.display())
        });
            return Ok(());
        }
        if path.is_dir() {
            if let Some(p) = self.active_pane_mut() {
                p.jump_to(path.clone())?;
            }
            self.message = Some(format!("→ {}", path.display()));
            return Ok(());
        }
        // A file: put the cursor on it in its own directory, then open it the
        // same way Enter would — including any init.lua on_open handler.
        if let Some(parent) = path.parent().map(|p| p.to_path_buf()) {
            if let Some(p) = self.active_pane_mut() {
                let _ = p.jump_to(parent);
                if let Some(i) = p.entries.iter().position(|e| e.path == path) {
                    p.cursor = i;
                }
            }
        }
        self.open_externally();
        Ok(())
    }

    /// Open the context menu beside the highlighted entry, as though it had
    /// been right-clicked.
    pub(crate) fn open_menu_at_cursor(&mut self) {
        let rect = match self.focused {
            FocusedPane::Left => self.layout_rects.left,
            FocusedPane::Right => self.layout_rects.right,
            FocusedPane::Shell => self.layout_rects.shell,
        };
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        // Anchor on the cursor's row so the menu appears next to what it acts
        // on, the same as a right-click would.
        let view_h = rect.height.saturating_sub(2);
        let offset = self
            .active_pane()
            .map(|p| {
                let first = p.cursor.saturating_sub(view_h.saturating_sub(1) as usize);
                (p.cursor - first) as u16
            })
            .unwrap_or(0);
        let row = (rect.y + 1 + offset).min(rect.y + rect.height.saturating_sub(1));
        self.open_context_menu(rect.x + 4, row);
    }

    // ------- Manual -------
    pub(crate) fn open_manual(&mut self) {
        // Reading the keys is a question about the window, not a request to
        // close whatever is open in it. A panel docked beside the listing steps
        // aside and comes back when the manual does. See `stash_viewer`.
        self.open_popup(Popup::Manual { lines: manual_lines(&self.keymap, self.menu_lang), scroll: 0 });
    }

    // ------- AI -------

    /// Re-read `init.lua` and apply everything that can change without a
    /// restart: keymaps, options, SSH hosts and open handlers. The colour theme
    /// and border style are installed once at startup (into set-once globals),
    /// so a change to those is reported as needing a restart rather than being
    /// silently ignored.
    pub(crate) fn reload_config(&mut self) {
        let config = cian_lua::load();

        // Rebuild the user keymap, validating action names as at startup.
        if let Some(w) = config.options.tab_width {
            cian_core::viewer::set_tab_width(w);
        }
        let mut keymap: HashMap<(char, KeyModifiers), Action> = HashMap::new();
        let mut problems: Vec<String> = config.errors.clone();
        for (spec, name) in &config.keymaps {
            let Some(k) = crate::theme::parse_key_spec(spec) else {
                problems.push(format!("keymap: cannot read the key {spec:?}"));
                continue;
            };
            match action_from_name(name) {
                Some(a) => {
                    keymap.insert(k, a);
                }
                None => problems.push(format!("keymap: unknown action {name:?} (key {spec:?})")),
            }
        }
        self.keymap = keymap;

        // Re-read macro.lua, count.lua and shortcuts too, so `:reload` picks them up.
        let (macros, macro_error) = crate::macro_run::load_macros();
        self.macros = macros;
        self.macro_error = macro_error;
        self.count_opts = crate::count::load_count_opts();
        self.shortcuts = ShortcutStore::load_or_default();
        // A hand-edited shortcuts.lua that fails to parse is otherwise loaded as
        // "empty" in silence — surface it so an edit that "isn't reflected" is
        // explained rather than swallowed.
        if let Some(p) = cian_lua::config_read_path("shortcuts.lua").filter(|p| p.exists()) {
            if let Err(e) = cian_lua::shortcuts::load(&p) {
                problems.push(format!("shortcuts.lua: {}", e));
            }
        }

        // Live-applicable options.
        self.lang = Lang::from_opt(config.options.lang.as_deref());
        self.menu_lang = match config.options.menu_lang.as_deref() {
            Some(s) => Lang::from_opt(Some(s)),
            None => self.lang,
        };
        self.menu_lang_pinned = config.options.menu_lang.is_some();
        self.show_key_hints = config.options.key_hints.unwrap_or(true);
        self.anim_dur =
            Duration::from_millis(config.options.animation_ms.unwrap_or(DEFAULT_ANIM_MS));
        let show_hidden = config.options.show_hidden.unwrap_or(true);
        for tabs in [&mut self.left, &mut self.right] {
            for pane in tabs.all_mut() {
                pane.set_show_hidden(show_hidden);
            }
        }

        // The theme can be swapped live, so apply whatever the file now resolves
        // to. Borders still live in a set-once global (they change the glyphs the
        // whole frame is built from), so those still need a restart.
        let (resolved, theme_errors) = resolve_theme(&config.theme);
        problems.extend(theme_errors);
        let wear = resolved;
        set_theme(wear);
        self.theme_name = theme_name_of(&wear).unwrap_or("custom").to_string();
        let borders_changed =
            resolve_border_type(config.options.borders.as_deref()) != border_type();

        // ssh hosts and on_open handlers come along with the replaced config.
        self.config = config;
        // Rebuild the AI request config too, and re-probe availability, so
        // endpoint/model/api_base_url can be tuned with `:reload` alone.
        self.ai = crate::ai_config_from(&self.config);
        self.spawn_ai_probe();

        if !problems.is_empty() {
            let mut lines = vec!["reloaded with issues:".to_string(), String::new()];
            let total = problems.len();
            lines.extend(problems.into_iter().take(10));
            if total > 10 {
                lines.push(format!("... and {} more", total - 10));
            }
            self.open_popup(Popup::Notice { lines });
        } else if borders_changed {
            self.message = Some(tr(self.lang, "config reloaded. restart to apply the border change", "設定を再読み込みしました。枠線の変更は再起動後に反映されます").into());
        } else {
            self.message = Some(tr(self.lang, "config reloaded", "設定を再読み込みしました").into());
        }
    }

    // ------- Theme gallery (#8) -------

    /// Open the whole-app theme gallery (`:theme` with no argument, or the menu).
    /// The cursor starts on the active theme; moving it previews each preset live.
    pub(crate) fn start_theme_picker(&mut self) {
        let current = theme();
        let cursor = theme_name_of(&current)
            .and_then(|n| THEME_NAMES.iter().position(|&m| m == n))
            .unwrap_or(0);
        self.open_popup(Popup::ThemePicker { cursor, scope: ThemeScope::App { revert: current } });
    }

    /// Open the gallery targeting a single file pane (0 = left, 1 = right). The
    /// preview recolors just that pane; the rest of the app keeps its theme.
    pub(crate) fn start_pane_theme_picker(&mut self, side: usize) {
        let revert = self.pane_theme[side].clone();
        let cursor = revert
            .as_deref()
            .and_then(|n| THEME_NAMES.iter().position(|&m| m == n))
            .unwrap_or(0);
        self.open_popup(Popup::ThemePicker { cursor, scope: ThemeScope::Pane { side, revert } });
    }

    /// Move the gallery cursor by `delta` (wrapping) and preview that preset,
    /// applying it to whichever target the gallery drives.
    pub(crate) fn theme_picker_move(&mut self, delta: isize) {
        if let Popup::ThemePicker { cursor, scope } = &mut self.popup {
            let n = THEME_NAMES.len() as isize;
            let c = (*cursor as isize + delta).rem_euclid(n) as usize;
            *cursor = c;
            let name = THEME_NAMES[c];
            match scope {
                ThemeScope::App { .. } => {
                    if let Some(t) = theme_preset(name) {
                        set_theme(t);
                    }
                }
                ThemeScope::Pane { side, .. } => self.pane_theme[*side] = Some(name.to_string()),
            }
        }
        // The gallery previews live, so the colours behind it are already wrong.
        self.drop_highlight_cache();
    }

    /// Keep the previewed theme and close the gallery.
    pub(crate) fn theme_picker_commit(&mut self) {
        if let Popup::ThemePicker { cursor, scope } = &self.popup {
            let name = THEME_NAMES[*cursor];
            match scope {
                ThemeScope::App { .. } => {
                    if let Some(t) = theme_preset(name) {
                        set_theme(t);
                    }
                    self.theme_name = name.to_string();
                    save_theme_pref(name); // persist so the next launch keeps it
                    self.message = Some(format!("theme: {name} (saved)"));
                }
                ThemeScope::Pane { side, .. } => {
                    let s = *side;
                    self.pane_theme[s] = Some(name.to_string());
                    let which = if s == 0 { "left" } else { "right" };
                    self.message = Some(format!("{which} pane theme: {name}"));
                }
            }
        }
        self.popup = Popup::None;
        self.drop_highlight_cache();
        // Back to the file it was picked over, if it was picked from there.
        self.restore_viewer();
    }

    /// Throw away the cached syntax colours.
    ///
    /// They are worked out against the page they will be drawn on, so a change
    /// of theme makes every one of them wrong until they are computed again —
    /// and the gallery previews live, so this happens on every keypress in it.
    pub(crate) fn drop_highlight_cache(&mut self) {
        let clear = |p: &mut Popup| {
            if let Popup::Viewer { hl, .. } = p {
                hl.clear();
            }
        };
        clear(&mut self.popup);
        for t in &mut self.viewer_tabs {
            clear(t);
        }
        if let Some(o) = self.viewer_split.as_deref_mut() {
            clear(o);
        }
        if let Some(v) = self.viewer_return.as_deref_mut() {
            clear(v);
        }
    }

    /// Cancel the gallery: restore whatever the target had when it opened.
    pub(crate) fn theme_picker_cancel(&mut self) {
        if let Popup::ThemePicker { scope, .. } = &self.popup {
            match scope {
                ThemeScope::App { revert } => set_theme(*revert),
                ThemeScope::Pane { side, revert } => self.pane_theme[*side] = revert.clone(),
            }
        }
        self.popup = Popup::None;
        self.drop_highlight_cache();
        self.restore_viewer();
    }

    /// Clear a pane's theme override so it follows the app theme again (the `x`
    /// key in a pane-scoped gallery). A no-op for the app-scoped gallery.
    pub(crate) fn theme_picker_clear_pane(&mut self) {
        if let Popup::ThemePicker { scope: ThemeScope::Pane { side, .. }, .. } = &self.popup {
            let s = *side;
            self.pane_theme[s] = None;
            let which = if s == 0 { "left" } else { "right" };
            self.message = Some(format!("{which} pane follows the app theme"));
            self.popup = Popup::None;
        }
    }

    /// `:theme <name>` — switch directly, no gallery.
    pub(crate) fn set_theme_by_name(&mut self, name: &str) {
        match theme_preset(name) {
            Some(t) => {
                set_theme(t);
                self.theme_name = theme_name_of(&t).unwrap_or("custom").to_string();
                save_theme_pref(&self.theme_name); // persist across restarts
                self.message = Some(format!("theme: {} (saved)", self.theme_name));
                self.drop_highlight_cache();
            }
            None => {
                self.message =
                    Some(format!("unknown theme {name:?} — :theme with no argument lists them"));
            }
        }
    }

    // ------- Quit confirmation -------
    pub(crate) fn start_quit_confirm(&mut self) {
        self.open_popup(Popup::ConfirmQuit);
    }

    /// Ask before opening another tab in this pane.
    ///
    /// F9 opened one on the spot, and a new tab looks almost exactly like the
    /// old one — same directory, same listing — so nothing on screen says a
    /// tab has appeared until there are several. Asking costs one keystroke
    /// and makes the tab something that was decided rather than something that
    /// happened.
    pub(crate) fn ask_new_tab(&mut self) {
        let side = match self.focused {
            FocusedPane::Shell => self.last_file_pane,
            p => p,
        };
        self.open_popup(Popup::ConfirmNewTab { side });
    }

    /// Open the tab that [`ask_new_tab`](Self::ask_new_tab) asked about.
    pub(crate) fn open_new_tab(&mut self, side: FocusedPane) -> Result<()> {
        let tabs = match side {
            FocusedPane::Left => &mut self.left,
            FocusedPane::Right => &mut self.right,
            FocusedPane::Shell => return Ok(()),
        };
        tabs.add_clone()
    }

    /// Perform a confirmed close (shell split pane or file tab).
    pub(crate) fn execute_close(&mut self, target: CloseTarget) {
        match target {
            // Shrink the pane away first; the removal happens when the
            // transition lands (or immediately if animation is off).
            CloseTarget::ShellPane => self.close_shell_pane_animated(),
            CloseTarget::ViewerFile => {
                // Put the panel back before closing it: `close_viewer_file`
                // works on what is in `popup`, and the dialog was standing
                // where the panel had been.
                self.restore_viewer();
                self.close_viewer_file();
            }
            CloseTarget::ShellTab => {
                // The last tab taking the shell with it hands the focus back to
                // the listing it was called from, rather than to nothing.
                if self.shell.close_active() {
                    self.focus(self.last_file_pane);
                }
            }
            CloseTarget::FileTab(pane) => {
                let tabs = match pane {
                    FocusedPane::Left => &mut self.left,
                    FocusedPane::Right => &mut self.right,
                    FocusedPane::Shell => return,
                };
                tabs.close_active();
            }
        }
    }

    pub(crate) fn jump_to_next_match(&mut self, forward: bool) {
        let Some(query) = self.last_search_query.clone() else {
            self.message = Some(tr(self.lang, "no previous search", "直前の検索がありません").into());
            return;
        };
        let ql = query.to_lowercase();
        let Some(p) = self.active_pane_mut() else { return };
        let n = p.entries.len();
        if n == 0 { return; }
        let start = p.cursor;
        let mut i = if forward { (start + 1) % n } else { (start + n - 1) % n };
        for _ in 0..n {
            if p.entries[i].name.to_lowercase().contains(&ql) {
                p.cursor = i;
                return;
            }
            i = if forward { (i + 1) % n } else { (i + n - 1) % n };
        }
        self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("パターンが見つかりません: {}", query)
        } else {
            format!("pattern not found: {}", query)
        });
    }

    /// Run a file operation on a worker thread, showing a progress popup —
    /// or, when one is already running, queue it to start automatically when
    /// the runner finishes (`:queue` lists and manages the line).
    pub(crate) fn start_op<F>(&mut self, label: &'static str, work: F)
    where
        F: FnMut(&mut cian_core::progress::Ctl) -> OpReport + Send + 'static,
    {
        // Transfers are the ops whose failures a retry can actually fix (a
        // network blip); local failures (permissions, missing files) are not
        // improved by trying again.
        let retries = if matches!(label, "uploading" | "downloading") { 2 } else { 0 };
        let queued = QueuedOp { label, work: Box::new(work), retries };
        if self.op_job.is_some() {
            self.op_queue.push_back(queued);
            self.message = Some(if self.lang == Lang::Ja {
                format!("キューに追加 — {} 件待ち（:queue で一覧）", self.op_queue.len())
            } else {
                format!("queued — {} waiting (:queue to manage)", self.op_queue.len())
            });
            self.popup = Popup::None;
            return;
        }
        self.spawn_op(queued);
        self.popup = Popup::None;
    }

    /// Actually put a (possibly queued) op on its worker thread.
    fn spawn_op(&mut self, mut op: QueuedOp) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_cancel = Arc::clone(&cancel);
        let worker_tx = tx.clone();
        let retries = op.retries;
        std::thread::spawn(move || {
            // Rate-limit the updates: a chunked copy calls back on every
            // megabyte, and forwarding all of that would flood the channel
            // and repaint far more often than a screen can show.
            let mut last = Instant::now() - Duration::from_secs(1);
            let mut on_progress = |p: &cian_core::progress::Progress| {
                if last.elapsed() >= Duration::from_millis(60) {
                    last = Instant::now();
                    let _ = worker_tx.send(OpMsg::Tick(p.clone()));
                }
            };
            let mut ctl = cian_core::progress::Ctl {
                cancel: &worker_cancel,
                on_progress: &mut on_progress,
            };
            let mut report = (op.work)(&mut ctl);
            // Auto-retry inside the worker: transfers re-run whole (uploads
            // overwrite, so a re-run converges), with a growing pause first.
            let mut attempt = 0u8;
            while attempt < retries
                && !report.errors.is_empty()
                && !worker_cancel.load(Ordering::Relaxed)
            {
                attempt += 1;
                std::thread::sleep(Duration::from_secs(2 * attempt as u64));
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                report = (op.work)(&mut ctl);
            }
            let _ = tx.send(OpMsg::Done(report));
        });
        self.op_job = Some(OpJob {
            rx,
            cancel,
            label: op.label,
            latest: cian_core::progress::Progress::default(),
            started: Instant::now(),
            undo: None,
            last_progress: Instant::now(),
            cancel_requested: None,
        });
    }

    /// Start the next queued op, if the runner seat is free. Returns true if
    /// one was started.
    pub(crate) fn start_next_op(&mut self) -> bool {
        if self.op_job.is_some() {
            return false;
        }
        match self.op_queue.pop_front() {
            Some(op) => {
                self.spawn_op(op);
                true
            }
            None => {
                self.op_bar_hidden = false;
                false
            }
        }
    }

    /// `:nobom` — strip the UTF-8 byte-order mark from the marked files (or
    /// the cursor's), after a confirm. UTF-16 BOMs are left alone: without
    /// one, a UTF-16 file's byte order is guesswork.
    pub(crate) fn start_nobom(&mut self) {
        let targets: Vec<PathBuf> = self
            .active_pane()
            .map(|p| p.target_paths())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.is_file())
            .collect();
        if targets.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        }
        self.open_popup(Popup::ConfirmNoBom { targets });
    }

    pub(crate) fn confirm_nobom(&mut self) -> Result<()> {
        let Popup::ConfirmNoBom { targets } = std::mem::replace(&mut self.popup, Popup::None)
        else {
            return Ok(());
        };
        let (mut stripped, mut none, mut utf16, mut failed) = (0usize, 0usize, 0usize, 0usize);
        for t in &targets {
            match cian_core::ops::strip_utf8_bom(t) {
                Ok(Some(true)) => stripped += 1,
                Ok(Some(false)) => none += 1,
                Ok(None) => utf16 += 1,
                Err(_) => failed += 1,
            }
        }
        let mut parts = if self.lang == Lang::Ja {
            vec![format!("BOM除去 {} 件", stripped)]
        } else {
            vec![format!("stripped {} BOM(s)", stripped)]
        };
        if none > 0 {
            parts.push(if self.lang == Lang::Ja { format!("BOMなし {}", none) } else { format!("{} had none", none) });
        }
        if utf16 > 0 {
            parts.push(if self.lang == Lang::Ja {
                format!("UTF-16 のためスキップ {}", utf16)
            } else {
                format!("{} UTF-16 (kept — load-bearing)", utf16)
            });
        }
        if failed > 0 {
            parts.push(if self.lang == Lang::Ja { format!("失敗 {}", failed) } else { format!("{} failed", failed) });
        }
        self.message = Some(parts.join(" — "));
        self.reload_both();
        if let Some(p) = self.active_pane_mut() {
            p.clear_marks();
        }
        Ok(())
    }

    /// `:queue` — the running operation and the line behind it.
    pub(crate) fn start_op_queue(&mut self) {
        if self.op_job.is_none() && self.op_queue.is_empty() {
            self.message = Some(tr(
                self.lang,
                "no operations running or queued",
                "実行中・待機中の操作はありません",
            ).into());
            return;
        }
        // A panel open beside the listing steps aside rather than being
        // written over — `:queue` is reached from a listing while the panel is
        // still what is in `self.popup`. Fourth instance of this; the fix
        // belongs in a setter, not at each call site. See `stash_viewer`.
        self.open_popup(Popup::OpQueue { cursor: 0 });
    }

    /// `x` in the queue popup. Row 0 = the running op: first press asks it to
    /// stop; once the grace period passes with the worker still deaf, the
    /// same key abandons it. Other rows just leave the line.
    pub(crate) fn op_queue_kill(&mut self, row: usize) {
        if row == 0 {
            let stuck_for = self
                .op_job
                .as_ref()
                .and_then(|j| j.cancel_requested)
                .map(|t| t.elapsed().as_secs());
            if self.op_job.is_none() {
                // The row is drawn as "(nothing running)" and `x` on it did
                // nothing and said nothing — the same silent refusal the
                // switches had.
                self.message = Some(tr(
                    self.lang,
                    "nothing is running — x on a queued line removes it",
                    "実行中の操作はありません — 待機行の x で取り消せます",
                ).into());
                return;
            }
            match stuck_for {
                None => self.cancel_op_job(),
                Some(s) if s >= OP_ABANDON_GRACE_SECS => self.abandon_op(),
                Some(_) => {
                    self.message = Some(tr(
                        self.lang,
                        "stop already requested — press x again shortly to abandon",
                        "停止要求済み — 少し待ってもう一度 x で見捨てます",
                    ).into());
                }
            }
            return;
        }
        let i = row - 1;
        if i < self.op_queue.len() {
            self.op_queue.remove(i);
            self.message = Some(tr(self.lang, "removed from the queue", "キューから外しました").into());
        }
    }

    /// Give up on a worker that is ignoring its cancel flag (wedged in a
    /// syscall): orphan the thread and let the queue move on. The thread may
    /// linger until the process exits — said out loud rather than hidden.
    pub(crate) fn abandon_op(&mut self) {
        if let Some(job) = self.op_job.take() {
            job.cancel.store(true, Ordering::Relaxed);
            drop(job);
            self.message = Some(tr(
                self.lang,
                "⚠ abandoned — the stuck worker may linger until cian exits; continuing with the queue",
                "⚠ 見捨てました — 固まったワーカーは終了まで残る場合があります。キューを続行します",
            ).into());
        }
        self.start_next_op();
    }

    /// Drain worker updates. Returns true if the UI should repaint.
    /// Ring the terminal bell and post a desktop notification when a job that
    /// ran at least `notify_min_secs` finishes — the "I started a big copy and
    /// walked away" case. Silent for quick jobs and when `notify` is off.
    ///
    /// OSC 9 (`ESC ] 9 ; text BEL`) is the notification escape both Windows
    /// Terminal and iTerm2 understand; terminals that don't just ignore it. The
    /// sequences are out-of-band, so they don't disturb the drawn UI.
    pub(crate) fn notify_task_done(&self, elapsed: Duration, summary: &str) {
        if !self.notify_runtime.or(self.config.options.notify).unwrap_or(true) {
            return;
        }
        let min = self.config.options.notify_min_secs.unwrap_or(5);
        if elapsed.as_secs() < min {
            return;
        }
        use std::io::Write;
        // Drop control chars so the summary can't break out of the sequence.
        let clean: String = summary.chars().filter(|c| !c.is_control()).collect();
        let mut out = std::io::stdout();
        let _ = write!(out, "\x07\x1b]9;cian — {clean}\x07");
        let _ = out.flush();
    }

    pub(crate) fn poll_op_job(&mut self) -> bool {
        let Some(job) = &mut self.op_job else { return false };
        let mut changed = false;
        let mut finished = None;
        loop {
            match job.rx.try_recv() {
                Ok(OpMsg::Tick(p)) => {
                    // Bytes moving is the liveness signal; a tick with the
                    // same count keeps the stall clock running.
                    if p.bytes_done != job.latest.bytes_done || p.files_done != job.latest.files_done {
                        job.last_progress = Instant::now();
                    }
                    job.latest = p;
                    changed = true;
                }
                Ok(OpMsg::Done(r)) => {
                    finished = Some(r);
                    changed = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                // The worker vanished without reporting; do not wait forever.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished = Some(OpReport::default());
                    changed = true;
                    break;
                }
            }
        }
        if let Some(report) = finished {
            let cancelled = self.op_job.as_ref().map(|j| j.cancel.load(Ordering::Relaxed));
            let label = self.op_job.as_ref().map(|j| j.label).unwrap_or("");
            let elapsed = self.op_job.as_ref().map(|j| j.started.elapsed()).unwrap_or_default();
            let undo = self.op_job.as_mut().and_then(|j| j.undo.take());
            self.op_job = None;
            // Record the undo only when the op finished cleanly (a partial move
            // with conflicts/errors would make undo ambiguous).
            if cancelled == Some(false) && report.errors.is_empty() {
                if let Some(a) = undo {
                    self.record_undo(a);
                }
            }
            self.reload_both();
            // An archive pane's listing is synthetic too: re-list it so a zip
            // op (add/delete/rename) shows its result.
            self.refresh_archive_panes();
            // A remote pane isn't touched by reload_both (its listing is
            // synthetic); re-fetch it if an upload just landed files on it.
            if let Some(side) = self.remote_refresh.take() {
                if let Some(cwd) = self.side_pane(side).remote_view().map(|(_, p)| p.to_string()) {
                    self.remote_pane_ls_spawn(side, cwd);
                }
            }
            if let Some(p) = self.active_pane_mut() {
                p.clear_marks();
            }
            self.flash(self.focused);
            // A refused copy/move is the one failure on Windows with a real
            // way out, and there are two of them in sequence.
            //
            // **First `robocopy /B`** — the administrator's backup privileges,
            // which is what an ACL that does not name you actually calls for
            // and what a cian already started as administrator can spend right
            // now. **Then, only if that was refused too**, a new elevated
            // process: the one thing it fixes is cian not being administrator
            // at all, which is precisely what robocopy will have just said.
            //
            // Offered in that order rather than side by side, because a dialog
            // raised by a failure has one job — say what happened, and offer
            // the thing most likely to help. A second answer on a letter is a
            // key only somebody who already knows will press.
            let refused = report.permission_denied && cfg!(windows);
            let backup_failed = label == "backup mode" && !report.errors.is_empty();
            let retry = (refused || backup_failed) && self.pending_elevation.is_some();
            if !retry {
                self.pending_elevation = None;
            }
            // A long job that finished while you were reading mail elsewhere is
            // exactly what the bell/desktop notification is for. Not for a
            // cancel, and not while we still need an elevation confirm.
            if cancelled == Some(false) && !retry {
                let n = report.ok;
                // `hashing` repurposes `errors` to carry the digests, so it is
                // never a failure count there.
                let summary = if label == "hashing" || report.errors.is_empty() {
                    format!("{label} finished — {n} item(s)")
                } else {
                    format!("{label} finished with {} problem(s)", report.errors.len())
                };
                self.notify_task_done(elapsed, &summary);
            }
            if cancelled == Some(true) {
                self.pending_elevation = None;
                self.message = Some(format!(
                    "cancelled — {} done before stopping",
                    report.ok
                ));
            } else if retry {
                let (op, targets, dest, conflict) = self.pending_elevation.take().unwrap();
                // The second offer only exists because the first was tried.
                let how = if backup_failed { RetryHow::Elevate } else { RetryHow::Backup };
                let why = report.errors.first().cloned().unwrap_or_default();
                self.open_popup(crate::transfer_retry_popup(op, targets, dest, conflict, how, why));
            } else {
                self.show_op_report(&report);
                // A checksum is worth pasting into a verify field, so put the
                // digest(s) straight onto the clipboard when hashing finishes.
                if label == "hashing" {
                    let sums: Vec<String> = report
                        .errors
                        .iter()
                        .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
                        .collect();
                    if !sums.is_empty() {
                        if let Some(cb) = self.clipboard.as_mut() {
                            let _ = cb.set_text(sums.join("\n"));
                        }
                        if let Popup::Notice { lines } = &mut self.popup {
                            lines.push(String::new());
                            lines.push("→ copied to the clipboard".to_string());
                        }
                    }
                }
            }
            // The runner seat is free: pull the next queued op in.
            self.start_next_op();
        }
        changed
    }

    /// Reload any pane whose directory changed underneath it.
    ///
    /// cian only ever reloaded after its own actions, so a file created by
    /// something else — a build, a download, a colleague's sync — simply never
    /// appeared. Returns true if anything was refreshed.
    pub(crate) fn poll_external_changes(&mut self) -> bool {
        if self.last_watch.elapsed() < WATCH_INTERVAL {
            return false;
        }
        self.last_watch = Instant::now();
        // Not while an operation runs: it will reload at the end anyway, and
        // re-reading a directory being written to would just fight it.
        if self.op_job.is_some() {
            return false;
        }
        let mut changed = false;
        for tabs in [&mut self.left, &mut self.right] {
            let pane = tabs.active_mut();
            if pane.is_stale() {
                let _ = pane.reload();
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn cancel_op_job(&mut self) {
        if let Some(job) = &mut self.op_job {
            job.cancel.store(true, Ordering::Relaxed);
            if job.cancel_requested.is_none() {
                job.cancel_requested = Some(Instant::now());
            }
            self.message = Some(tr(self.lang, "stopping…", "停止中…").into());
        }
    }

    /// True when the running op has made no byte progress for a while — the
    /// "is it stuck?" light. Slow-but-moving transfers never trip it.
    pub(crate) fn op_stalled(&self) -> bool {
        self.op_job
            .as_ref()
            .map(|j| j.last_progress.elapsed().as_secs() >= OP_STALL_SECS)
            .unwrap_or(false)
    }

    /// Remember a reversible operation for `u`. The stack is capped so a long
    /// session cannot grow it without bound.
    pub(crate) fn record_undo(&mut self, action: UndoAction) {
        const UNDO_CAP: usize = 64;
        // Doing something new is where a redo chain ends.
        self.redo_stack.clear();
        self.undo_stack.push(action);
        if self.undo_stack.len() > UNDO_CAP {
            self.undo_stack.remove(0);
        }
    }

    /// `u` — reverse the last thing done: a rename, a create, a move, or a
    /// step into another directory.
    pub(crate) fn undo_last(&mut self) {
        let Some(action) = self.undo_stack.pop() else {
            self.message = Some(tr(self.lang, "nothing to undo", "取り消せる操作はありません").into());
            return;
        };
        // What `Ctrl+Y` would put back. A file that was *created* is the one
        // thing that cannot be: undoing it removed it, and nothing here
        // remembers what was inside. A copy is not in that position — its
        // sources are untouched — so it goes on like the rest.
        if !matches!(action, UndoAction::Created { .. }) {
            self.redo_stack.push(action.clone());
        }
        let msg = match action {
            UndoAction::Rename { from, to } => {
                if !to.exists() {
                    format!("cannot undo rename: {} is gone", to.display())
                } else if from.exists() {
                    format!("cannot undo rename: {} already exists", from.display())
                } else {
                    match std::fs::rename(&to, &from) {
                        Ok(()) => format!("undo: renamed back to {}", from.display()),
                        Err(e) => format!("undo failed: {}", e),
                    }
                }
            }
            UndoAction::Created { path } => {
                let r = if path.is_dir() {
                    std::fs::remove_dir(&path) // only if empty — a filled dir stays
                } else {
                    std::fs::remove_file(&path)
                };
                match r {
                    Ok(()) => format!("undo: removed {}", path.display()),
                    Err(e) => format!("undo failed: {}", e),
                }
            }
            UndoAction::Moved { pairs } => {
                let (mut ok, mut fail) = (0usize, 0usize);
                for (now, back) in pairs {
                    if !now.exists() || back.exists() {
                        fail += 1;
                        continue;
                    }
                    if let Some(parent) = back.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::rename(&now, &back) {
                        Ok(()) => ok += 1,
                        Err(_) => fail += 1,
                    }
                }
                if fail == 0 {
                    format!("undo: moved {} back", ok)
                } else {
                    format!("undo: moved {} back, {} could not be undone", ok, fail)
                }
            }
            UndoAction::Copied { srcs, dest, paths } => {
                // Only what is still there: the list was drawn up before the
                // copy ran, so a file it never managed to write is on it.
                let here: Vec<_> = paths.into_iter().filter(|p| p.exists()).collect();
                let r = cian_core::ops::delete_many(&here, cian_core::ops::DeleteMode::Trash);
                if r.errors.is_empty() {
                    format!("undo: {} copied to the trash", r.ok)
                } else {
                    // Put back what could not be taken. The step was popped
                    // before it ran, so stopping here would spend the only
                    // chance to undo this copy on an attempt that did nothing
                    // — and the usual cause (a permission the OS is
                    // withholding) is one the person can fix and try again.
                    let left: Vec<_> = here.into_iter().filter(|p| p.exists()).collect();
                    let n = left.len();
                    if n > 0 {
                        // Off the redo stack too: half of it is still on disk,
                        // so "do it again" is no longer a thing with one
                        // meaning.
                        self.redo_stack.pop();
                        self.undo_stack.push(UndoAction::Copied { srcs, dest, paths: left });
                    }
                    format!("undo: {} to the trash, {n} left — {}", r.ok, r.errors[0])
                }
            }
        };
        self.reload_both();
        self.message = Some(msg);
    }

    /// `Ctrl+Y` / `:redo` — do again what `u` just undid.
    pub(crate) fn redo_last(&mut self) {
        let Some(action) = self.redo_stack.pop() else {
            self.message = Some(tr(self.lang, "nothing to redo", "やり直す操作はありません").into());
            return;
        };
        // Back onto the undo stack, so the two keys walk the same chain in
        // either direction. Not through `record_undo`, which would empty the
        // redo stack it was just taken from.
        self.undo_stack.push(action.clone());
        let msg = match action {
            UndoAction::Rename { from, to } => match std::fs::rename(&from, &to) {
                Ok(()) => format!("redo: renamed to {}", to.display()),
                Err(e) => format!("redo failed: {e}"),
            },
            UndoAction::Moved { pairs } => {
                let (mut ok, mut fail) = (0usize, 0usize);
                for (now, back) in pairs {
                    // `pairs` reads "it is at `now`, it was at `back`", so a
                    // redo is the move from `back` to `now` again.
                    if let Some(parent) = now.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::rename(&back, &now) {
                        Ok(()) => ok += 1,
                        Err(_) => fail += 1,
                    }
                }
                if fail == 0 {
                    format!("redo: moved {ok} again")
                } else {
                    format!("redo: moved {ok} again, {fail} could not be redone")
                }
            }
            UndoAction::Copied { srcs, dest, .. } => {
                // The sources never moved, so doing it again is doing it. Skip
                // is the conflict rule on purpose: undo took only what this
                // copy created, so anything wearing one of those names now is
                // somebody else's and is not this key's to write over.
                // Worked out **before** the copy, exactly as the first one did.
                // Afterwards every destination exists and `copy_creates` — which
                // answers "what is not there yet" — would come back empty, and
                // the `u` that follows would have nothing to take back.
                let made = cian_core::ops::copy_creates(&srcs, &dest);
                let (mut ok, mut fail) = (0usize, 0usize);
                for src in &srcs {
                    match cian_core::ops::copy_one(src, &dest, cian_core::ops::Conflict::Skip) {
                        Ok(_) => ok += 1,
                        Err(_) => fail += 1,
                    }
                }
                if let Some(UndoAction::Copied { paths, .. }) = self.undo_stack.last_mut() {
                    *paths = made;
                }
                if fail == 0 {
                    format!("redo: copied {ok} again")
                } else {
                    format!("redo: copied {ok} again, {fail} could not be redone")
                }
            }
            // Never pushed to the redo stack — see `undo_last`.
            UndoAction::Created { .. } => String::new(),
        };
        self.reload_both();
        self.message = Some(msg);
    }

    pub(crate) fn finish_transfer(&mut self, conflict: Conflict) -> Result<()> {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let Popup::ConfirmTransfer { op, targets, dest, .. } = popup else { return Ok(()) };
        self.remember_dest(&dest);
        let label = match op {
            PendingOp::Copy => "copying",
            PendingOp::Move => "moving",
        };
        // Remembered so a permission failure can offer an elevated retry; the
        // op-completion handler clears this unless it actually hit that wall.
        self.pending_elevation = Some((op, targets.clone(), dest.clone(), conflict));
        // Both can be undone, and both are worked out before the transfer
        // runs — afterwards the destination looks the same either way. A move
        // ends each target at dest/<name> and undo moves it back; a copy is
        // additive, so undo removes what it added and `copy_creates` decides
        // what that is (and leaves out anything that was already there).
        let undo = match op {
            PendingOp::Move => Some(UndoAction::Moved {
                pairs: targets
                    .iter()
                    .filter_map(|t| t.file_name().map(|n| (dest.join(n), t.clone())))
                    .collect(),
            }),
            // Nothing here is this copy's to take back.
            PendingOp::Copy => match cian_core::ops::copy_creates(&targets, &dest) {
                made if made.is_empty() => None,
                // The sources and the destination ride along so `Ctrl+Y` can
                // run the same copy again; `paths` stays the list of what to
                // take back.
                made => Some(UndoAction::Copied {
                    srcs: targets.clone(),
                    dest: dest.clone(),
                    paths: made,
                }),
            },
        };
        self.start_op(label, move |ctl| match op {
            PendingOp::Copy => cian_core::progress::copy_many(&targets, &dest, conflict, ctl),
            PendingOp::Move => cian_core::progress::move_many(&targets, &dest, conflict, ctl),
        });
        if let Some(job) = self.op_job.as_mut() {
            job.undo = undo;
        }
        Ok(())
    }

    /// Redo the remembered copy/move using the administrator privileges this
    /// process **already holds**, reading and writing past the ACL without
    /// changing it. See [`cian_core::backup`].
    ///
    /// The other half of [`Self::run_elevated_transfer`], and the half that
    /// was missing: starting a second elevated process does nothing for
    /// somebody who started cian as administrator in the first place — same
    /// token, same ACL, same refusal. This is the answer to that, and it is
    /// what Explorer is doing when it asks every time and leaves the folder's
    /// permissions untouched.
    ///
    /// Runs on a worker like any other transfer, but robocopy reports only at
    /// the end, so the bar has no interior — the same bargain the elevated
    /// retry makes.
    pub(crate) fn run_backup_transfer(&mut self) {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let Popup::ConfirmRetry { op, targets, dest, conflict, .. } = popup else { return };
        let move_after = op == PendingOp::Move;
        let items = cian_core::backup::items_for(&targets, &dest);
        // **Armed again for the offer after this one.** The op-done handler
        // takes this when it raises a retry, so by the time we get here it is
        // empty — and without putting it back, a robocopy that is refused too
        // has nothing to build the elevation offer out of. That second dialog
        // was unreachable, which is the quietest kind of wrong: code nobody
        // has seen on screen is code nobody can see is broken.
        self.pending_elevation = Some((op, targets, dest, conflict));
        self.message = Some(tr(
            self.lang,
            "retrying with the administrator's backup privileges…",
            "管理者の権限で読み書きしてやり直しています…",
        ).into());
        self.start_op("backup mode", move |_ctl| {
            cian_core::backup::backup_copy(&items, move_after, conflict)
        });
    }

    /// Redo the remembered copy/move with administrator rights (Windows UAC).
    /// The elevated process runs the transfer itself, so there is no in-app
    /// progress — cian just waits on the worker and reports the outcome.
    pub(crate) fn run_elevated_transfer(&mut self) {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let Popup::ConfirmRetry { op, targets, dest, .. } = popup else { return };
        let move_after = op == PendingOp::Move;
        let n = targets.len();
        // The last offer there is. Left armed, a refused elevation would raise
        // the same dialog again, for ever.
        self.pending_elevation = None;
        let items: Vec<cian_core::elevate::CopyItem> = targets
            .into_iter()
            .map(|src| cian_core::elevate::CopyItem { src, dest_dir: dest.clone() })
            .collect();
        self.message = Some(tr(self.lang, "waiting for the administrator prompt…", "管理者の確認を待っています…").into());
        self.start_op("elevating", move |_ctl| {
            let mut report = OpReport::default();
            match cian_core::elevate::elevated_copy(&items, move_after) {
                Ok(()) => {
                    report.ok = n;
                    report.note = Some("as administrator".into());
                }
                Err(e) => report.note_error(e.to_string()),
            }
            report
        });
    }

    pub(crate) fn finish_delete(&mut self, mode: DeleteMode) -> Result<()> {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let Popup::ConfirmDelete { targets } = popup else { return Ok(()) };
        if cian_core::log::enabled() {
            cian_core::log::log(&format!("delete {:?}: {} target(s)", mode, targets.len()));
        }
        self.start_op("deleting", move |ctl| {
            cian_core::progress::delete_many(&targets, mode, ctl)
        });
        Ok(())
    }

    pub(crate) fn show_op_report(&mut self, report: &OpReport) {
        if !report.errors.is_empty() {
            let mut lines = vec![format!(
                "{} ok · {} skipped · {} errors", report.ok, report.skipped, report.errors.len()
            )];
            // Turn the raw "Access is denied (os error 5)" into something that
            // says what to do about it.
            if report.permission_denied {
                lines.push(String::new());
                lines.push("Permission denied — this location's ACL does not grant you access.".into());
                if cfg!(windows) {
                    // **Not "run as administrator".** That was the advice for
                    // years and it is empty for the case that actually turns
                    // up: somebody already running as administrator, on a
                    // share whose ACL simply does not name them. What being an
                    // administrator gets you here is the backup privilege, and
                    // `b` on the retry is how to spend it.
                    lines.push("Retry with `b` (backup mode) to read and write past it".into());
                    lines.push("as administrator, without changing any permissions.".into());
                } else {
                    lines.push("Copy to a folder you can write to, or fix its permissions.".into());
                }
                lines.push(String::new());
            }
            lines.extend(report.errors.iter().take(8).cloned());
            if report.errors.len() > 8 {
                lines.push(format!("... and {} more", report.errors.len() - 8));
            }
            self.open_popup(Popup::Notice { lines });
        } else {
            let mut msg = format!("done — {} ok · {} skipped", report.ok, report.skipped);
            if let Some(note) = &report.note {
                msg.push_str(&format!(" ({})", note));
            }
            self.message = Some(msg);
        }
    }

    pub(crate) fn finish_text_input(&mut self) -> Result<()> {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let Popup::TextInput { buffer, kind, .. } = popup else { return Ok(()) };
        let name = buffer.trim().to_string();
        // A blank chmod means "keep the default", so it is valid there. A blank
        // adjustment means "never mind", and it has somewhere to go back to —
        // the command it was raised over, which must not be thrown away just
        // because the prompt over it was dismissed. Every other field still
        // treats empty as a cancel.
        let empty_means_something = matches!(
            kind,
            InputKind::UploadChmod { .. }
                | InputKind::DownloadChmod { .. }
                | InputKind::AiShellRefine { .. }
        );
        if name.is_empty() && !empty_means_something {
            self.message = Some(tr(self.lang, "cancelled (empty name)", "中止しました（名前が空）").into());
            return Ok(());
        }
        let result = match &kind {
            InputKind::Rename { original } => match ops::rename_in_place(original, &name) {
                Ok(p) => {
                    self.record_undo(UndoAction::Rename { from: original.clone(), to: p.clone() });
                    Ok(format!("renamed: {}", p.display()))
                }
                Err(e) => Err(e),
            },
            InputKind::NewFile { parent } => match ops::create_file(parent, &name) {
                Ok(p) => {
                    self.record_undo(UndoAction::Created { path: p.clone() });
                    Ok(format!("created: {}", p.display()))
                }
                Err(e) => Err(e),
            },
            InputKind::NewDir { parent } => match ops::create_dir(parent, &name) {
                Ok(p) => {
                    self.record_undo(UndoAction::Created { path: p.clone() });
                    Ok(format!("mkdir: {}", p.display()))
                }
                Err(e) => Err(e),
            },
            InputKind::RemoteMkdir { side } => {
                let side = *side;
                if let Some(cwd) = self.side_pane(side).remote_view().map(|(_, p)| p.to_string()) {
                    self.remote_mut_spawn(side, crate::ssh::RemoteMut::Mkdir(crate::ssh::join_remote(&cwd, &name)));
                }
                return Ok(());
            }
            InputKind::RemoteTouch { side } => {
                let side = *side;
                if let Some(cwd) = self.side_pane(side).remote_view().map(|(_, p)| p.to_string()) {
                    self.remote_mut_spawn(side, crate::ssh::RemoteMut::Touch(crate::ssh::join_remote(&cwd, &name)));
                }
                return Ok(());
            }
            InputKind::RemoteRename { side, from } => {
                let (side, from) = (*side, from.clone());
                if let Some(cwd) = self.side_pane(side).remote_view().map(|(_, p)| p.to_string()) {
                    self.remote_mut_spawn(
                        side,
                        crate::ssh::RemoteMut::Rename { from, to: crate::ssh::join_remote(&cwd, &name) },
                    );
                }
                return Ok(());
            }
            InputKind::JumpPath => return self.finish_jump_path(&name),
            InputKind::FindRecursive => {
                self.start_find(&name, cian_core::search::Mode::Name);
                return Ok(());
            }
            InputKind::GrepRecursive => {
                self.start_find(&name, cian_core::search::Mode::Content);
                return Ok(());
            }
            InputKind::GrepReplaceWith { paths, pattern } => {
                self.build_grep_replace(paths.clone(), pattern, &name);
                return Ok(());
            }
            InputKind::AiShellCmd => {
                self.start_ai_shell_cmd(&name);
                return Ok(());
            }
            InputKind::AiShellRefine { description, rejected } => {
                self.start_ai_shell_refine(description, rejected, &name);
                return Ok(());
            }
            InputKind::DiffSaveAs { text, html, md } => {
                let dir = self.cwd();
                if let Some(dir) = dir {
                    let path = dir.join(&name);
                    // The extension chooses the rendering: side-by-side HTML or
                    // Markdown, else the plain-text form.
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_ascii_lowercase())
                        .unwrap_or_default();
                    let body = match ext.as_str() {
                        "html" | "htm" => html,
                        "md" | "markdown" => md,
                        _ => text,
                    };
                    match std::fs::write(&path, body) {
                        Ok(()) => {
                            self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("比較結果を保存しました → {}", path.display())
        } else {
            format!("saved comparison → {}", path.display())
        });
                            self.reload_active();
                        }
                        Err(e) => self.message = Some(format!("save failed: {}", e)),
                    }
                }
                return Ok(());
            }
            InputKind::DestPath { op, targets } => {
                let dest = expand_path(&name);
                if !dest.is_dir() {
                    self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("ディレクトリではありません: {}", dest.display())
        } else {
            format!("not a directory: {}", dest.display())
        });
                    return Ok(());
                }
                self.popup = crate::transfer_popup(*op, targets.clone(), dest);
                return Ok(());
            }
            InputKind::ZipPassword { dest, sources } => {
                // An empty password here means "never mind the encryption".
                if name.is_empty() {
                    self.message = Some(tr(self.lang, "zip cancelled", "zip を中止しました").into());
                    return Ok(());
                }
                self.start_zip(dest.clone(), sources.clone(), Some(name));
                return Ok(());
            }
            InputKind::CompressName { kind, sources } => {
                if name.is_empty() {
                    self.message = Some(tr(self.lang, "compress cancelled", "圧縮を中止しました").into());
                    return Ok(());
                }
                let Some(cwd) = self.cwd() else { return Ok(()) };
                let ext = match kind {
                    CompressKind::Zip | CompressKind::ZipEnc => ".zip",
                    CompressKind::TarGz => ".tar.gz",
                };
                let mut fname = name.clone();
                let low = fname.to_lowercase();
                let has_ext = match kind {
                    CompressKind::TarGz => low.ends_with(".tar.gz") || low.ends_with(".tgz"),
                    _ => low.ends_with(".zip"),
                };
                if !has_ext {
                    fname.push_str(ext);
                }
                let dest = cwd.join(&fname);
                if dest.exists() {
                    self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("既に存在します: {}", fname)
        } else {
            format!("already exists: {}", fname)
        });
                    return Ok(());
                }
                match kind {
                    CompressKind::Zip => self.start_zip(dest, sources.clone(), None),
                    CompressKind::TarGz => self.start_tar(dest, sources.clone(), true),
                    // Encrypted: chain to the password prompt, which builds it.
                    CompressKind::ZipEnc => {
                        self.open_popup(text_input(
                            "zip password",
                            "password (AES-256; open with 7-Zip, not Explorer):",
                            String::new(),
                            InputKind::ZipPassword { dest, sources: sources.clone() },
                        ));
                    }
                }
                return Ok(());
            }
            InputKind::ExtractPassword { archive, members, dest, strip } => {
                if name.is_empty() {
                    self.message = Some(tr(self.lang, "extract cancelled", "展開を中止しました").into());
                    return Ok(());
                }
                self.run_extract(archive.clone(), members.clone(), dest.clone(), Some(name), strip.clone());
                return Ok(());
            }
            InputKind::RenameZipMember { archive, sub, from, is_dir } => {
                let (archive, sub, from, is_dir) =
                    (archive.clone(), sub.clone(), from.clone(), *is_dir);
                self.finish_zip_rename(archive, sub, from, is_dir, name);
                return Ok(());
            }
            InputKind::SvnCommit { paths } => {
                if name.is_empty() {
                    self.message = Some(tr(self.lang, "commit cancelled (empty message)", "コミットを中止しました（メッセージが空）").into());
                    return Ok(());
                }
                let paths = paths.clone();
                self.svn_commit(&paths, &name);
                return Ok(());
            }
            InputKind::BulkRenamePattern { targets } => {
                if name.trim().is_empty() {
                    self.message = Some(tr(self.lang, "rename cancelled", "リネームを中止しました").into());
                    return Ok(());
                }
                let targets = targets.clone();
                self.build_bulk_rename(&targets, &name);
                return Ok(());
            }
            InputKind::LocalDestPath { files } => {
                if name.trim().is_empty() {
                    self.message = Some(tr(self.lang, "download cancelled", "ダウンロードを中止しました").into());
                    return Ok(());
                }
                let files = files.clone();
                let dir = expand_path(&name);
                self.prompt_download_chmod(files, dir);
                return Ok(());
            }
            InputKind::ShellName => {
                let name = name.trim().to_string();
                if name.chars().count() > 24 {
                    self.message = Some(tr(self.lang,
                        "a tab label is a short one", "名前は 24 文字までです").into());
                    return Ok(());
                }
                let said = name.clone();
                self.shell.rename_active(name);
                self.message = Some(if said.is_empty() {
                    tr(self.lang, "the tab shows its number again", "番号に戻しました").into()
                } else {
                    said
                });
                return Ok(());
            }
            InputKind::LogDir => {
                self.start_session_log(&name);
                return Ok(());
            }
            InputKind::ManualSshTarget { for_scp } => {
                let for_scp = *for_scp;
                let raw = name.trim();
                if raw.is_empty() {
                    self.message = Some(tr(self.lang, "cancelled", "中止しました").into());
                    self.scp_dir = None;
                    return Ok(());
                }
                // Parse user@host[:port]; a bare host defaults the user to root.
                let (user, rest) = match raw.split_once('@') {
                    Some((u, r)) => (u.trim().to_string(), r.trim()),
                    None => ("root".to_string(), raw),
                };
                let (host, port) = match rest.rsplit_once(':') {
                    Some((h, p)) => match p.trim().parse::<u16>() {
                        Ok(n) => (h.trim().to_string(), n),
                        Err(_) => (rest.to_string(), 22),
                    },
                    None => (rest.to_string(), 22),
                };
                if host.is_empty() {
                    self.message = Some(tr(self.lang, "need a host (user@host)", "ホストが必要です（user@host）").into());
                    self.scp_dir = None;
                    return Ok(());
                }
                self.manual_ssh_password(user, host, port, for_scp);
                return Ok(());
            }
            InputKind::ManualSshPass { user, host, port, for_scp } => {
                self.manual_ssh_finish(user.clone(), host.clone(), *port, name.clone(), *for_scp);
                return Ok(());
            }
            InputKind::UploadChmod { remote, idx } => {
                let (mode, err) = parse_chmod(&name);
                if let Some(e) = err {
                    // Invalid mode: re-ask this same file rather than dropping the
                    // whole upload (the old behaviour, which silently lost it).
                    self.message = Some(e);
                    self.prompt_upload_chmod(remote.clone(), *idx);
                    return Ok(());
                }
                self.scp_upload_modes.push(mode);
                self.prompt_upload_chmod(remote.clone(), *idx + 1);
                return Ok(());
            }
            InputKind::DownloadChmod { files, dir } => {
                let (files, dir) = (files.clone(), dir.clone());
                let (mode, err) = parse_chmod(&name);
                if let Some(e) = err {
                    self.message = Some(e);
                    return Ok(());
                }
                self.start_remote_download(files, dir, mode);
                return Ok(());
            }
            InputKind::TransferAs { op, src, dest_dir } => {
                let target = dest_dir.join(&name);
                let verb = if *op == PendingOp::Move { "mv" } else { "cp" };
                let res = match op {
                    PendingOp::Move => std::fs::rename(src, &target).map_err(anyhow::Error::from),
                    PendingOp::Copy => cian_core::ops::copy_one(src, dest_dir, Conflict::Overwrite)
                        .and_then(|_| {
                            let landed =
                                dest_dir.join(src.file_name().unwrap_or_default());
                            if landed != target {
                                std::fs::rename(&landed, &target)?;
                            }
                            Ok(())
                        }),
                };
                match res {
                    Ok(_) => {
                        self.reload_both();
                        self.message = Some(format!("{} → {}", verb, target.display()));
                    }
                    Err(e) => self.message = Some(format!("{}: {}", verb, e)),
                }
                return Ok(());
            }
            InputKind::ShortcutName { path, edit_idx, group } => {
                if *group {
                    // A folder needs no target: create/rename it and reopen.
                    let p = path.clone();
                    if let Some(lvl) = sc_level_mut(&mut self.shortcuts.entries, &p) {
                        match edit_idx {
                            Some(i) if *i < lvl.len() => lvl[*i].name = name,
                            _ => lvl.push(Shortcut::group(name)),
                        }
                    }
                    let cursor = edit_idx.unwrap_or(sc_level(&self.shortcuts.entries, &p).len().saturating_sub(1));
                    self.save_shortcuts(p, cursor, "");
                    return Ok(());
                }
                // A leaf chains into the target step. New shortcuts default to a
                // target picked elsewhere (history) or the entry under the cursor.
                let here = self
                    .pending_shortcut_target
                    .take()
                    .or_else(|| {
                        self.active_pane()
                            .and_then(|p| p.selected().map(|e| e.path.display().to_string()))
                    })
                    .unwrap_or_default();
                let prev_target = edit_idx
                    .and_then(|i| sc_level(&self.shortcuts.entries, path).get(i).map(|s| s.target_str().to_string()))
                    .filter(|t| !t.is_empty())
                    .unwrap_or(here);
                self.open_popup(text_input(
                    "shortcut — target",
                    "URL / path / app   (Ctrl+V paste, Ctrl+A select all):",
                    prev_target,
                    InputKind::ShortcutTarget { path: path.clone(), edit_idx: *edit_idx, name },
                ));
                return Ok(());
            }
            InputKind::ShortcutTarget { path, edit_idx, name: stored_name } => {
                let target = name; // `name` here is actually the trimmed buffer
                if target.is_empty() {
                    self.message = Some(tr(self.lang, "cancelled (empty target)", "中止しました（対象が空）").into());
                    return Ok(());
                }
                let entry = Shortcut::leaf(stored_name.clone(), target);
                let p = path.clone();
                let cursor = if let Some(lvl) = sc_level_mut(&mut self.shortcuts.entries, &p) {
                    match edit_idx {
                        Some(i) if *i < lvl.len() => {
                            lvl[*i] = entry;
                            *i
                        }
                        _ => {
                            lvl.push(entry);
                            lvl.len() - 1
                        }
                    }
                } else {
                    0
                };
                self.save_shortcuts(p, cursor, tr(self.lang, "shortcut saved", "ショートカットを保存しました"));
                return Ok(());
            }
        };
        if let Some(t) = self.active_file_tabs_mut() { let _ = t.active_mut().reload(); }
        match result {
            Ok(msg) => self.message = Some(msg),
            Err(e) => self.open_popup(Popup::Notice { lines: vec![e.to_string()] }),
        }
        Ok(())
    }
}

/// The base name of an archive without its extension, handling the two-part
/// `.tar.gz` / `.tar.bz2` / `.tar.xz` / `.tgz` cases: `proj.tar.gz` → `proj`.
fn archive_stem(name: &str) -> String {
    let low = name.to_lowercase();
    for suf in [".tar.gz", ".tar.bz2", ".tar.xz", ".tgz"] {
        if low.ends_with(suf) {
            return name[..name.len() - suf.len()].to_string();
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name.to_string(),
    }
}

/// A directory path under `parent` named `stem`, made unique by appending
/// `-1`, `-2`, … so extracting never merges into an existing folder.
fn unique_dir(parent: &Path, stem: &str) -> PathBuf {
    let base = parent.join(stem);
    if !base.exists() {
        return base;
    }
    for n in 1.. {
        let cand = parent.join(format!("{stem}-{n}"));
        if !cand.exists() {
            return cand;
        }
    }
    base
}

/// Build the per-file command lines for `:each`. `{}` in `template` expands to
/// each path double-quoted; with no `{}` the quoted path is appended. Paths
/// containing a double quote can't be quoted safely and are skipped — the
/// second element counts them.
pub(crate) fn each_lines(template: &str, paths: &[PathBuf]) -> (Vec<String>, usize) {
    let mut lines = Vec::new();
    let mut skipped = 0usize;
    for p in paths {
        let s = p.display().to_string();
        // The line goes straight into the live shell, and a double quote is not
        // the only thing that survives being inside one. A POSIX shell still
        // expands `$` and a backtick between double quotes, so a file called
        // `$(id).txt` *ran* `id`; PowerShell treats both the same way. A
        // newline would end the command and start another.
        //
        // Not the backslash, though the same list would suggest it: on Windows
        // it is the path separator, and rejecting it would skip every path
        // there. With `$`, the backtick, the quote and the newline gone it can
        // only escape itself or a literal — mangling at worst, never a second
        // command.
        if s.contains(['"', '$', '`', '\n', '\r']) {
            skipped += 1;
            continue;
        }
        let quoted = format!("\"{s}\"");
        lines.push(if template.contains("{}") {
            template.replace("{}", &quoted)
        } else {
            format!("{template} {quoted}")
        });
    }
    (lines, skipped)
}

impl App {
    /// Remove the bookmark the confirmation was about, and go back to the list.
    ///
    /// The list is restored rather than closed: removing one of several is a
    /// tidying-up job, and being thrown out of the list after each one would
    /// make it four keystrokes per bookmark instead of two.
    pub(crate) fn confirm_shortcut_delete(&mut self) {
        let Popup::ConfirmShortcutDelete { path, idx, back, .. } =
            std::mem::replace(&mut self.popup, Popup::None)
        else {
            return;
        };
        if let Some(lvl) = crate::sc_level_mut(&mut self.shortcuts.entries, &path) {
            if idx < lvl.len() {
                lvl.remove(idx);
            }
        }
        self.save_shortcuts(path, idx, "");
        // `save_shortcuts` reopens the list at the right place; if it did not,
        // fall back to whatever was showing before.
        if matches!(self.popup, Popup::None) {
            self.popup = *back;
        }
    }

    /// Leave the bookmark alone and go back to the list.
    pub(crate) fn cancel_shortcut_delete(&mut self) {
        let Popup::ConfirmShortcutDelete { back, .. } =
            std::mem::replace(&mut self.popup, Popup::None)
        else {
            return;
        };
        self.popup = *back;
    }
}

#[cfg(test)]
mod each_tests {
    use super::each_lines;
    use std::path::PathBuf;

    #[test]
    fn expands_placeholder_and_appends_when_absent() {
        let paths = vec![PathBuf::from("/a/one.txt"), PathBuf::from("/a/two.txt")];
        let (lines, skipped) = each_lines("gzip {}", &paths);
        assert_eq!(skipped, 0);
        assert_eq!(lines, vec!["gzip \"/a/one.txt\"", "gzip \"/a/two.txt\""]);

        let (lines, _) = each_lines("md5sum", &paths);
        assert_eq!(lines[0], "md5sum \"/a/one.txt\"");
    }

    #[test]
    fn skips_paths_that_would_break_quoting() {
        let paths = vec![PathBuf::from("/a/ok.txt"), PathBuf::from("/a/we\"ird.txt")];
        let (lines, skipped) = each_lines("rm {}", &paths);
        assert_eq!(skipped, 1);
        assert_eq!(lines, vec!["rm \"/a/ok.txt\""]);
    }
}
