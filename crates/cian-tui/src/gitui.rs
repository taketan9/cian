//! Git actions on the App: refreshing/caching per-pane status, stage, unstage,
//! and discard (with its confirm), plus the accessors the rest of the UI reads.
//! The git plumbing itself lives in cian-core::git.
use super::*;

impl App {
    /// The directory a newly-spawned shell should start in: the cwd of the
    /// file pane we were last on.
    /// Recompute each pane's git status when its directory has changed since the
    /// last cache. Called once per frame; the `git` shell-out only runs on an
    /// actual directory change, so it costs nothing while browsing one folder.
    pub(crate) fn ensure_git(&mut self) {
        for (idx, tabs) in [&self.left, &self.right].into_iter().enumerate() {
            let cwd = tabs.active_ref().cwd.clone();
            let stale = self.git[idx].as_ref().map(|g| g.cwd != cwd).unwrap_or(true);
            if stale {
                // Git first, then svn; a directory is at most one of them.
                let (kind, status) = if let Some(s) = cian_core::git::status(&cwd) {
                    (Some(Vcs::Git), Some(s))
                } else if let Some(s) = cian_core::svn::status(&cwd) {
                    (Some(Vcs::Svn), Some(s))
                } else {
                    (None, None)
                };
                self.git[idx] = Some(GitState { cwd: cwd.clone(), kind, status });
            }
            // Free-space cache: same cwd-keyed refresh as git. Cheap local
            // syscall, only when the directory actually changed.
            let disk_stale = self.disk[idx].as_ref().map(|(d, _)| *d != cwd).unwrap_or(true);
            if disk_stale {
                self.disk[idx] = Some((cwd.clone(), cian_core::disk::usage(&cwd)));
            }
        }
    }

    /// The cached disk usage for a file pane, if its mount could be queried.
    pub(crate) fn disk_for(&self, pane: FocusedPane) -> Option<cian_core::disk::Usage> {
        let idx = match pane {
            FocusedPane::Left => 0,
            FocusedPane::Right => 1,
            FocusedPane::Shell => return None,
        };
        self.disk[idx].as_ref().and_then(|(_, u)| *u)
    }

    /// The VCS the active file pane's directory belongs to, if any.
    pub(crate) fn vcs_kind(&self) -> Option<Vcs> {
        let idx = match self.focused {
            FocusedPane::Left => 0,
            FocusedPane::Right => 1,
            FocusedPane::Shell => return None,
        };
        self.git[idx].as_ref().and_then(|g| g.kind)
    }

    /// The active file pane's directory plus its VCS, if it is under one.
    /// Falls back to a direct probe (the cache is cold right after an action).
    pub(crate) fn vcs_dir(&self) -> Option<(PathBuf, Vcs)> {
        if !matches!(self.focused, FocusedPane::Left | FocusedPane::Right) {
            return None;
        }
        let cwd = self.active_pane()?.cwd.clone();
        if let Some(kind) = self.vcs_kind() {
            return Some((cwd, kind));
        }
        if cian_core::git::status(&cwd).is_some() {
            Some((cwd, Vcs::Git))
        } else if cian_core::svn::is_working_copy(&cwd) {
            Some((cwd, Vcs::Svn))
        } else {
            None
        }
    }

    /// Drop the git cache so the next frame recomputes it — after a git action
    /// or a file operation that may have changed the working tree.
    pub(crate) fn invalidate_git(&mut self) {
        self.git = [None, None];
        // A copy/move/delete/extract changes free space too — re-probe it.
        self.disk = [None, None];
    }

    /// The selection to act on for a git command: marked files, else the entry
    /// under the cursor (never the `..` row).
    pub(crate) fn git_targets(&self) -> Vec<PathBuf> {
        self.target_paths()
    }

    /// Stage the selection: `git add` in git, `svn add` in an svn working copy.
    pub(crate) fn git_stage(&mut self) {
        let Some((dir, kind)) = self.vcs_dir() else {
            self.message = Some(tr(self.lang, "not a version-controlled directory", "バージョン管理下のディレクトリではありません").into());
            return;
        };
        let paths = self.git_targets();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        }
        let (res, verb) = match kind {
            Vcs::Git => (cian_core::git::stage(&dir, &paths), "git add"),
            Vcs::Svn => (cian_core::svn::add(&dir, &paths), "svn add"),
        };
        match res {
            Ok(()) => {
                self.message = Some(format!("● added {} path(s)", paths.len()));
                self.invalidate_git();
                self.reload_active();
            }
            Err(e) => self.message = Some(format!("{}: {}", verb, e)),
        }
    }

    /// `git reset HEAD` the selection (unstage). Git-only — svn has no index.
    pub(crate) fn git_unstage(&mut self) {
        let Some((dir, kind)) = self.vcs_dir() else {
            self.message = Some(tr(self.lang, "not a version-controlled directory", "バージョン管理下のディレクトリではありません").into());
            return;
        };
        if kind != Vcs::Git {
            self.message = Some(tr(self.lang, "svn has no staging area to unstage", "svn にはステージング領域がありません").into());
            return;
        }
        let paths = self.git_targets();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        }
        match cian_core::git::unstage(&dir, &paths) {
            Ok(()) => {
                self.message = Some(format!("unstaged {} path(s)", paths.len()));
                self.invalidate_git();
                self.reload_active();
            }
            Err(e) => self.message = Some(format!("git reset: {}", e)),
        }
    }

    /// Open the confirm dialog for discarding worktree changes to the selection.
    pub(crate) fn git_discard_prompt(&mut self) {
        let Some((dir, _kind)) = self.vcs_dir() else {
            self.message = Some(tr(self.lang, "not a version-controlled directory", "バージョン管理下のディレクトリではありません").into());
            return;
        };
        let paths = self.git_targets();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        }
        self.open_popup(Popup::ConfirmDiscard { targets: paths, dir });
    }

    /// Discard worktree changes to the selection: `git checkout --` in git,
    /// `svn revert` in an svn working copy. Called after the confirm.
    pub(crate) fn git_discard(&mut self) {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let Popup::ConfirmDiscard { targets, dir } = popup else { return };
        let kind = self.vcs_kind().unwrap_or(Vcs::Git);
        let (res, verb) = match kind {
            Vcs::Git => (cian_core::git::discard(&dir, &targets), "git checkout"),
            Vcs::Svn => (cian_core::svn::revert(&dir, &targets), "svn revert"),
        };
        match res {
            Ok(()) => {
                self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("{} 件の変更を取り消しました", targets.len())
        } else {
            format!("reverted changes to {} path(s)", targets.len())
        });
                self.invalidate_git();
                self.reload_active();
            }
            Err(e) => self.message = Some(format!("{}: {}", verb, e)),
        }
    }

    /// `svn resolve --accept working` the selection (svn only).
    /// `:commit` ── ステージ済みの変更を、自分で書いたメッセージでコミットする。
    ///
    /// これが無いあいだ、git へのコミットは `:aicommit` からしか行けなかった。
    /// AI に下書きを作らせてから確定する道しか無いので、**社内の endpoint に
    /// 届かない機械では git にコミットできない**という状態になっていた。svn の
    /// 側には最初から素のコミットがある。
    ///
    /// 開く枠は `:aicommit` と同じもので、違うのは中身が空で編集中から始まる
    /// ことだけ。入力 → Esc で編集終了 → Enter でコミット。
    pub(crate) fn start_commit(&mut self) {
        let Some(dir) = self.cwd() else { return };
        let Some(diff) = cian_core::git::staged_diff(&dir) else {
            self.message = Some(tr(self.lang, "not a git repository", "git リポジトリではありません").into());
            return;
        };
        if diff.trim().is_empty() {
            self.message = Some(tr(self.lang, "nothing staged. `git add` first (or stage from the pane)", "ステージされていません。先に `git add`（ペインからでも可）").into());
            return;
        }
        let stat = cian_core::git::staged_stat(&dir).unwrap_or_default();
        self.open_popup(Popup::CommitMessage {
            buffer: String::new(),
            stat,
            dir,
            editing: true,
            drafted: false,
        });
    }

    pub(crate) fn svn_resolve(&mut self) {
        let Some((dir, kind)) = self.vcs_dir() else {
            self.message = Some(tr(self.lang, "not a version-controlled directory", "バージョン管理下のディレクトリではありません").into());
            return;
        };
        if kind != Vcs::Svn {
            self.message = Some(tr(self.lang, "resolve is svn-only", "resolve は svn 専用です").into());
            return;
        }
        let paths = self.git_targets();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        }
        match cian_core::svn::resolve(&dir, &paths) {
            Ok(()) => {
                self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("{} 件を解決しました", paths.len())
        } else {
            format!("resolved {} path(s)", paths.len())
        });
                self.invalidate_git();
                self.reload_active();
            }
            Err(e) => self.message = Some(format!("svn resolve: {}", e)),
        }
    }

    /// `svn update` the working copy (svn only; touches the network).
    pub(crate) fn svn_update(&mut self) {
        let Some((dir, kind)) = self.vcs_dir() else {
            self.message = Some(tr(self.lang, "not a version-controlled directory", "バージョン管理下のディレクトリではありません").into());
            return;
        };
        if kind != Vcs::Svn {
            self.message = Some(tr(self.lang, "update is svn-only", "update は svn 専用です").into());
            return;
        }
        match cian_core::svn::update(&dir) {
            Ok(()) => {
                self.message = Some(tr(self.lang, "● svn update complete", "● svn update が完了しました").into());
                self.invalidate_git();
                self.reload_active();
            }
            Err(e) => self.message = Some(format!("svn update: {}", e)),
        }
    }

    /// Open a text prompt for the commit message, then `svn commit` (svn only).
    pub(crate) fn svn_commit_prompt(&mut self) {
        let Some((_dir, kind)) = self.vcs_dir() else {
            self.message = Some(tr(self.lang, "not a version-controlled directory", "バージョン管理下のディレクトリではありません").into());
            return;
        };
        if kind != Vcs::Svn {
            self.message = Some(tr(self.lang, "commit is svn-only here", "ここでのコミットは svn 専用です").into());
            return;
        }
        let paths = self.git_targets();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected to commit", "コミットする対象が選択されていません").into());
            return;
        }
        self.open_popup(text_input(
            "svn commit",
            "message:",
            String::new(),
            InputKind::SvnCommit { paths },
        ));
    }

    /// `svn commit -m <message>` the given paths (called when the input popup
    /// is confirmed).
    pub(crate) fn svn_commit(&mut self, paths: &[PathBuf], message: &str) {
        let Some((dir, _kind)) = self.vcs_dir() else { return };
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected to commit", "コミットする対象が選択されていません").into());
            return;
        }
        match cian_core::svn::commit(&dir, paths, message) {
            Ok(()) => {
                self.message = Some(format!("● committed {} path(s)", paths.len()));
                self.invalidate_git();
                self.reload_active();
            }
            Err(e) => self.message = Some(format!("svn commit: {}", e)),
        }
    }

    /// The git status for a file pane, if it sits in a repo.
    pub(crate) fn git_for(&self, pane: FocusedPane) -> Option<&cian_core::git::RepoStatus> {
        let idx = match pane {
            FocusedPane::Left => 0,
            FocusedPane::Right => 1,
            FocusedPane::Shell => return None,
        };
        self.git[idx].as_ref().and_then(|g| g.status.as_ref())
    }

    /// Lift a pane's git status out of `app` for the length of a frame.
    ///
    /// The drawing code needs the status *and* a mutable borrow of the pane
    /// next to it, which the borrow checker will not allow — so it used to take
    /// a clone. A `RepoStatus` holds a map of every changed path and a set of
    /// every directory above them, and that was being copied twice per frame,
    /// for a screen that shows forty rows. In a tree with a lot of untracked
    /// files under it the copy is larger than everything else the frame does.
    ///
    /// Taken and put back instead: two moves, no allocation. Always paired with
    /// [`App::put_git`] — a frame that returns in between leaves the pane
    /// looking like it is not in a repository until the next reload.
    pub(crate) fn take_git(&mut self, pane: FocusedPane) -> Option<cian_core::git::RepoStatus> {
        let idx = match pane {
            FocusedPane::Left => 0,
            FocusedPane::Right => 1,
            FocusedPane::Shell => return None,
        };
        self.git[idx].as_mut().and_then(|g| g.status.take())
    }

    /// Put back what [`App::take_git`] lifted out.
    pub(crate) fn put_git(&mut self, pane: FocusedPane, status: Option<cian_core::git::RepoStatus>) {
        let Some(status) = status else { return };
        let idx = match pane {
            FocusedPane::Left => 0,
            FocusedPane::Right => 1,
            FocusedPane::Shell => return,
        };
        if let Some(g) = self.git[idx].as_mut() {
            g.status = Some(status);
        }
    }

    /// Show read-only `text` in the viewer (used for git diffs / commit views).
    /// A synthetic view: no file on disk, not editable.
    pub(crate) fn open_text_viewer(&mut self, title: &str, text: String) {
        let total = text.len() as u64;
        let view = cian_core::viewer::View::from_text(text, total, false);
        let source = view.lines.clone();
        self.popup = Popup::Viewer {
            title: title.to_string(),
            path: PathBuf::new(),
            // A rendering of what git holds, not a file: there is nothing on
            // disk for a save to write over.
            stamp: None,
            view: Box::new(view),
            scroll: 0,
            line: 0,
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

    /// `:gitdiff` / right-click: show the selected file's working-tree changes
    /// versus HEAD, in the viewer.
    pub(crate) fn git_diff_file(&mut self) {
        let Some((dir, kind)) = self.vcs_dir() else {
            self.message = Some(tr(self.lang, "not a version-controlled directory", "バージョン管理下のディレクトリではありません").into());
            return;
        };
        let Some(entry) = self.active_pane().and_then(|p| p.selected()).filter(|e| !e.is_parent) else {
            self.message = Some(tr(self.lang, "select a file to diff", "差分を取るファイルを選んでください").into());
            return;
        };
        let (path, name) = (entry.path.clone(), entry.name.clone());
        let (diff, base) = match kind {
            Vcs::Git => (cian_core::git::file_diff(&dir, &path), "HEAD"),
            Vcs::Svn => (cian_core::svn::file_diff(&dir, &path), "BASE"),
        };
        match diff {
            Some(d) if !d.trim().is_empty() => self.open_text_viewer(&format!("diff {} — {}", base, name), d),
            Some(_) => self.message = Some(format!("{}: no changes vs {}", name, base)),
            None => self.message = Some(tr(self.lang, "diff failed", "差分の取得に失敗しました").into()),
        }
    }

    /// `@`/`:gitlog` / right-click: the commit log — one file's history when the
    /// cursor is on a tracked file, otherwise the whole repository/working copy.
    pub(crate) fn start_git_log(&mut self) {
        let Some((dir, kind)) = self.vcs_dir() else {
            self.message = Some(tr(self.lang, "not a version-controlled directory", "バージョン管理下のディレクトリではありません").into());
            return;
        };
        let file = self
            .active_pane()
            .and_then(|p| p.selected())
            .filter(|e| !e.is_parent && !e.is_dir)
            .map(|e| (e.path.clone(), e.name.clone()));
        let path_arg = file.as_ref().map(|(p, _)| p.as_path());
        let commits = match kind {
            Vcs::Git => cian_core::git::log(&dir, path_arg, 300),
            Vcs::Svn => cian_core::svn::log(&dir, path_arg, 300),
        };
        if commits.is_empty() {
            self.message = Some(tr(self.lang, "no commits", "コミットがありません").into());
            return;
        }
        let vcs_name = match kind {
            Vcs::Git => "git",
            Vcs::Svn => "svn",
        };
        let title = match &file {
            Some((_, name)) => format!("{} log — {}", vcs_name, name),
            None => format!("{} log", vcs_name),
        };
        self.open_popup(Popup::GitLog { title, dir, commits, cursor: 0, scroll: 0, vcs: kind });
    }

    /// Show a commit's diff (`git show` / `svn diff -c`) in the viewer.
    pub(crate) fn git_show_commit(&mut self, hash: &str, dir: &Path, vcs: Vcs) {
        let diff = match vcs {
            Vcs::Git => cian_core::git::show(dir, hash),
            Vcs::Svn => cian_core::svn::show(dir, hash),
        };
        match diff {
            Some(s) => self.open_text_viewer(&format!("commit {}", hash), s),
            None => self.message = Some(tr(self.lang, "show failed", "表示に失敗しました").into()),
        }
    }

    /// `B` in the viewer: toggle the blame gutter (git or svn) for the file.
    pub(crate) fn toggle_viewer_blame(&mut self) {
        let path = if let Popup::Viewer { path, blame, .. } = &self.popup {
            if !blame.is_empty() {
                // Already on → turn it off.
                if let Popup::Viewer { blame, .. } = &mut self.popup {
                    blame.clear();
                }
                self.message = Some(tr(self.lang, "blame off", "blame オフ").into());
                return;
            }
            path.clone()
        } else {
            return;
        };
        let Some(dir) = path.parent() else { return };
        // Git first, else svn (matching how the change gutter is sourced).
        let blame = cian_core::git::blame(dir, &path)
            .filter(|b| !b.is_empty())
            .or_else(|| cian_core::svn::blame(dir, &path).filter(|b| !b.is_empty()));
        match blame {
            Some(b) => {
                if let Popup::Viewer { blame, .. } = &mut self.popup {
                    *blame = b;
                }
                self.message = Some(tr(self.lang, "blame on", "blame オン").into());
            }
            None => self.message = Some(tr(self.lang, "no blame (untracked or not a repo)", "blame不可（未追跡/非リポジトリ）").into()),
        }
    }
}
