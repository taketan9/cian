//! SSH / SFTP-SCP actions on the App: matching configured hosts, the host and
//! user pickers, kicking off a transfer, connecting a shell, and watching for
//! the password prompt. Split out of lib.rs as an `impl App` block.
use super::*;

/// A single remote-side mutation, run on a worker via [`App::remote_mut_spawn`].
pub(crate) enum RemoteMut {
    Mkdir(String),
    Touch(String),
    Rename { from: String, to: String },
    Remove { path: String, is_dir: bool },
}

impl App {
    /// Hosts matching the picker's current filter, as `(index, host)`.
    pub(crate) fn ssh_matches(&self, filter: &str) -> Vec<(usize, &cian_lua::SshHost)> {
        let needle = filter.to_lowercase();
        self.config
            .ssh_hosts
            .iter()
            .enumerate()
            .filter(|(_, h)| {
                needle.is_empty()
                    || h.name.to_lowercase().contains(&needle)
                    || h.host.to_lowercase().contains(&needle)
            })
            .collect()
    }

    pub(crate) fn start_ssh(&mut self) {
        // With nothing configured, go straight to typing a server by hand (#2)
        // rather than a dead-end notice.
        if self.config.ssh_hosts.is_empty() {
            self.start_manual_ssh();
            return;
        }
        self.open_popup(Popup::SshHosts { cursor: 0, filter: String::new() });
    }

    /// Begin an SFTP transfer: capture the local side, then reuse the SSH
    /// host/user picker to choose the server. `ssh_pick` routes back here once
    /// a user is chosen because [`App::scp_dir`] is set.
    pub(crate) fn start_scp(&mut self, dir: ScpDir) {
        // Works from the shell too, acting on the last-focused file pane.
        let pane = self.effective_file_pane();
        let (locals, local_dir) = match dir {
            ScpDir::Upload => {
                // **Folders too.** This filtered them out, so marking a folder
                // and choosing 送る ▸ アップロード answered "select a file to
                // upload" — as if nothing had been selected. The window build
                // has carried folders since `plan_upload` existed; the two
                // halves of one program disagreed about what "send this"
                // means, and the terminal said so in a way that read as a
                // mistake by the person.
                let files: Vec<PathBuf> = pane.target_paths();
                if files.is_empty() {
                    self.message = Some(tr(self.lang, "select something to upload", "アップロードするものを選んでください").into());
                    return;
                }
                (files, PathBuf::new())
            }
            ScpDir::Download | ScpDir::BrowsePane => (Vec::new(), pane.cwd.clone()),
        };
        self.scp_dir = Some((dir, locals, local_dir));
        // Nothing configured: type the server by hand (#2).
        if self.config.ssh_hosts.is_empty() {
            self.start_manual_ssh();
            return;
        }
        // From the shell, if it is logged into a configured host we can
        // authenticate, go straight to that server; otherwise show the picker.
        if self.focused == FocusedPane::Shell {
            if let Some((idx, user)) = self.connected_shell_host() {
                self.scp_after_pick(idx, &user);
                return;
            }
        }
        self.open_popup(Popup::SshHosts { cursor: 0, filter: String::new() });
    }

    /// The configured host+user the active shell is logged into, if its title is
    /// `user@host` for a host we have a usable (password-bearing) login for.
    fn connected_shell_host(&self) -> Option<(usize, String)> {
        let title = self.shell.active_title()?;
        let user = title.split('@').next()?.trim();
        if user.is_empty() {
            return None;
        }
        let host = host_from_title(&title)?;
        let idx = self.config.ssh_hosts.iter().position(|h| {
            (h.host == host || h.name == host)
                && h.users.iter().any(|u| u.name == user && u.has_secret())
        })?;
        Some((idx, user.to_string()))
    }

    /// After a host+user is picked for a transfer, resolve the connection and
    /// ask for the remote path.
    pub(crate) fn scp_after_pick(&mut self, host_idx: usize, user: &str) {
        let Some(h) = self.config.ssh_hosts.get(host_idx) else { return };
        let Some(u) = h.users.iter().find(|u| u.name == user) else { return };
        let Some(password) = u.secret() else {
            self.message = Some(format!(
                "no password set for {}@{} — a transfer needs one in init.lua",
                u.name, h.name
            ));
            return;
        };
        let target = cian_scp::Target {
            host: h.host.clone(),
            port: h.port.unwrap_or(22),
            user: u.name.clone(),
            password,
            key: u.key_path(),
            key_pass: u.key_pass.clone(),
        };
        let label = format!("{}@{}", u.name, h.name);
        self.scp_dispatch(target, label);
    }

    /// Kick off the transfer for a resolved `target`, whether it came from a
    /// configured host or was typed in manually. Consumes `scp_dir` (the pending
    /// local side + direction) set up in [`App::start_scp`].
    pub(crate) fn scp_dispatch(&mut self, target: cian_scp::Target, label: String) {
        let Some((dir, locals, _local_dir)) = self.scp_dir.take() else { return };
        match dir {
            ScpDir::Upload => {
                // Upload browses the server (WinSCP-style) to pick the
                // destination folder; the pending holds the local files to send.
                self.scp_pending = Some(ScpPending { target: target.clone(), label: label.clone(), locals });
                self.scp_target = Some((target, label.clone()));
                self.open_remote_browser(label, ".", BrowsePurpose::Upload);
            }
            ScpDir::Download => {
                // Download opens a remote browser: navigate, mark files, then
                // pick where they land locally.
                self.scp_target = Some((target, label.clone()));
                self.open_remote_browser(label, ".", BrowsePurpose::Download);
            }
            ScpDir::BrowsePane => {
                self.open_remote_pane(target, label);
            }
        }
    }

    /// Start typing a connection by hand from the host picker (#2): server, user,
    /// then password. `for_scp` remembers whether a transfer is being set up so
    /// the final step either kicks off the transfer or logs a shell in.
    pub(crate) fn start_manual_ssh(&mut self) {
        let for_scp = self.scp_dir.is_some();
        self.open_popup(text_input(
            "manual connection — server",
            "user@host  (e.g. root@10.0.1.5, or deploy@web1:2222):",
            String::new(),
            InputKind::ManualSshTarget { for_scp },
        ));
    }

    /// Second manual step: ask for the password for `user@host:port`.
    pub(crate) fn manual_ssh_password(&mut self, user: String, host: String, port: u16, for_scp: bool) {
        self.open_popup(text_input(
            "manual connection — password",
            format!("password for {user}@{host} (blank = none):"),
            String::new(),
            InputKind::ManualSshPass { user, host, port, for_scp },
        ));
    }

    /// Final manual step: build the connection and either run the transfer or log
    /// the shell in (typing `ssh …` and feeding the password on the prompt).
    pub(crate) fn manual_ssh_finish(&mut self, user: String, host: String, port: u16, password: String, for_scp: bool) {
        let label = format!("{user}@{host}");
        if for_scp {
            if password.is_empty() {
                self.message = Some(tr(self.lang, "a transfer needs a password", "転送にはパスワードが必要です").into());
                self.scp_dir = None;
                return;
            }
            // Typed by hand, so no key: the place to keep one is `init.lua`.
            let target =
                cian_scp::Target { host, port, user, password, key: None, key_pass: None };
            self.scp_dispatch(target, label);
            return;
        }
        // Plain shell login: type the command, then feed the password (if any) on
        // the prompt via the existing pending-auth watcher.
        let mut cmd = format!("ssh {user}@{host}");
        if port != 22 {
            cmd.push_str(&format!(" -p {port}"));
        }
        self.popup = Popup::None;
        self.run_in_shell(cmd);
        if password.is_empty() {
            self.message = Some(format!("→ {label}"));
        } else {
            self.pending_auth = Some(PendingAuth { secret: password, deadline: Instant::now() + AUTH_WINDOW });
            self.message = Some(format!("→ {label} (sending password on prompt)"));
        }
    }

    /// Open the remote file browser at `cwd` and kick off its listing.
    pub(crate) fn open_remote_browser(&mut self, label: String, cwd: &str, purpose: BrowsePurpose) {
        self.open_popup(Popup::RemoteBrowser {
            label,
            cwd: cwd.to_string(),
            entries: Vec::new(),
            cursor: 0,
            scroll: 0,
            marked: std::collections::BTreeSet::new(),
            loading: true,
            purpose,
        });
        self.remote_ls_spawn(cwd.to_string());
    }

    /// List remote directory `path` on a worker thread; the result lands in
    /// [`App::poll_remote_ls`].
    fn remote_ls_spawn(&mut self, path: String) {
        let Some((target, _)) = self.scp_target.clone() else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // list_dir returns the canonical absolute path of `path`, which
            // becomes the browser's cwd so parent navigation can climb to "/".
            let res = cian_scp::list_dir(&target, &path).map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        self.remote_ls = Some(rx);
    }

    /// Install a finished remote listing into the open browser. Returns true if
    /// anything changed (so the caller repaints).
    pub(crate) fn poll_remote_ls(&mut self) -> bool {
        let Some(rx) = &self.remote_ls else { return false };
        match rx.try_recv() {
            Ok(result) => {
                self.remote_ls = None;
                match result {
                    Ok((cwd_new, mut entries)) => {
                        // A ".." row to step up one level, like the file panes —
                        // except at the filesystem root, where there is no up.
                        if cwd_new != "/" {
                            entries.insert(0, cian_scp::RemoteEntry { name: "..".into(), is_dir: true, size: 0, link: false });
                        }
                        if let Popup::RemoteBrowser { cwd, entries: es, cursor, scroll, loading, marked, .. } =
                            &mut self.popup
                        {
                            *cwd = cwd_new;
                            *es = entries;
                            *cursor = 0;
                            *scroll = 0;
                            *loading = false;
                            marked.clear();
                        }
                    }
                    Err(e) => {
                        self.popup = Popup::None;
                        self.scp_target = None;
                        self.message = Some(format!("remote listing failed: {}", e));
                    }
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.remote_ls = None;
                true
            }
        }
    }

    /// Enter the highlighted remote entry: descend into a directory, or mark a
    /// file and move on (Enter on a file selects it for download).
    pub(crate) fn remote_browser_enter(&mut self) {
        let (dir_to, is_dir_name) = {
            let Popup::RemoteBrowser { cwd, entries, cursor, purpose, .. } = &self.popup else { return };
            let Some(e) = entries.get(*cursor) else { return };
            if e.is_dir && e.name == ".." {
                // The synthetic up-row: climb to the parent (cwd is absolute).
                (Some(parent_remote(cwd)), None)
            } else if e.is_dir {
                (Some(join_remote(cwd, &e.name)), None)
            } else if *purpose == BrowsePurpose::Upload {
                // Uploading picks a *folder*; a file under the cursor is a no-op.
                (None, None)
            } else {
                (None, Some(e.name.clone()))
            }
        };
        if let Some(path) = dir_to {
            if let Popup::RemoteBrowser { loading, .. } = &mut self.popup {
                *loading = true;
            }
            self.remote_ls_spawn(path);
        } else if let Some(name) = is_dir_name {
            if let Popup::RemoteBrowser { marked, cursor, entries, .. } = &mut self.popup {
                if !marked.insert(name.clone()) {
                    marked.remove(&name);
                }
                *cursor = (*cursor + 1).min(entries.len().saturating_sub(1));
            }
        }
    }

    /// Go to the parent of the current remote directory.
    pub(crate) fn remote_browser_parent(&mut self) {
        let parent = if let Popup::RemoteBrowser { cwd, .. } = &self.popup {
            parent_remote(cwd)
        } else {
            return;
        };
        if let Popup::RemoteBrowser { loading, .. } = &mut self.popup {
            *loading = true;
        }
        self.remote_ls_spawn(parent);
    }

    /// Toggle the mark on the highlighted file (directories can't be marked).
    pub(crate) fn remote_browser_mark(&mut self) {
        if let Popup::RemoteBrowser { entries, cursor, marked, .. } = &mut self.popup {
            if let Some(e) = entries.get(*cursor) {
                if !e.is_dir && !marked.insert(e.name.clone()) {
                    marked.remove(&e.name);
                }
            }
            *cursor = (*cursor + 1).min(entries.len().saturating_sub(1));
        }
    }

    // ── remote pane (a persistent SFTP-backed file pane) ──────────────────────

    /// The file-pane side to open a remote pane on (the focused one, or the last
    /// file pane when the shell is focused).
    fn remote_side(&self) -> FocusedPane {
        match self.focused {
            FocusedPane::Left | FocusedPane::Right => self.focused,
            _ => self.last_file_pane,
        }
    }

    fn side_idx(side: FocusedPane) -> usize {
        usize::from(matches!(side, FocusedPane::Right))
    }

    fn side_tabs_mut(&mut self, side: FocusedPane) -> &mut PaneTabs {
        if matches!(side, FocusedPane::Right) { &mut self.right } else { &mut self.left }
    }

    /// Open `target` as a **remote pane** on the active file side: browse the
    /// server like a local pane, starting at the login directory.
    pub(crate) fn open_remote_pane(&mut self, target: cian_scp::Target, label: String) {
        let side = self.remote_side();
        self.remote_targets[Self::side_idx(side)] = Some((target, label.clone()));
        self.focus(side);
        self.message = Some(format!("⇅ connecting to {label} …"));
        self.remote_pane_ls_spawn(side, ".".to_string());
    }

    /// List remote directory `path` for the remote pane on `side`, on a worker
    /// thread; the result lands in [`App::poll_remote_pane_ls`].
    pub(crate) fn remote_pane_ls_spawn(&mut self, side: FocusedPane, path: String) {
        let Some((target, _)) = self.remote_targets[Self::side_idx(side)].clone() else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(cian_scp::list_dir(&target, &path).map_err(|e| e.to_string()));
        });
        self.remote_pane_ls = Some((side, rx));
    }

    /// Install a finished remote-pane listing into its pane. Returns true to
    /// repaint.
    pub(crate) fn poll_remote_pane_ls(&mut self) -> bool {
        let Some((side, rx)) = &self.remote_pane_ls else { return false };
        let side = *side;
        match rx.try_recv() {
            Ok(result) => {
                self.remote_pane_ls = None;
                match result {
                    Ok((cwd, remotes)) => {
                        let label = self.remote_targets[Self::side_idx(side)]
                            .as_ref()
                            .map(|(_, l)| l.clone())
                            .unwrap_or_default();
                        // A ".." up-row (except at the filesystem root), then the
                        // entries — each carrying its remote absolute path.
                        let mut entries = Vec::with_capacity(remotes.len() + 1);
                        if cwd != "/" {
                            entries.push(cian_core::Entry::remote("..", parent_remote(&cwd), true, 0, true));
                        }
                        for e in remotes {
                            let full = join_remote(&cwd, &e.name);
                            entries.push(cian_core::Entry::remote(e.name, full, e.is_dir, e.size, false));
                        }
                        self.side_tabs_mut(side).active_mut().enter_remote(label, cwd, entries);
                        self.message = Some(tr(
                            self.lang,
                            "remote pane — Enter/l open, - up, c copy, m move, A/a/r/d write, Esc close",
                            "リモートペイン — Enter/l 開く, - 上, c コピー, m 移動, A/a/r/d 書込, Esc 閉じる",
                        ).into());
                    }
                    Err(e) => {
                        // The pane keeps its connection. One directory that
                        // cannot be read — no permission, or it went away — is
                        // not a reason to forget where the whole pane is
                        // connected to: dropping the target left a remote pane
                        // on screen whose every key silently did nothing, which
                        // looks exactly like cian ignoring the keyboard.
                        let first = e.lines().next().unwrap_or_default().to_string();
                        self.message = Some(format!("✖ {first}"));
                    }
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.remote_pane_ls = None;
                true
            }
        }
    }

    /// Enter the highlighted remote entry: descend a directory, climb via `..`,
    /// read a file.
    ///
    /// Enter means the same thing on both sides of the network. On a local pane
    /// it opens a directory or reads a file in the docked viewer; here it did
    /// the first and silently ignored the second, so a remote file could only be
    /// opened by double-clicking it — a keyboard-first program answering only to
    /// the mouse. The file is fetched to a temp copy and read there, which is
    /// what F3 on a remote pane has always done.
    pub(crate) fn remote_pane_enter(&mut self) {
        let side = self.remote_side();
        let path = {
            let Some(pane) = self.active_pane() else { return };
            let Some((_, cwd)) = pane.remote_view() else { return };
            let cwd = cwd.to_string();
            let Some(e) = pane.selected() else { return };
            if e.is_parent {
                parent_remote(&cwd)
            } else if e.is_dir {
                // `path` holds the remote absolute path built at listing time.
                e.path.to_string_lossy().into_owned()
            } else {
                self.look_inside();
                return;
            }
        };
        self.message = Some(format!("⇅ {path} …"));
        self.remote_pane_ls_spawn(side, path);
    }

    /// Go to the parent of the remote pane's current directory.
    pub(crate) fn remote_pane_parent(&mut self) {
        let side = self.remote_side();
        let Some(cwd) = self.active_pane().and_then(|p| p.remote_view()).map(|(_, c)| c.to_string())
        else {
            return;
        };
        self.remote_pane_ls_spawn(side, parent_remote(&cwd));
    }

    /// The active pane on a given side (read-only).
    pub(crate) fn side_pane(&self, side: FocusedPane) -> &Pane {
        if matches!(side, FocusedPane::Right) { self.right.active_ref() } else { self.left.active_ref() }
    }

    /// The active pane on a given side, mutably.
    pub(crate) fn side_pane_mut(&mut self, side: FocusedPane) -> &mut Pane {
        if matches!(side, FocusedPane::Right) { self.right.active_mut() } else { self.left.active_mut() }
    }

    /// The absolute remote cwd of the remote pane on `side` (if it is one).
    fn remote_cwd(&self, side: FocusedPane) -> Option<String> {
        self.side_pane(side).remote_view().map(|(_, p)| p.to_string())
    }

    /// `A` in a remote pane: prompt for a new directory name (matches the local
    /// pane's mkdir key).
    pub(crate) fn remote_pane_mkdir(&mut self) {
        let side = self.focused;
        if self.remote_cwd(side).is_none() {
            return;
        }
        self.open_popup(text_input(
            tr(self.lang, "New remote folder", "リモート: 新規ディレクトリ"),
            tr(self.lang, "name:", "名前:"),
            String::new(),
            InputKind::RemoteMkdir { side },
        ));
    }

    /// `a` in a remote pane: prompt for a new (empty) file name.
    pub(crate) fn remote_pane_touch(&mut self) {
        let side = self.focused;
        if self.remote_cwd(side).is_none() {
            return;
        }
        self.open_popup(text_input(
            tr(self.lang, "New remote file", "リモート: 新規ファイル"),
            tr(self.lang, "name:", "名前:"),
            String::new(),
            InputKind::RemoteTouch { side },
        ));
    }

    /// `r` in a remote pane: prompt to rename the entry under the cursor.
    pub(crate) fn remote_pane_rename(&mut self) {
        let side = self.focused;
        let Some(e) = self.active_pane().and_then(|p| p.selected()) else { return };
        if e.is_parent {
            return;
        }
        let from = e.path.to_string_lossy().into_owned();
        let name = e.name.clone();
        self.open_popup(text_input(
            tr(self.lang, "Rename remote", "リモート: リネーム"),
            tr(self.lang, "new name:", "新しい名前:"),
            name,
            InputKind::RemoteRename { side, from },
        ));
    }

    /// `d` in a remote pane: confirm, then delete the entry under the cursor.
    pub(crate) fn remote_pane_delete(&mut self) {
        let side = self.focused;
        let Some(e) = self.active_pane().and_then(|p| p.selected()) else { return };
        if e.is_parent {
            return;
        }
        self.open_popup(Popup::ConfirmRemoteDelete {
            side,
            path: e.path.to_string_lossy().into_owned(),
            name: e.name.clone(),
            is_dir: e.is_dir,
        });
    }

    /// Confirmed remote delete: run it on the worker.
    pub(crate) fn confirm_remote_delete(&mut self) {
        if let Popup::ConfirmRemoteDelete { side, path, is_dir, .. } =
            std::mem::replace(&mut self.popup, Popup::None)
        {
            self.remote_mut_spawn(side, RemoteMut::Remove { path, is_dir });
        }
    }

    /// Run a remote mutation on a worker thread; [`App::poll_remote_mut`] installs
    /// the result and re-lists the pane.
    pub(crate) fn remote_mut_spawn(&mut self, side: FocusedPane, op: RemoteMut) {
        let Some((target, _)) = self.remote_targets[Self::side_idx(side)].clone() else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let res: Result<&str, String> = match op {
                RemoteMut::Mkdir(p) => cian_scp::make_dir(&target, &p).map(|_| "created folder").map_err(|e| e.to_string()),
                RemoteMut::Touch(p) => cian_scp::make_file(&target, &p).map(|_| "created file").map_err(|e| e.to_string()),
                RemoteMut::Rename { from, to } => cian_scp::rename(&target, &from, &to).map(|_| "renamed").map_err(|e| e.to_string()),
                RemoteMut::Remove { path, is_dir } => cian_scp::remove(&target, &path, is_dir).map(|_| "deleted").map_err(|e| e.to_string()),
            };
            let _ = tx.send(res.map(str::to_string));
        });
        self.remote_mut = Some((side, rx));
        self.message = Some(tr(self.lang, "remote: working…", "リモート: 実行中…").into());
    }

    /// Install a finished remote mutation and re-list the pane. Returns true to
    /// repaint.
    pub(crate) fn poll_remote_mut(&mut self) -> bool {
        let Some((side, rx)) = &self.remote_mut else { return false };
        let side = *side;
        match rx.try_recv() {
            Ok(res) => {
                self.remote_mut = None;
                match res {
                    Ok(what) => {
                        self.message = Some(format!("remote: {what}"));
                        if let Some(cwd) = self.remote_cwd(side) {
                            self.remote_pane_ls_spawn(side, cwd);
                        }
                    }
                    Err(e) => self.message = Some(format!("remote failed: {e}")),
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.remote_mut = None;
                true
            }
        }
    }

    /// If a copy (`c`) crosses the local/remote boundary, run it as an SFTP
    /// transfer and return true. Local→remote uploads the marked files to the
    /// remote pane's directory; remote→local downloads them. `move` and
    /// remote↔remote are declined with a message (still "handled").
    pub(crate) fn try_remote_pane_transfer(&mut self, is_move: bool) -> bool {
        let active = self.remote_side();
        let opp = if matches!(active, FocusedPane::Right) { FocusedPane::Left } else { FocusedPane::Right };
        let a_remote = self.side_pane(active).is_remote();
        let o_remote = self.side_pane(opp).is_remote();
        if !a_remote && !o_remote {
            return false; // purely local — let the normal copy handle it
        }
        if is_move {
            // A host-crossing move: copy across, then delete the source. Build a
            // plan for whichever direction and confirm before touching anything.
            let files: Vec<String> = self
                .side_pane(active)
                .target_paths()
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            if files.is_empty() {
                self.message = Some(tr(self.lang, "nothing to move", "移動対象なし").into());
                return true;
            }
            let dst_dir = if o_remote {
                self.side_pane(opp).remote_view().map(|(_, p)| p.to_string())
            } else {
                Some(self.side_pane(opp).cwd.display().to_string())
            };
            let Some(dst_dir) = dst_dir else { return true };
            let src_target = self.remote_targets[Self::side_idx(active)].clone();
            let dst_target = self.remote_targets[Self::side_idx(opp)].clone();
            let from = src_target.as_ref().map(|(_, l)| l.clone()).unwrap_or_else(|| "local".into());
            let to = dst_target.as_ref().map(|(_, l)| l.clone()).unwrap_or_else(|| "local".into());
            self.open_popup(Popup::ConfirmRemoteMove {
                plan: RemoteMovePlan {
                    files,
                    src_target: src_target.map(|(t, _)| t),
                    dst_target: dst_target.map(|(t, _)| t),
                    dst_dir,
                },
                from,
                to,
            });
            return true;
        }
        if a_remote && !o_remote {
            // Download: the remote pane's marked entries → the local pane's dir.
            let files: Vec<String> =
                self.side_pane(active).target_paths().iter().map(|p| p.to_string_lossy().into_owned()).collect();
            if files.is_empty() {
                self.message = Some(tr(self.lang, "nothing to copy", "コピー対象なし").into());
                return true;
            }
            let local_dir = self.side_pane(opp).cwd.clone();
            if let Some((target, label)) = self.remote_targets[Self::side_idx(active)].clone() {
                self.scp_target = Some((target, label));
                self.start_remote_download(files, local_dir, None);
            }
            return true;
        }
        if !a_remote && o_remote {
            // Upload: the local pane's marked files → the remote pane's dir.
            let locals: Vec<PathBuf> = self.side_pane(active).target_paths();
            if locals.is_empty() {
                self.message = Some(tr(self.lang, "select something to upload", "アップロードするものを選択").into());
                return true;
            }
            let rcwd = self.side_pane(opp).remote_view().map(|(_, p)| p.to_string());
            if let (Some(rcwd), Some((target, label))) =
                (rcwd, self.remote_targets[Self::side_idx(opp)].clone())
            {
                self.scp_pending = Some(ScpPending { target, label, locals });
                self.run_scp_upload(rcwd);
                // Re-list the remote pane once the upload lands so the new files show.
                self.remote_refresh = Some(opp);
            }
            return true;
        }
        // Both remote — relay each file through this machine (there is no
        // server-to-server SFTP; a segmented enterprise network often can't do
        // direct A→B anyway).
        let files: Vec<String> = self
            .side_pane(active)
            .target_paths()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        if files.is_empty() {
            self.message = Some(tr(self.lang, "nothing to copy", "コピー対象なし").into());
            return true;
        }
        let dst_dir = self.side_pane(opp).remote_view().map(|(_, p)| p.to_string());
        let src = self.remote_targets[Self::side_idx(active)].clone();
        let dst = self.remote_targets[Self::side_idx(opp)].clone();
        if let (Some(dst_dir), Some((s_target, _)), Some((d_target, d_label))) = (dst_dir, src, dst) {
            self.message = Some(format!(
                "{} → {} …",
                tr(self.lang, "copying via this machine", "この端末を経由してコピー"),
                d_label
            ));
            // Re-list the destination pane once the relay finishes.
            self.remote_refresh = Some(opp);
            self.start_remote_to_remote(files, s_target, d_target, dst_dir);
        }
        true
    }

    /// Copy files from one server to another by relaying each through a local
    /// temp file: download from the source, upload to the destination, delete
    /// the temp. Runs on the file-op worker so it shows progress and the
    /// done-notification, and honours cancel between files.
    pub(crate) fn start_remote_to_remote(
        &mut self,
        files: Vec<String>,
        src: cian_scp::Target,
        dst: cian_scp::Target,
        dst_dir: String,
    ) {
        let limit = self.transfer_limit;
        self.start_op("copying", move |ctl| {
            let mut report = OpReport::default();
            let cancel = ctl.cancel;
            let total = files.len();
            let tmp_dir = std::env::temp_dir().join("cian-relay");
            let _ = std::fs::create_dir_all(&tmp_dir);
            for (i, remote) in files.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let fname = remote.rsplit('/').next().unwrap_or("file").to_string();
                let tmp = tmp_dir.join(&fname);
                // Stage 1: download source → local temp.
                let dl = {
                    let cur = format!("↓ {fname}");
                    let mut fwd = |done: u64, tot: u64| {
                        (ctl.on_progress)(&cian_core::progress::Progress {
                            bytes_done: done,
                            bytes_total: tot,
                            files_done: i,
                            files_total: total,
                            current: cur.clone(),
                        });
                    };
                    let mut sctl = cian_scp::Ctl { cancel, on_progress: &mut fwd, limit_bps: limit };
                    cian_scp::download(&src, remote, &tmp, &mut sctl)
                };
                if let Err(e) = dl {
                    report.note_error(format!("{fname}: download: {e}"));
                    let _ = std::fs::remove_file(&tmp);
                    continue;
                }
                // Stage 2: upload local temp → destination dir.
                let dst_path = join_remote(&dst_dir, &fname);
                let up = {
                    let cur = format!("↑ {fname}");
                    let mut fwd = |done: u64, tot: u64| {
                        (ctl.on_progress)(&cian_core::progress::Progress {
                            bytes_done: done,
                            bytes_total: tot,
                            files_done: i,
                            files_total: total,
                            current: cur.clone(),
                        });
                    };
                    let mut sctl = cian_scp::Ctl { cancel, on_progress: &mut fwd, limit_bps: limit };
                    cian_scp::upload(&dst, &tmp, &dst_path, None, &mut sctl)
                };
                match up {
                    Ok(_) => report.ok += 1,
                    Err(e) => report.note_error(format!("{fname}: upload: {e}")),
                }
                let _ = std::fs::remove_file(&tmp);
            }
            let _ = std::fs::remove_dir_all(&tmp_dir);
            report.note = Some("relayed via this machine".to_string());
            report
        });
    }

    /// Confirmed host-crossing move: copy each file across (upload / download /
    /// relay, depending on which end is local), then delete the source. Runs on
    /// the op worker; the remote panes re-list when it lands.
    pub(crate) fn confirm_remote_move(&mut self) {
        let Popup::ConfirmRemoteMove { plan, .. } =
            std::mem::replace(&mut self.popup, Popup::None)
        else {
            return;
        };
        // Refresh whichever remote pane loses files (the source), else the dest.
        self.remote_refresh = Some(if plan.src_target.is_some() {
            self.remote_side()
        } else if matches!(self.focused, FocusedPane::Right) {
            FocusedPane::Left
        } else {
            FocusedPane::Right
        });
        let RemoteMovePlan { files, src_target, dst_target, dst_dir } = plan;
        let limit = self.transfer_limit;
        self.start_op("moving", move |ctl| {
            let mut report = OpReport::default();
            let cancel = ctl.cancel;
            let total = files.len();
            let tmp_dir = std::env::temp_dir().join("cian-relay");
            let _ = std::fs::create_dir_all(&tmp_dir);
            for (i, src) in files.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let fname = src.trim_end_matches('/').rsplit(['/', '\\']).next().unwrap_or("file").to_string();
                // Transfer the file across, by direction.
                let moved: Result<(), String> = match (&src_target, &dst_target) {
                    (Some(s), Some(d)) => {
                        // remote → remote: relay through a local temp.
                        let tmp = tmp_dir.join(&fname);
                        let dl = {
                            let mut fwd = |done: u64, tot: u64| {
                                (ctl.on_progress)(&cian_core::progress::Progress { bytes_done: done, bytes_total: tot, files_done: i, files_total: total, current: format!("↓ {fname}") });
                            };
                            let mut sctl = cian_scp::Ctl { cancel, on_progress: &mut fwd, limit_bps: limit };
                            cian_scp::download(s, src, &tmp, &mut sctl)
                        };
                        let r = dl.map_err(|e| format!("download: {e}")).and_then(|_| {
                            let dst_path = join_remote(&dst_dir, &fname);
                            let mut fwd = |done: u64, tot: u64| {
                                (ctl.on_progress)(&cian_core::progress::Progress { bytes_done: done, bytes_total: tot, files_done: i, files_total: total, current: format!("↑ {fname}") });
                            };
                            let mut sctl = cian_scp::Ctl { cancel, on_progress: &mut fwd, limit_bps: limit };
                            cian_scp::upload(d, &tmp, &dst_path, None, &mut sctl).map(|_| ()).map_err(|e| format!("upload: {e}"))
                        });
                        let _ = std::fs::remove_file(&tmp);
                        r
                    }
                    (Some(s), None) => {
                        // remote → local: download into the local dir.
                        let dst = std::path::Path::new(&dst_dir).join(&fname);
                        let mut fwd = |done: u64, tot: u64| {
                            (ctl.on_progress)(&cian_core::progress::Progress { bytes_done: done, bytes_total: tot, files_done: i, files_total: total, current: format!("↓ {fname}") });
                        };
                        let mut sctl = cian_scp::Ctl { cancel, on_progress: &mut fwd, limit_bps: limit };
                        cian_scp::download(s, src, &dst, &mut sctl).map(|_| ()).map_err(|e| e.to_string())
                    }
                    (None, Some(d)) => {
                        // local → remote: upload to the remote dir.
                        let dst_path = join_remote(&dst_dir, &fname);
                        let mut fwd = |done: u64, tot: u64| {
                            (ctl.on_progress)(&cian_core::progress::Progress { bytes_done: done, bytes_total: tot, files_done: i, files_total: total, current: format!("↑ {fname}") });
                        };
                        let mut sctl = cian_scp::Ctl { cancel, on_progress: &mut fwd, limit_bps: limit };
                        cian_scp::upload(d, std::path::Path::new(src), &dst_path, None, &mut sctl).map(|_| ()).map_err(|e| e.to_string())
                    }
                    (None, None) => Err("both ends local".into()),
                };
                // Delete the source only if the copy succeeded.
                match moved {
                    Ok(()) => {
                        let del = match &src_target {
                            Some(s) => cian_scp::remove(s, src, false).map_err(|e| e.to_string()),
                            None => std::fs::remove_file(src).map_err(|e| e.to_string()),
                        };
                        match del {
                            Ok(()) => report.ok += 1,
                            Err(e) => report.note_error(format!("{fname}: copied but source not removed: {e}")),
                        }
                    }
                    Err(e) => report.note_error(format!("{fname}: {e}")),
                }
            }
            let _ = std::fs::remove_dir_all(&tmp_dir);
            report
        });
    }

    /// F3 on a remote pane: fetch the file under the cursor to a temp path on a
    /// worker thread, then open it in the viewer when it lands. Returns true if
    /// it started a fetch (so the caller skips the local viewer).
    pub(crate) fn remote_pane_view(&mut self) -> bool {
        let side = self.remote_side();
        let (remote_path, name) = {
            let Some(e) = self.side_pane(side).selected() else { return false };
            if e.is_dir || e.is_parent {
                return false;
            }
            (e.path.to_string_lossy().into_owned(), e.name.clone())
        };
        let Some((target, _)) = self.remote_targets[Self::side_idx(side)].clone() else { return false };
        // A stable temp name per remote file (its basename), overwritten each view.
        let base = std::path::Path::new(&name).file_name().map(|s| s.to_os_string()).unwrap_or_default();
        let temp = std::env::temp_dir().join("cian-remote").join(base);
        if let Some(dir) = temp.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Remember where this temp came from, so saving it uploads back.
        self.remote_edits.insert(temp.clone(), (target.clone(), remote_path.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        let temp_worker = temp.clone();
        let limit = self.transfer_limit;
        std::thread::spawn(move || {
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let mut prog = |_: u64, _: u64| {};
            let mut ctl = cian_scp::Ctl { cancel: &cancel, on_progress: &mut prog, limit_bps: limit };
            let r = cian_scp::download(&target, &remote_path, &temp_worker, &mut ctl)
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = tx.send(r);
        });
        self.remote_view = Some(RemoteView { rx, temp, name: name.clone() });
        self.message = Some(format!("⇅ fetching {name} …"));
        true
    }

    /// Install a finished remote-file fetch: open it in the viewer. Returns true
    /// to repaint.
    pub(crate) fn poll_remote_view(&mut self) -> bool {
        let Some(rv) = &self.remote_view else { return false };
        match rv.rx.try_recv() {
            Ok(result) => {
                let RemoteView { temp, name, .. } = self.remote_view.take().unwrap();
                match result {
                    Ok(()) => {
                        self.message = None;
                        self.open_viewer_at(&temp, &name, 0);
                    }
                    Err(e) => self.message = Some(format!("remote view failed: {e}")),
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.remote_view = None;
                true
            }
        }
    }

    /// If `local` is a temp file that was opened from a remote pane (F3), upload
    /// it back to where it came from. Called after a save (built-in editor) or
    /// after the external editor exits. Reuses the `remote_mut` channel so it
    /// doesn't disturb the open viewer.
    pub(crate) fn reupload_remote(&mut self, local: &std::path::Path) {
        let Some((target, remote)) = self.remote_edits.get(local).cloned() else { return };
        if self.remote_mut.is_some() {
            return; // one remote op at a time
        }
        let local = local.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        let limit = self.transfer_limit;
        std::thread::spawn(move || {
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let mut prog = |_: u64, _: u64| {};
            let mut sctl = cian_scp::Ctl { cancel: &cancel, on_progress: &mut prog, limit_bps: limit };
            let r = cian_scp::upload(&target, &local, &remote, None, &mut sctl)
                .map(|_| "uploaded your edit to the server")
                .map_err(|e| e.to_string());
            let _ = tx.send(r.map(str::to_string));
        });
        // side = the focused pane; poll_remote_mut re-lists it if it's remote.
        self.remote_mut = Some((self.focused, rx));
        self.message =
            Some(tr(self.lang, "remote: uploading your edit…", "リモート: 編集をアップロード中…").into());
    }

    /// Leave the remote pane, returning it to its local directory.
    pub(crate) fn leave_remote_pane(&mut self) {
        let side = self.remote_side();
        if let Some(p) = self.active_pane_mut() {
            let _ = p.leave_flat();
        }
        self.remote_targets[Self::side_idx(side)] = None;
    }

    /// Confirm the remote selection (marked files, else the file under the
    /// cursor) and move on to choose the local destination.
    pub(crate) fn remote_browser_download(&mut self) {
        let files: Vec<String> = if let Popup::RemoteBrowser { cwd, entries, cursor, marked, .. } = &self.popup {
            if !marked.is_empty() {
                marked.iter().map(|n| join_remote(cwd, n)).collect()
            } else {
                match entries.get(*cursor).filter(|e| !e.is_dir) {
                    Some(e) => vec![join_remote(cwd, &e.name)],
                    None => Vec::new(),
                }
            }
        } else {
            return;
        };
        if files.is_empty() {
            self.message = Some(tr(self.lang, "mark a file (Space) or put the cursor on one", "ファイルをマーク（Space）するか、カーソルを合わせてください").into());
            return;
        }
        self.open_popup(Popup::LocalDest { files, cursor: 0 });
    }

    /// Confirm the current remote directory as the upload destination and move on
    /// to the chmod prompts. The pending upload (target + local files) is already
    /// captured; we only needed the folder. Each file is asked for its own mode.
    pub(crate) fn remote_browser_upload_here(&mut self) {
        let cwd = if let Popup::RemoteBrowser { cwd, purpose: BrowsePurpose::Upload, .. } = &self.popup {
            cwd.clone()
        } else {
            return;
        };
        self.scp_target = None; // done browsing; the upload runs off scp_pending
        self.scp_upload_modes.clear();
        self.prompt_upload_chmod(cwd, 0);
    }

    /// Ask for the `idx`-th pending file's upload mode (one prompt per file, so
    /// each can differ). Once every file has a mode, kick off the upload. `Enter`
    /// on the seeded value reuses the previous file's mode, so accepting the same
    /// mode for all is just repeated Enters.
    pub(crate) fn prompt_upload_chmod(&mut self, remote: String, idx: usize) {
        let Some(p) = self.scp_pending.as_ref() else { return };
        let n = p.locals.len();
        if idx >= n {
            self.run_scp_upload(remote);
            return;
        }
        let fname = p.locals[idx]
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Seed with the previous file's mode (or 777) so repeating is one keypress.
        let seed = self
            .scp_upload_modes
            .last()
            .and_then(|m| *m)
            .map(|m| format!("{m:o}"))
            .unwrap_or_else(|| "777".to_string());
        self.open_popup(text_input(
            format!("upload chmod — {}/{}", idx + 1, n),
            format!("mode for {fname} (octal e.g. 777; blank = keep server default):"),
            seed,
            InputKind::UploadChmod { remote, idx },
        ));
    }

    /// Upload the pending files, each with its collected mode, on a worker thread.
    pub(crate) fn run_scp_upload(&mut self, remote: String) {
        let Some(p) = self.scp_pending.take() else { return };
        let remote = remote.trim().to_string();
        if remote.is_empty() {
            self.message = Some(tr(self.lang, "cancelled (no remote path)", "中止しました（リモートパスなし）").into());
            return;
        }
        let ScpPending { target, label, locals, .. } = p;
        let modes = std::mem::take(&mut self.scp_upload_modes);
        let verify = self.verify_runtime.or(self.config.options.verify_transfers).unwrap_or(false);
        self.popup = Popup::None;
        self.message = Some(format!("uploading {} …", label));
        let limit = self.transfer_limit;
        self.start_op("uploading", move |ctl| {
            let mut report = OpReport::default();
            let cancel = ctl.cancel;
            // What each marked thing actually involves: a file is one entry,
            // a folder is every file beneath it with the folders listed to be
            // made first. **SFTP has no recursive put** — the tree has to be
            // walked here and the directories created in order, which is what
            // `plan_upload` works out and what the window build has always
            // used. `modes` is indexed by the *marked* item, so the mode a
            // folder was given applies to every file that came out of it.
            let mut jobs: Vec<(PathBuf, String, Option<u32>)> = Vec::new();
            let mut dirs: Vec<String> = Vec::new();
            for (i, local) in locals.iter().enumerate() {
                let mode = modes.get(i).copied().flatten();
                match cian_scp::plan_upload(local, remote.trim_end_matches('/')) {
                    Ok(plan) => {
                        dirs.extend(plan.dirs);
                        jobs.extend(plan.files.into_iter().map(|(from, to)| (from, to, mode)));
                    }
                    Err(e) => report.note_error(format!("{}: {}", local.display(), e)),
                }
            }
            // Parents before children — `plan_upload` returns them that way,
            // and an existing directory is not an error worth stopping for.
            for d in &dirs {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let _ = cian_scp::make_dir(&target, d);
            }
            let total = jobs.len();
            for (i, (local, dest, mode)) in jobs.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let (local, dest, mode) = (local, dest.clone(), *mode);
                let fname = local.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let cur = fname.clone();
                let mut fwd = |done: u64, tot: u64| {
                    (ctl.on_progress)(&cian_core::progress::Progress {
                        bytes_done: done,
                        bytes_total: tot,
                        files_done: i,
                        files_total: total,
                        current: cur.clone(),
                    });
                };
                let mut sctl = cian_scp::Ctl { cancel, on_progress: &mut fwd, limit_bps: limit };
                match cian_scp::upload(&target, local, &dest, mode, &mut sctl) {
                    Ok(via) => {
                        report.ok += 1;
                        report.note = Some(format!("via {}", via.label()));
                        // Verify only when SFTP carried it: the SCP fallback
                        // cannot be re-read for a second checksum.
                        if verify && via == cian_scp::Transport::Sftp {
                            if let Err(e) = verify_transfer(&target, &dest, local, cancel) {
                                report.note_error(format!("{}: {}", fname, e));
                            } else {
                                report.note = Some(format!("via {} ✓ verified", via.label()));
                            }
                        }
                    }
                    Err(e) => report.note_error(format!("{}: {}", fname, e)),
                }
            }
            report
        });
    }

    /// The four local-destination choices, in order, as (label, resolved dir).
    /// The last (`None` dir) means "type a path".
    pub(crate) fn local_dest_options(&self) -> Vec<(String, Option<PathBuf>)> {
        let desktop = dirs_desktop();
        vec![
            ("Left pane".into(), Some(self.left.active_ref().cwd.clone())),
            ("Right pane".into(), Some(self.right.active_ref().cwd.clone())),
            ("Desktop".into(), desktop),
            ("Type a path…".into(), None),
        ]
    }

    /// Act on the chosen local destination: download into a resolved dir, or
    /// prompt for a typed path.
    pub(crate) fn local_dest_pick(&mut self, cursor: usize) {
        let files = if let Popup::LocalDest { files, .. } = &self.popup { files.clone() } else { return };
        let opts = self.local_dest_options();
        let Some((_, dir)) = opts.get(cursor) else { return };
        match dir {
            Some(dir) => {
                // L / R / Desktop: on to the chmod step (local, Unix only).
                let dir = dir.clone();
                self.prompt_download_chmod(files, dir);
            }
            None => {
                self.open_popup(text_input(
                    "download to",
                    "local directory:",
                    self.active_pane().map(|p| p.cwd.display().to_string()).unwrap_or_default(),
                    InputKind::LocalDestPath { files },
                ));
            }
        }
    }

    /// Ask for the mode to apply to downloaded files. Skipped on Windows: NTFS
    /// has no Unix permission bits, so a chmod on the local file can never take
    /// effect — asking for one there is only misleading (a downloaded file shows
    /// up as 644 via a Samba/NFS view no matter what was typed). The upload chmod
    /// still works because it is applied server-side over SFTP.
    pub(crate) fn prompt_download_chmod(&mut self, files: Vec<String>, dir: PathBuf) {
        if cfg!(windows) {
            self.start_remote_download(files, dir, None);
            return;
        }
        self.open_popup(text_input(
            "download — chmod",
            "mode for downloaded files (octal, e.g. 644; blank = keep):",
            String::new(),
            InputKind::DownloadChmod { files, dir },
        ));
    }

    /// Download `files` (remote paths) into `local_dir` on a worker thread, then
    /// apply `mode` to each (Unix; a no-op elsewhere).
    pub(crate) fn start_remote_download(&mut self, files: Vec<String>, local_dir: PathBuf, mode: Option<u32>) {
        let Some((target, label)) = self.scp_target.take() else { return };
        let verify = self.verify_runtime.or(self.config.options.verify_transfers).unwrap_or(false);
        self.popup = Popup::None;
        if let Err(e) = std::fs::create_dir_all(&local_dir) {
            self.message = Some(format!("cannot create {}: {}", local_dir.display(), e));
            return;
        }
        self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("{} 件のファイルを {} から取得中…", files.len(), label)
        } else {
            format!("downloading {} file(s) from {} …", files.len(), label)
        });
        let limit = self.transfer_limit;
        self.start_op("downloading", move |ctl| {
            let mut report = OpReport::default();
            let cancel = ctl.cancel;
            let total = files.len();
            for (i, remote) in files.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let fname = remote.rsplit('/').next().unwrap_or("download").to_string();
                let dest = local_dir.join(&fname);
                let cur = fname.clone();
                let mut fwd = |done: u64, tot: u64| {
                    (ctl.on_progress)(&cian_core::progress::Progress {
                        bytes_done: done,
                        bytes_total: tot,
                        files_done: i,
                        files_total: total,
                        current: cur.clone(),
                    });
                };
                let mut sctl = cian_scp::Ctl { cancel, on_progress: &mut fwd, limit_bps: limit };
                match cian_scp::download(&target, remote, &dest, &mut sctl) {
                    Ok(via) => {
                        // The file is down; a chmod failure is secondary, so still
                        // count it as a success but surface why the mode did not
                        // stick rather than silently dropping it.
                        report.ok += 1;
                        if let Err(e) = chmod_local(&dest, mode) {
                            report.note_error(format!("{}: downloaded, but chmod failed: {}", fname, e));
                        }
                        report.note = Some(format!("via {}", via.label()));
                        // Confirm the local copy matches the file still on the
                        // server (SFTP only — SCP cannot be re-read).
                        if verify && via == cian_scp::Transport::Sftp {
                            if let Err(e) = verify_transfer(&target, remote, &dest, cancel) {
                                report.note_error(format!("{}: {}", fname, e));
                            } else {
                                report.note = Some(format!("via {} ✓ verified", via.label()));
                            }
                        }
                    }
                    Err(e) => report.note_error(format!("{}: {}", fname, e)),
                }
            }
            report
        });
    }

    /// Connect as `user` to host index `idx`, by typing the command into the
    /// shell panel.
    ///
    /// Typing it into a shell rather than spawning `ssh` directly means the
    /// user's own shell config and agent apply, and when the session ends the
    /// tab drops back to a local prompt instead of closing.
    pub(crate) fn ssh_connect(&mut self, idx: usize, user: &str) {
        let Some(h) = self.config.ssh_hosts.get(idx) else { return };
        let Some(u) = h.users.iter().find(|u| u.name == user) else { return };
        let cmd = cian_core::auth::ssh_command(&u.name, &h.host, h.port);
        let label = format!("{}@{}", u.name, h.name);
        // Resolved before the command is sent so a slow `password_cmd` cannot
        // make us miss the prompt.
        let secret = u.secret();
        self.run_in_shell(cmd);
        match secret {
            Some(s) => {
                self.pending_auth =
                    Some(PendingAuth { secret: s, deadline: Instant::now() + AUTH_WINDOW });
                self.message = Some(format!("→ {} (sending password on prompt)", label));
            }
            None => self.message = Some(format!("→ {}", label)),
        }
    }

    /// Send the held password if ssh is now asking for one.
    ///
    /// Returns true if the UI should repaint. The secret is written straight to
    /// the PTY and never logged, echoed, or put in `message`.
    pub(crate) fn poll_pending_auth(&mut self) -> bool {
        let Some(auth) = &self.pending_auth else { return false };
        if Instant::now() > auth.deadline {
            // Expired: keyed host, refused login, or a prompt we do not answer.
            self.pending_auth = None;
            return false;
        }
        // Nothing to look at until the command has actually been delivered.
        if self.pending_shell_input.is_some() {
            return false;
        }
        let asking = match self.shell.active_session() {
            Some(s) => match s.parser().lock() {
                Ok(p) => cian_core::auth::looks_like_password_prompt(&p.screen().contents()),
                Err(_) => false,
            },
            None => false,
        };
        if !asking {
            return false;
        }
        let Some(auth) = self.pending_auth.take() else { return false };
        if let Some(s) = self.shell.active_session_mut() {
            // Submit with a carriage return — a getpass/readpassphrase prompt
            // reads the line ended by Enter (CR), which a bare `\n` may not be.
            let mut bytes = auth.secret.into_bytes();
            bytes.push(b'\r');
            s.write_input(&bytes);
        }
        true
    }
}

/// Join a remote directory and a child name into a remote path (POSIX `/`).
pub(crate) fn join_remote(cwd: &str, name: &str) -> String {
    match cwd {
        "." | "" => name.to_string(),
        "/" => format!("/{}", name),
        _ => format!("{}/{}", cwd.trim_end_matches('/'), name),
    }
}

/// Re-read a just-transferred file from the server over SFTP and compare its
/// checksum with the local copy's. `Ok(())` when they match; `Err(reason)` on a
/// mismatch (the transfer corrupted or truncated the file) or when the check
/// could not be run. Runs on the transfer worker thread, so it honours the same
/// cancel flag.
fn verify_transfer(
    target: &cian_scp::Target,
    remote_path: &str,
    local_path: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> std::result::Result<(), String> {
    use cian_core::attrs::{hash_file, HashKind, Hasher};
    let kind = HashKind::Sha256;
    let local = match hash_file(local_path, kind, cancel) {
        Ok(Some(h)) => h,
        Ok(None) => return Err("verify cancelled".into()),
        Err(e) => return Err(format!("verify: reading the local file failed: {}", e)),
    };
    let mut hasher = Hasher::new(kind);
    if let Err(e) = cian_scp::remote_read(target, remote_path, cancel, &mut |b| hasher.update(b)) {
        return Err(format!("verify unavailable: {}", e));
    }
    let remote = hasher.finish();
    if remote == local {
        Ok(())
    } else {
        let short = |s: &str| s.chars().take(12).collect::<String>();
        Err(format!("CHECKSUM MISMATCH — local {}… ≠ remote {}…", short(&local), short(&remote)))
    }
}

/// The parent of a remote path. Home-relative "." stays "."; an absolute path
/// climbs toward "/".
fn parent_remote(cwd: &str) -> String {
    match cwd {
        "." | "" => ".".to_string(),
        "/" => "/".to_string(),
        _ => {
            let trimmed = cwd.trim_end_matches('/');
            match trimmed.rsplit_once('/') {
                Some(("", _)) => "/".to_string(),      // "/foo" -> "/"
                Some((parent, _)) => parent.to_string(),
                None => ".".to_string(),                 // "foo" (relative) -> "."
            }
        }
    }
}

/// Apply Unix permission bits to a just-downloaded local file. A no-op on
/// Windows (NTFS has no Unix mode) and when `mode` is `None`.
#[cfg(unix)]
fn chmod_local(path: &std::path::Path, mode: Option<u32>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(m) = mode {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(m))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn chmod_local(_path: &std::path::Path, _mode: Option<u32>) -> std::io::Result<()> {
    Ok(())
}

/// The user's Desktop, if it exists.
fn dirs_desktop() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let d = PathBuf::from(home).join("Desktop");
    d.is_dir().then_some(d)
}

#[cfg(test)]
mod tests {
    use super::{join_remote, parent_remote};

    #[test]
    fn remote_path_join_and_parent() {
        assert_eq!(join_remote(".", "docs"), "docs");
        assert_eq!(join_remote("/var", "log"), "/var/log");
        assert_eq!(join_remote("/", "etc"), "/etc");
        assert_eq!(join_remote("a/b", "c"), "a/b/c");

        assert_eq!(parent_remote("."), ".");
        assert_eq!(parent_remote("/"), "/");
        assert_eq!(parent_remote("/var/log"), "/var");
        assert_eq!(parent_remote("/var"), "/");     // climbs to root
        assert_eq!(parent_remote("a/b"), "a");
        assert_eq!(parent_remote("docs"), ".");     // relative single -> home

        // The reported case: connected as userA (home /home/userA), climbing up
        // must reach /home and then / rather than stopping at home.
        assert_eq!(parent_remote("/home/userA"), "/home");
        assert_eq!(parent_remote("/home"), "/");
        assert_eq!(parent_remote("/home/userA/"), "/home"); // trailing slash tolerated
    }
}
