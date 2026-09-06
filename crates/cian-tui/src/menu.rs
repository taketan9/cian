//! The right-click / `M` context menu and clipboard paste: building the menu
//! for the focused surface, drilling into submenus, and running an item — plus
//! clip_targets/paste_clip/reload_both. Split out of lib.rs.
use super::*;

impl App {
    /// The menu for a file open in the viewer: ask the AI about what is
    /// selected, or change the theme. Deliberately short — it is a reading
    /// window, not the file manager.
    pub(crate) fn open_viewer_menu(&mut self, col: u16, row: u16) {
        if !matches!(self.popup, Popup::Viewer { .. }) {
            return;
        }
        let mut items = Vec::new();
        // The AI actions live under their own heading, as they do in the file
        // panes: three of them at the top level would be most of the menu.
        if self.ai.is_some() && self.ai_ready() {
            items.push(MenuItem::AiMenu);
        }
        items.push(MenuItem::Copy);
        // The viewer's own actions. They used to be single letters — `S`, `A`,
        // `B`, `e`, `E`, `m` — which are vi's, and vi's keys belong to vi in a
        // window that behaves like vi. Each still has a `:` command.
        if self.ai.is_some() && self.ai_ready() {
            items.push(MenuItem::ViewerSummary);
        }
        items.push(MenuItem::ViewerSave);
        items.push(MenuItem::ViewerEdit);
        items.push(MenuItem::ViewerEncoding);
        items.push(MenuItem::ViewerBlame);
        items.push(MenuItem::ViewerMermaid);
        // The `:` family, behind one heading. There is no command line in
        // notepad style, so this is the only road to them from that grammar —
        // and the shorter road from either.
        items.push(MenuItem::ViewerLineMenu);
        // Where the file lives, for when reading it raises a question about
        // the folder it is in. This used to be Shift+Enter's whole job.
        items.push(MenuItem::RevealInPane);
        items.push(MenuItem::ViewerEditStyle);
        items.push(MenuItem::ThemePick);
        // Last, and on its own: it is the one item here that loses work.
        items.push(MenuItem::ViewerCloseDiscard);
        // The viewer steps aside rather than closing: the question is about
        // the file, and the file may have unsaved edits in it. `open_popup`
        // does that for every popup now; this line was the original, written
        // out by hand before there was a door to walk through.
        self.open_popup(Popup::ContextMenu { items, cursor: 0, at: (col, row) });
    }

    /// Put a live panel aside before something else takes the popup slot.
    ///
    /// `self.popup` holds one thing at a time, and a docked panel is in it —
    /// while the listing beside it still answers its own keys, by design. So
    /// anything the listing opened *wrote over the open file*, unsaved edits
    /// and all: `?` for the manual, `M` for the menu, a right-click on the
    /// neighbouring pane. Meanwhile the ✕ and `:q` both refuse to close a dirty
    /// panel — the same loss was forbidden through two doors and silent through
    /// a third.
    ///
    /// Nothing is asked of the user, because nothing is lost: what goes aside
    /// is drawn behind whatever took its place and comes back when that closes.
    /// A no-op unless a viewer is what is being displaced.
    pub(crate) fn stash_viewer(&mut self) {
        if matches!(self.popup, Popup::Viewer { .. }) {
            self.viewer_return = Some(Box::new(std::mem::replace(&mut self.popup, Popup::None)));
        }
    }

    /// Put the viewer back after a menu (or what the menu opened) is done.
    /// A no-op when nothing was put aside.
    /// Put a popup on screen, standing a live panel aside first.
    ///
    /// `self.popup` holds one thing, and a docked editor panel lives in it
    /// while the listing beside it still answers its own keys — so anything
    /// the listing opened used to write straight over the open file, unsaved
    /// edits and all. That was found and patched four separate times before
    /// anyone counted: the manual, the context menu, the switches, the
    /// operation queue. Four patches is three chances to forget, and there are
    /// ninety-odd places that raise a popup.
    ///
    /// So it is one door. `stash_viewer` does nothing unless a viewer is what
    /// is being displaced, which makes this safe everywhere and needed at only
    /// a handful of the sites that now use it.
    ///
    /// Not for a viewer itself: opening a second file *replaces* the first,
    /// which is long-standing and pinned by a test, and `viewer_return` holds
    /// one panel — for a menu raised over one, not for the file before last.
    pub(crate) fn open_popup(&mut self, popup: Popup) {
        self.stash_viewer();
        self.popup = popup;
    }

    /// Dismiss whatever is on screen: back to the panel it was raised over, or
    /// to nothing when it was not raised over one.
    ///
    /// What every way out of a dialog, menu, manual or switch list does, in one
    /// place. Four of them had written it out, which is three chances for the
    /// next one to forget — and forgetting it is how an open file gets left
    /// stranded behind a popup that has already closed.
    pub(crate) fn dismiss_to_viewer(&mut self) {
        if !self.restore_viewer() {
            self.popup = Popup::None;
        }
    }

    pub(crate) fn restore_viewer(&mut self) -> bool {
        match self.viewer_return.take() {
            Some(v) => {
                self.popup = *v;
                true
            }
            None => false,
        }
    }

    pub(crate) fn open_context_menu(&mut self, col: u16, row: u16) {
        // Whether the AI helper is usable, checked (and cached) up front so the
        // AI entries only appear when they will work.
        let ai = self.ai.is_some() && self.ai_ready();
        let mut items = Vec::new();
        // Launchers lead both menus: "AI - simple ▸" (when the local model is
        // on), then snippets, macros. Each appears only when it has something
        // to offer.
        if ai {
            items.push(MenuItem::AiMenu);
        }
        if !self.config.snippets.is_empty() {
            items.push(MenuItem::Snippets);
        }
        if !self.macros.is_empty() {
            items.push(MenuItem::Macros);
        }
        let has_hosts = !self.config.ssh_hosts.is_empty();
        // Both menus follow the same zones so common items line up: launchers,
        // pane-specific actions, pane-specific groups, then the shared blocks
        // (connect/transfer, appearance, footer).
        if self.focused == FocusedPane::Shell {
            // The shell can't type `:`, so offer the command line explicitly.
            items.push(MenuItem::CommandInput);
            // Pane action: only "paste" (send to the PTY) makes sense here.
            items.push(MenuItem::Paste);
            // Shell-specific groups.
            items.push(MenuItem::SessionMenu); // log ▸ / encoding
            items.push(MenuItem::WindowMenu);
            // Synchronize input — only when there's more than one pane.
            if self.shell.active_pane_count() > 1 {
                if self.shell.is_broadcasting() {
                    items.push(MenuItem::SyncStop);
                    // Narrow (or widen) the group to just the chosen panes.
                    items.push(MenuItem::SyncMember);
                } else {
                    items.push(MenuItem::SyncStart);
                }
            }
        } else {
            // Shortcuts is a bookmark launcher, so it joins the launcher cluster
            // at the top rather than sitting among the connect/transfer items.
            items.push(MenuItem::Shortcuts);
            // Frequent file ops at the top level; the rest fold into groups.
            items.push(MenuItem::Copy);
            items.push(MenuItem::CopyPathText); // directly under Copy
            // The way out of cian into Finder/Explorer: a terminal program
            // cannot be an OS drag source, so the clipboard is the bridge.
            items.push(MenuItem::CopyFileRef);
            items.push(MenuItem::Cut);
            items.push(MenuItem::PasteHere); // file clipboard paste (Ctrl+V)
            items.push(MenuItem::Rename);
            items.push(MenuItem::Delete);
            items.push(MenuItem::EditTab); // open in the editor in a new shell tab
            items.push(MenuItem::FileMenu); // to-other / copy-to / bulk rename
            // Archive ▸ when there is something to pack, or the cursor is on one.
            let has_targets = self.active_pane().map(|p| !p.target_paths().is_empty()).unwrap_or(false);
            let on_archive = self
                .active_pane()
                .and_then(|p| p.selected())
                .map(|e| !e.is_dir && cian_core::archive::is_archive(&e.path))
                .unwrap_or(false);
            if has_targets || on_archive {
                items.push(MenuItem::ArchiveMenu);
            }
            items.push(MenuItem::InspectMenu); // attributes / hash / compare / count / dupes
            match self.vcs_kind() {
                Some(Vcs::Git) => items.push(MenuItem::GitMenu),
                Some(Vcs::Svn) => items.push(MenuItem::SvnMenu),
                None => {}
            }
        }
        // ── Shared: connect / transfer ──────────────────────────────────────
        items.push(MenuItem::Ssh);
        items.push(MenuItem::RemotePane); // browse a server in this pane (:sftp)
        if has_hosts {
            items.push(MenuItem::SendMenu); // Transfer ▸
        }
        // ── Shared: appearance ──────────────────────────────────────────────
        items.push(MenuItem::Background);
        items.push(MenuItem::ThemePick);
        items.push(MenuItem::Lang);
        if self.focused != FocusedPane::Shell {
            items.push(MenuItem::ViewMenu); // show hidden / pane theme
            // The OS-native group sits last, just above Quit / the manual.
            items.push(MenuItem::OsMenu); // open / open-with / reveal / properties
        }
        // Quit and the manual close every menu — reachable by mouse alone
        // (quitting otherwise needs `q`, which the shell eats).
        items.push(MenuItem::Quit);
        items.push(MenuItem::Manual);
        self.menu_stack.clear();
        // A panel open beside the listing steps aside rather than being
        // overwritten — the same courtesy `open_viewer_menu` above already
        // extends, and for the same reason.
        self.open_popup(Popup::ContextMenu { items, cursor: 0, at: (col, row) });
    }

    /// The children a group item drills into (context-dependent). `None` for a
    /// leaf item.
    pub(crate) fn submenu_children(&self, item: MenuItem) -> Option<Vec<MenuItem>> {
        match item {
            MenuItem::ViewerLineMenu => Some(vec![
                MenuItem::ViewerSort,
                MenuItem::ViewerRsort,
                MenuItem::ViewerUniq,
                MenuItem::ViewerSubstitute,
                MenuItem::ViewerHan,
                MenuItem::ViewerZen,
                MenuItem::ViewerExpand,
                MenuItem::ViewerUnexpand,
                MenuItem::ViewerReindent,
                MenuItem::ViewerLf,
                MenuItem::ViewerCrlf,
                MenuItem::Back,
            ]),
            MenuItem::AiMenu => {
                // The local assistant: a plain chat plus the
                // helpers for whatever it was opened over.
                let mut v = vec![MenuItem::AiChat];
                if self.viewer_return.is_some() {
                    // Over a file being read, the questions are about the text
                    // in front of you rather than about the folder.
                    v.push(MenuItem::AiWriting);
                    v.push(MenuItem::AiCommandHelp);
                    v.push(MenuItem::AiCodeFix);
                    v.push(MenuItem::Back);
                    return Some(v);
                }
                if self.focused == FocusedPane::Shell {
                    v.push(MenuItem::AiShellCmd);
                    v.push(MenuItem::AiExplainError);
                } else {
                    v.push(MenuItem::AiTriageLog);
                    v.push(MenuItem::AiCommit);
                }
                v.push(MenuItem::Back);
                Some(v)
            }
            MenuItem::SendMenu => {
                Some(vec![MenuItem::ScpUpload, MenuItem::ScpDownload, MenuItem::Back])
            }
            MenuItem::GitMenu => Some(vec![
                MenuItem::GitStage,
                MenuItem::GitUnstage,
                MenuItem::GitDiscard,
                MenuItem::GitDiff,
                MenuItem::GitHistory,
                MenuItem::Back,
            ]),
            MenuItem::SvnMenu => Some(vec![
                MenuItem::SvnAdd,
                MenuItem::SvnRevert,
                MenuItem::SvnResolve,
                MenuItem::SvnDiff,
                MenuItem::SvnLog,
                MenuItem::SvnUpdate,
                MenuItem::SvnCommit,
                MenuItem::Back,
            ]),
            MenuItem::WindowMenu => {
                let mut v = vec![
                    MenuItem::ShellSplitLR,
                    MenuItem::ShellSplitTB,
                    MenuItem::ShellNewTab,
                ];
                // Offer the close that matches what is active: a split pane if
                // this tab is split, otherwise the tab itself.
                if self.shell.active_pane_count() > 1 {
                    v.push(MenuItem::ShellCloseSplit);
                } else {
                    v.push(MenuItem::ShellCloseTab);
                }
                v.push(MenuItem::ShellZoom);
                v.push(MenuItem::Back);
                Some(v)
            }
            MenuItem::CompressMenu => Some(vec![
                MenuItem::CompressZip,
                MenuItem::CompressZipEnc,
                MenuItem::CompressTarGz,
                MenuItem::Back,
            ]),
            MenuItem::FileMenu => Some(vec![
                MenuItem::CopyToOther,
                MenuItem::MoveToOther,
                MenuItem::CopyToPath,
                MenuItem::BulkRename,
                MenuItem::EditorRename,
                MenuItem::Back,
            ]),
            MenuItem::ArchiveMenu => {
                let mut v = vec![MenuItem::CompressMenu];
                // "Extract here" only when the cursor is on an archive.
                let on_archive = self
                    .active_pane()
                    .and_then(|p| p.selected())
                    .map(|e| !e.is_dir && cian_core::archive::is_archive(&e.path))
                    .unwrap_or(false);
                if on_archive {
                    v.push(MenuItem::Extract);
                }
                v.push(MenuItem::Back);
                Some(v)
            }
            MenuItem::InspectMenu => Some(vec![
                MenuItem::Attributes,
                MenuItem::Hash,
                MenuItem::Compare,
                MenuItem::Count,
                MenuItem::DiskUsage,
                MenuItem::FindDupes,
                MenuItem::Back,
            ]),
            // The OS-native actions (#9): hand the selected file to the shell's
            // own verbs rather than reimplementing them.
            MenuItem::OsMenu => {
                let mut v = vec![MenuItem::OpenDefault, MenuItem::OpenWithOs];
                // The two Office entries only when there are libraries to
                // resolve against and the file is one of theirs — an item that
                // can only ever refuse is not worth the row.
                if !self.config.sharepoint.is_empty() && self.office_target_ok() {
                    v.push(MenuItem::OfficeOpen);
                    v.push(MenuItem::OfficeLink);
                }
                v.push(MenuItem::RevealInOs);
                v.push(MenuItem::PropertiesOs);
                v.push(MenuItem::Back);
                Some(v)
            }
            MenuItem::ViewMenu => {
                Some(vec![
                    MenuItem::HiddenToggle,
                    MenuItem::ThemePickPane,
                    MenuItem::TogglesMenu,
                    MenuItem::Back,
                ])
            }
            MenuItem::SessionMenu => {
                let mut v = Vec::new();
                if self.shell.active_session().map(|s| s.is_logging()).unwrap_or(false) {
                    v.push(MenuItem::StopLog);
                } else {
                    v.push(MenuItem::StartLog);
                }
                v.push(MenuItem::Encoding);
                v.push(MenuItem::Back);
                Some(v)
            }
            _ => None,
        }
    }

    /// Open a submenu, stashing the current menu so `Back`/Esc returns to it.
    pub(crate) fn open_submenu(&mut self, items: Vec<MenuItem>) {
        let at = match &self.popup {
            Popup::ContextMenu { at, .. } => *at,
            _ => (0, 0),
        };
        let parent = std::mem::replace(&mut self.popup, Popup::None);
        if matches!(parent, Popup::ContextMenu { .. }) {
            self.menu_stack.push(parent);
        }
        self.open_popup(Popup::ContextMenu { items, cursor: 0, at });
    }

    /// Go back up one menu level, or close the menu when at the top.
    pub(crate) fn menu_back(&mut self) {
        match self.menu_stack.pop() {
            Some(parent) => self.popup = parent,
            // Closing the top of the menu goes back to whatever it was opened
            // over — the viewer, when it was opened over one.
            None => self.dismiss_to_viewer(),
        }
    }

    /// Put the pane's targets into the file clipboard.
    pub(crate) fn clip_targets(&mut self, op: ClipOp) {
        let paths = self.target_paths();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        }
        let verb = match op {
            ClipOp::Copy => "copied",
            ClipOp::Cut => "cut",
        };
        self.message = Some(format!("{} {} item(s)", verb, paths.len()));
        self.file_clip = Some(FileClipboard { paths, op });
    }

    /// Paste the file clipboard into the focused pane's directory.
    pub(crate) fn paste_clip(&mut self) -> Result<()> {
        // cian's own register wins when set — it was filled deliberately, from
        // here. Otherwise fall back to the OS clipboard, so a file copied in
        // Explorer or Finder pastes as you would expect.
        let dest = match self.focused {
            FocusedPane::Shell => self.shell_cwd(),
            _ => match self.active_pane() {
                Some(p) => p.cwd.clone(),
                None => return Ok(()),
            },
        };
        let (paths, op, from_os) =
            match clip::plan(self.file_clip.as_ref(), os_clipboard_files, &dest) {
                clip::Paste::Empty => {
                    self.message = Some(tr(self.lang, "clipboard has no files", "クリップボードにファイルがありません").into());
                    return Ok(());
                }
                clip::Paste::AlreadyHere => {
                    self.message = Some(tr(self.lang, "already in this directory", "既にこのディレクトリです").into());
                    return Ok(());
                }
                clip::Paste::Go { paths, op, from_os } => (paths, op, from_os),
            };
        let report = match op {
            ClipOp::Copy => ops::copy_many(&paths, &dest, Conflict::Skip),
            ClipOp::Cut => ops::move_many(&paths, &dest, Conflict::Skip),
        };
        if !clip::survives(op) {
            self.file_clip = None;
        }
        self.reload_both();
        self.flash(self.focused);
        if from_os && report.errors.is_empty() {
            // Say where they came from, so "paste" is never ambiguous about
            // which of the two clipboards it just used.
            self.message =
                Some(format!("pasted {} item(s) from the system clipboard", report.ok));
            return Ok(());
        }
        self.show_op_report(&report);
        Ok(())
    }

    /// Reload both file panes; a paste can change either of them.
    pub(crate) fn reload_both(&mut self) {
        let _ = self.left.active_mut().reload();
        let _ = self.right.active_mut().reload();
        // A file op or refresh may have changed the working tree.
        self.invalidate_git();
    }

    pub(crate) fn run_menu_item(&mut self, item: MenuItem) -> Result<()> {
        // A group drills into its submenu; Back climbs out — neither acts.
        if let Some(children) = self.submenu_children(item) {
            self.open_submenu(children);
            return Ok(());
        }
        if item == MenuItem::Back {
            self.menu_back();
            return Ok(());
        }
        // A leaf action closes the whole (possibly nested) menu.
        self.menu_stack.clear();
        self.popup = Popup::None;
        match item {
            MenuItem::AiMenu | MenuItem::SendMenu | MenuItem::WindowMenu | MenuItem::GitMenu | MenuItem::SvnMenu | MenuItem::CompressMenu | MenuItem::FileMenu | MenuItem::ArchiveMenu | MenuItem::InspectMenu | MenuItem::OsMenu | MenuItem::ViewMenu | MenuItem::SessionMenu | MenuItem::Back => {} // handled above
            MenuItem::OpenDefault => self.open_externally(),
            MenuItem::OpenWithOs => self.open_with_os(),
            MenuItem::OfficeOpen => self.open_in_office(),
            MenuItem::OfficeLink => self.write_office_link(),
            MenuItem::RevealInOs => self.reveal_in_os(),
            MenuItem::PropertiesOs => self.properties_os(),
            MenuItem::CopyPathText => self.copy_paths_to_clipboard(),
            MenuItem::CopyFileRef => self.copy_file_refs_to_clipboard(),
            MenuItem::ShellSplitLR => {
                let cwd = self.shell_cwd();
                self.shell.split_active(&cwd, SplitDir::LeftRight);
                self.focus(FocusedPane::Shell);
            }
            MenuItem::ShellSplitTB => {
                let cwd = self.shell_cwd();
                self.shell.split_active(&cwd, SplitDir::TopBottom);
                self.focus(FocusedPane::Shell);
            }
            MenuItem::ShellNewTab => {
                let cwd = self.shell_cwd();
                self.shell.new_tab(&cwd);
                self.focus(FocusedPane::Shell);
            }
            MenuItem::ShellCloseSplit => {
                self.open_popup(Popup::ConfirmClose { target: CloseTarget::ShellPane });
            }
            MenuItem::ShellCloseTab => {
                if self.shell.close_active() {
                    self.focus(self.last_file_pane);
                }
            }
            MenuItem::ShellZoom => {
                self.focus(FocusedPane::Shell);
                // Don't fight a full-screen TUI running in the pane.
                if !self.shell.active_modes().0 {
                    self.toggle_zoom();
                }
            }
            MenuItem::SyncStart | MenuItem::SyncStop => {
                self.focus(FocusedPane::Shell);
                let on = self.shell.set_broadcast(matches!(item, MenuItem::SyncStart));
                self.message = Some(if on {
                    tr(self.lang, "⇄ synchronize ON. input goes to every pane", "⇄ 同時入力 ON。全ペインに入力されます").into()
                } else {
                    tr(self.lang, "synchronize off", "同時入力 OFF").into()
                });
            }
            MenuItem::SyncMember => {
                self.focus(FocusedPane::Shell);
                let n = self.shell.toggle_sync_member();
                let total = self.shell.active_pane_count();
                self.message = Some(if n == 0 {
                    tr(self.lang, "⇄ sync group cleared. every pane again", "⇄ 同時入力グループを解除しました。対象は全ペインです").into()
                } else {
                    format!("⇄ sync group: {n}/{total}")
                });
            }
            // From the viewer's menu, "Copy" means the selection in it; from a
            // pane's, the files.
            MenuItem::Copy if self.viewer_return.is_some() => {
                self.restore_viewer();
                self.copy_viewer_selection();
            }
            MenuItem::Copy => self.clip_targets(ClipOp::Copy),
            MenuItem::Cut => self.clip_targets(ClipOp::Cut),
            // In the shell, "Paste" means the text on the clipboard goes to
            // the running program — the terminal sense of paste. Only in a file
            // pane does it mean the file clipboard.
            MenuItem::Paste if self.focused == FocusedPane::Shell => self.paste_text_to_shell(),
            MenuItem::Paste => return self.paste_clip(),
            MenuItem::PasteHere => return self.paste_clip(),
            MenuItem::CommandInput => self.enter_command_mode(),
            MenuItem::CopyToOther => self.start_transfer(PendingOp::Copy),
            MenuItem::MoveToOther => self.start_transfer(PendingOp::Move),
            MenuItem::CopyToPath => self.start_dest_picker(PendingOp::Copy),
            MenuItem::Rename => self.start_rename(),
            MenuItem::Delete => self.start_delete(),
            MenuItem::Manual => self.open_manual(),
            MenuItem::Ssh => self.start_ssh(),
            MenuItem::ScpUpload => self.start_scp(ScpDir::Upload),
            MenuItem::ScpDownload => self.start_scp(ScpDir::Download),
            MenuItem::StartLog => self.start_log_prompt(),
            MenuItem::StopLog => self.stop_session_log(),
            MenuItem::AiChat => self.open_ai_chat(),
            MenuItem::AiShellCmd => self.start_ai_shell_prompt(),
            MenuItem::AiExplainError => self.explain_shell_error(),
            MenuItem::RevealInPane => {
                self.restore_viewer();
                self.viewer_reveal_in_pane();
            }
            MenuItem::AiWriting => self.ai_over_viewer(AiOverText::Writing),
            MenuItem::AiCommandHelp => self.ai_over_viewer(AiOverText::Command),
            MenuItem::AiCodeFix => self.ai_over_viewer(AiOverText::Code),
            MenuItem::AiCommit => self.start_ai_commit_message(),
            MenuItem::AiTriageLog => self.triage_log(),
            // With a `:rag` question behind it this needs no argument, so it
            // runs; otherwise it opens the prompt for one.
            MenuItem::ViewerSummary => {
                self.restore_viewer();
                self.summarize_viewer();
            }
            MenuItem::ViewerBlame => {
                self.restore_viewer();
                self.toggle_viewer_blame();
            }
            MenuItem::ViewerEncoding => {
                self.restore_viewer();
                self.start_viewer_encoding_pick();
            }
            MenuItem::ViewerMermaid => {
                self.restore_viewer();
                self.open_mermaid_in_browser();
            }
            // One arm for eleven, because they are one thing: the command the
            // `:` line would have run, run from here instead.
            MenuItem::ViewerSort
            | MenuItem::ViewerRsort
            | MenuItem::ViewerUniq
            | MenuItem::ViewerHan
            | MenuItem::ViewerZen
            | MenuItem::ViewerExpand
            | MenuItem::ViewerUnexpand
            | MenuItem::ViewerReindent
            | MenuItem::ViewerLf
            | MenuItem::ViewerCrlf => {
                let cmd = match item {
                    MenuItem::ViewerSort => "sort",
                    MenuItem::ViewerRsort => "rsort",
                    MenuItem::ViewerUniq => "uniq",
                    MenuItem::ViewerHan => "han",
                    MenuItem::ViewerZen => "zen",
                    MenuItem::ViewerExpand => "expand",
                    MenuItem::ViewerUnexpand => "unexpand",
                    MenuItem::ViewerReindent => "reindent",
                    MenuItem::ViewerLf => "lf",
                    _ => "crlf",
                };
                self.restore_viewer();
                self.run_substitute(cmd);
            }
            // The one that needs saying what to replace with.
            MenuItem::ViewerSubstitute => {
                self.restore_viewer();
                self.start_replace_bar();
            }
            MenuItem::ViewerLineMenu => {} // a heading; handled above
            MenuItem::ViewerEdit => {
                self.restore_viewer();
                self.edit_viewer_file_externally();
            }
            MenuItem::ViewerSave => {
                self.restore_viewer();
                self.save_viewer_file();
            }
            MenuItem::ViewerEditStyle => {
                self.restore_viewer();
                self.flip_edit_style();
            }
            MenuItem::ViewerCloseDiscard => {
                // Put it back before closing it: `close_viewer_file` works on
                // the panel in `popup`, and while the menu is up the panel
                // is not there.
                self.restore_viewer();
                self.close_viewer_file();
            }
            MenuItem::TogglesMenu => {
                // Not `restore_viewer` first: the toggles are a popup of
                // their own and would be written over by it. What was put
                // aside stays aside and comes back when they close.
                self.start_toggles();
            }
            MenuItem::RemotePane => self.start_scp(ScpDir::BrowsePane),
            MenuItem::DiskUsage => self.start_du_here(),
            MenuItem::GitStage => self.git_stage(),
            MenuItem::GitUnstage => self.git_unstage(),
            MenuItem::GitDiscard => self.git_discard_prompt(),
            MenuItem::GitHistory => self.start_git_log(),
            MenuItem::GitDiff => self.git_diff_file(),
            MenuItem::SvnAdd => self.git_stage(),
            MenuItem::SvnRevert => self.git_discard_prompt(),
            MenuItem::SvnResolve => self.svn_resolve(),
            MenuItem::SvnDiff => self.git_diff_file(),
            MenuItem::SvnLog => self.start_git_log(),
            MenuItem::SvnUpdate => self.svn_update(),
            MenuItem::SvnCommit => self.svn_commit_prompt(),
            MenuItem::BulkRename => self.start_bulk_rename(),
            MenuItem::EditorRename => self.start_editor_rename(),
            MenuItem::Snippets => self.start_snippets(),
            MenuItem::Macros => self.start_macros(),
            MenuItem::EditTab => self.edit_in_new_tab(None),
            MenuItem::ThemePick => self.start_theme_picker(),
            MenuItem::ThemePickPane => {
                let side = matches!(self.focused, FocusedPane::Right) as usize;
                self.start_pane_theme_picker(side);
            }
            MenuItem::Shortcuts => self.start_shortcuts(),
            MenuItem::Lang => {
                // Flip the interface language; every localized string reads
                // `self.lang` at draw time, so the next frame is fully in the
                // new language.
                self.lang = self.lang.toggled();
                // The menu and the manual read `menu_lang`, which is what this
                // switch used to leave behind: "Switch to English" put the
                // status line into English and left the menu it was chosen from
                // in Japanese. They follow now — unless init.lua asked for them
                // in a particular language, which is a choice and not an
                // oversight.
                if !self.menu_lang_pinned {
                    self.menu_lang = self.lang;
                }
                self.message = Some(match self.lang {
                    Lang::En => "language: English".into(),
                    Lang::Ja => "言語: 日本語".into(),
                });
            }
            MenuItem::Encoding => {
                match self.shell.active_session() {
                    Some(s) => {
                        let cur = cian_core::viewer::TextEncoding::ALL
                            .iter()
                            .position(|e| *e == s.encoding())
                            .unwrap_or(0);
                        self.popup =
                            Popup::EncodingPicker { cursor: cur, target: EncTarget::Shell };
                    }
                    None => self.message = Some(tr(self.lang, "no shell here", "ここにシェルがありません").into()),
                }
            }
            MenuItem::Quit => self.start_quit_confirm(),
            MenuItem::HiddenToggle => self.toggle_hidden(),
            MenuItem::Attributes => self.show_attributes(),
            MenuItem::Hash => self.start_hash(cian_core::attrs::HashKind::Sha256),
            MenuItem::Compare => self.open_diff(),
            MenuItem::FindDupes => self.start_dupes(),
            MenuItem::Count => self.start_count(),
            MenuItem::Extract => self.extract_selected(),
            MenuItem::CompressZip => self.prompt_compress(CompressKind::Zip),
            MenuItem::CompressZipEnc => self.prompt_compress(CompressKind::ZipEnc),
            MenuItem::CompressTarGz => self.prompt_compress(CompressKind::TarGz),
            MenuItem::Background => {
                let pane = self.focused;
                let current = match pane {
                    FocusedPane::Shell => self.shell.active_pane_bg(),
                    _ => Self::bg_slot(pane).and_then(|s| self.pane_bg[s]),
                };
                let cur = current
                    .and_then(|c| pane_bg_presets().iter().position(|(_, p)| *p == Some(c)))
                    .unwrap_or(0);
                self.open_popup(Popup::ColorPicker { pane, cursor: cur });
            }
        }
        Ok(())
    }
}
