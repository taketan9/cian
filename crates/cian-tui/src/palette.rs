//! The fuzzy pickers: a **command palette** (`C` / `:palette`) over the
//! commands, and a **fuzzy jump** (`Z` / `:jump`) over recently-visited
//! directories and bookmarks. Both share [`cian_core::fuzzy`] for ranking.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::*;

/// How many paths the walk carries back at a time. Small enough that the first
/// of them lands within a frame or two of the picker opening, large enough
/// that a fast local tree is not a channel benchmark.
const FILE_SCAN_BATCH: usize = 512;

/// Where the walk gives up. It is not there to keep the picker responsive any
/// more — the walk is off the main loop now — but ranking every row against
/// every keystroke does cost something, and a tree this size is one to narrow
/// rather than to scroll.
const FILE_SCAN_CAP: usize = 50_000;

/// The file finder's tree walk, in flight while its picker is already open.
pub(crate) struct FileScan {
    rx: std::sync::mpsc::Receiver<FileBatch>,
    /// Set when the picker closes, so the walk stops rather than reading a
    /// network drive to the end for a list nobody will see.
    cancel: Arc<AtomicBool>,
    root: PathBuf,
    /// Paths already in the picker — the recent files went in before the walk
    /// started, and the walk will find them again.
    seen: HashSet<PathBuf>,
}

enum FileBatch {
    More(Vec<PathBuf>),
    Done { capped: bool, count: usize },
}

/// The commands the palette offers, as `(verb, description, takes_arg)`. Kept a
/// curated list (not every alias) so the picker stays scannable.
#[rustfmt::skip]
pub(crate) fn command_list() -> &'static [(&'static str, (&'static str, &'static str), bool)] {
    &[
        ("ime",        ("input-method switching: state and helper", "日本語入力の自動切替: 状態と設定"), false),
        ("ai",         ("AI - simple: chat with the local model", "AI - simple: ローカルモデルとチャット"), false),
        ("aicmd",      ("AI: shell command from a description", "AI: 説明からコマンド"), true),
        ("aicommit",   ("AI: draft a commit message", "AI: コミットメッセージ下書き"), false),
        ("aierror",    ("AI: explain the last shell error", "AI: 直前のエラーを説明"), false),
        ("aidiff",     ("AI: explain the diff on screen", "AI: 差分を説明"), false),
        ("ailog",      ("AI: triage the selected log", "AI: ログを診断"), false),
        ("du",         ("disk usage: what's biggest here", "容量分析"), false),
        ("preview",    ("cursor preview in the shell panel", "カーソル追従プレビュー"), false),
        ("queue",      ("operation queue: running + waiting", "操作キュー（実行中と待機）"), false),
        ("nobom",      ("strip UTF-8 BOMs from the selection", "選択から UTF-8 BOM を除去"), false),
        ("count",      ("count files, lines and steps", "ファイル/行/ステップ数"), false),
        ("diff",       ("compare the two panes", "左右を比較"), false),
        ("duplicate", ("find duplicate files", "重複ファイル検出"), false),
        ("renamepattern",("bulk rename by a pattern", "パターン一括リネーム"), false),
        ("each",       ("run a command per marked file ({} = path)", "マーク各ファイルにコマンド実行（{} = パス）"), true),
        ("sftp",       ("open a server in this pane (remote pane)", "サーバをペインで開く"), false),
        ("ssh",        ("ssh connect picker", "SSH 接続ピッカー"), false),
        ("zip",        ("zip the selection (-e for a password)", "選択を zip（-e で暗号化）"), false),
        ("tar",        ("tar the selection", "選択を tar"), false),
        ("targz",      ("tar.gz the selection", "選択を tar.gz"), false),
        ("unzip",      ("extract the archive here", "アーカイブをここに展開"), false),
        ("hash",       ("checksum the selection (md5 / sha256)", "チェックサム"), true),
        ("attr",       ("permissions & owner", "権限・所有者"), false),
        ("chmod",      ("change the mode (octal)", "モード変更（8進）"), true),
        ("hidden",     ("show / hide dotfiles", "隠しファイル表示切替"), false),
        ("theme",      ("theme gallery / set a theme", "テーマ選択"), true),
        ("sync",       ("synchronize input across shell panes", "シェル入力同期"), false),
        ("snip",       ("snippet launcher", "スニペット"), false),
        ("macro",     ("run a macro", "マクロ実行"), false),
        ("stage",      ("git add the selection", "git add"), false),
        ("unstage",    ("git reset the selection", "git reset"), false),
        ("discard",    ("discard worktree changes", "変更を破棄"), false),
        ("log",       ("commit log", "コミットログ"), false),
        ("gitdiff",    ("working-tree diff vs HEAD", "作業ツリーの差分"), false),
        ("back",       ("this pane's directory history", "このペインの移動履歴"), false),
        ("jump",       ("fuzzy-jump to a recent directory", "最近のディレクトリへ移動"), false),
        ("files",      ("live fuzzy file finder (this tree)", "ライブ・ファイル検索（このツリー）"), false),
        ("recent",     ("recently-opened files", "最近開いたファイル"), false),
        ("toggle",    ("UI toggles menu (T)", "UIトグルメニュー（T）"), false),
        ("cd",         ("go to a typed path", "パス入力で移動"), true),
        ("edit",       ("open in the external editor", "外部エディタで開く"), false),
        ("reload",     ("reload init.lua", "init.lua を再読込"), false),
        ("menu",       ("the right-click menu", "右クリックメニュー"), false),
        ("where",      ("show config / state file paths", "設定ファイルの場所"), false),
    ]
}

impl App {
    /// Open the command palette (`C` / `:palette`).
    pub(crate) fn start_command_palette(&mut self) {
        let ja = self.lang == Lang::Ja;
        let items: Vec<PaletteItem> = command_list()
            .iter()
            .map(|(verb, (en, jp), takes_arg)| PaletteItem {
                label: format!(":{}", verb),
                detail: (if ja { *jp } else { *en }).to_string(),
                value: verb.to_string(),
                takes_arg: *takes_arg,
            })
            .collect();
        self.open_palette(PaletteKind::Commands, items);
    }

    /// Open the fuzzy jump picker (`Z` / `:jump`): recent directories from
    /// both panes' history, plus bookmarked folders, deduped.
    pub(crate) fn start_fuzzy_jump(&mut self) {
        let mut seen = std::collections::HashSet::new();
        let mut items = Vec::new();
        let push = |items: &mut Vec<PaletteItem>, seen: &mut std::collections::HashSet<String>, p: &std::path::Path, tag: &str| {
            let s = p.display().to_string();
            if !p.is_dir() || !seen.insert(s.clone()) {
                return;
            }
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| s.clone());
            items.push(PaletteItem { label: name, detail: format!("{}  {}", tag, s), value: s, takes_arg: false });
        };
        // Recent directories from both panes (most recent first).
        for tabs in [&self.left, &self.right] {
            let pane = tabs.active_ref();
            push(&mut items, &mut seen, &pane.cwd, "•");
            for p in &pane.history {
                push(&mut items, &mut seen, p, "recent");
            }
        }
        // Bookmarked folders (shortcuts whose target is a directory).
        for sc in bookmark_dirs(&self.shortcuts.entries) {
            push(&mut items, &mut seen, &sc, "bookmark");
        }
        if items.is_empty() {
            self.message = Some(tr(self.lang, "no recent directories to jump to", "移動できる履歴がありません").into());
            return;
        }
        self.open_palette(PaletteKind::Jump, items);
    }

    /// Record a file just opened in the viewer for the recent-files list. Skips
    /// the temp copies of remote files (they aren't real local files to reopen).
    pub(crate) fn note_recent_file(&mut self, path: &std::path::Path) {
        if path.components().any(|c| c.as_os_str() == "cian-remote") {
            return;
        }
        let p = path.to_path_buf();
        self.recent_files.retain(|x| x != &p);
        self.recent_files.insert(0, p);
        self.recent_files.truncate(50);
    }

    /// `:files` — a live fuzzy finder over the files under the active pane. The
    /// empty-query list leads with the recently-opened files, then the tree.
    pub(crate) fn start_file_finder(&mut self) {
        let Some(root) = self.cwd() else { return };
        // The recently-opened files are a short list already in memory, so the
        // picker opens on them at once.
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let items: Vec<PaletteItem> = self
            .recent_files
            .iter()
            .filter(|p| p.is_file() && seen.insert((*p).clone()))
            .map(|p| file_item(p, &root, "recent"))
            .collect();
        self.open_palette(PaletteKind::File, items);
        self.start_file_scan(root, seen, FILE_SCAN_CAP);
    }

    /// Walk the tree on a worker, sending files back in batches.
    ///
    /// This used to run to completion on the main loop before the picker drew
    /// a single row: on a deep tree or a network drive, opening the finder
    /// meant waiting for it with no sign that anything was happening. Every
    /// other slow thing in cian is a worker polled each frame, and this is
    /// that — the picker is usable from the first keystroke and the rest of
    /// the tree arrives underneath.
    /// `cap` is where the walk gives up; the tests hand it a small one rather
    /// than making fifty thousand files.
    fn start_file_scan(&mut self, root: PathBuf, seen: HashSet<PathBuf>, cap: usize) {
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_root = root.clone();
        std::thread::spawn(move || {
            let mut batch: Vec<PathBuf> = Vec::with_capacity(FILE_SCAN_BATCH);
            let mut count = 0usize;
            let complete = gather_files(&worker_root, &mut |p| {
                if worker_cancel.load(Ordering::Relaxed) {
                    return false;
                }
                batch.push(p.to_path_buf());
                count += 1;
                if batch.len() >= FILE_SCAN_BATCH && tx.send(FileBatch::More(std::mem::take(&mut batch))).is_err()
                {
                    // The picker closed; nobody is listening any more.
                    return false;
                }
                count < cap
            });
            if !batch.is_empty() {
                let _ = tx.send(FileBatch::More(batch));
            }
            // `gather_files` returns false when the callback stopped it. A
            // cancelled walk says nothing; one the cap stopped has to say so.
            let capped = !complete && !worker_cancel.load(Ordering::Relaxed);
            let _ = tx.send(FileBatch::Done { capped, count });
        });
        self.file_scan = Some(FileScan { rx, cancel, root, seen });
    }

    /// Take whatever the walk has found since the last frame into the open
    /// picker. Returns true if anything changed on screen.
    pub(crate) fn poll_file_scan(&mut self) -> bool {
        // Typing into it, or leaving it, both end the walk's usefulness.
        if !matches!(self.popup, Popup::Palette { kind: PaletteKind::File, .. }) {
            self.stop_file_scan();
            return false;
        }
        let mut added = 0usize;
        let mut finished: Option<(bool, usize)> = None;
        {
            let Some(scan) = &mut self.file_scan else { return false };
            let Popup::Palette { items, .. } = &mut self.popup else { return false };
            loop {
                match scan.rx.try_recv() {
                    Ok(FileBatch::More(paths)) => {
                        for p in paths {
                            if scan.seen.insert(p.clone()) {
                                items.push(file_item(&p, &scan.root, ""));
                                added += 1;
                            }
                        }
                    }
                    Ok(FileBatch::Done { capped, count }) => {
                        finished = Some((capped, count));
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        finished = Some((false, 0));
                        break;
                    }
                }
            }
        }
        if let Some((capped, count)) = finished {
            self.file_scan = None;
            // A walk that stopped at the cap used to say nothing at all, so a
            // file that existed simply was not in the list and the finder
            // looked wrong rather than full.
            if capped {
                self.message = Some(match self.lang {
                    Lang::En => format!("first {count} files — narrow the folder to see the rest"),
                    Lang::Ja => format!("{count} 件で打ち切りました。ディレクトリを絞ると残りも見られます"),
                });
            } else if matches!(&self.popup, Popup::Palette { items, .. } if items.is_empty()) {
                // Nothing here at all. Better to say so than to leave an empty
                // picker open for the user to type into.
                self.popup = Popup::None;
                self.message = Some(tr(self.lang, "no files here", "ファイルがありません").into());
            }
        }
        if added > 0 {
            // Rank what arrived against whatever has been typed so far.
            self.palette_refilter_keeping_cursor();
            return true;
        }
        finished.is_some()
    }

    /// The two hooks the tests drive the walk through: an empty picker, and a
    /// walk with a cap small enough to reach without making the files.
    #[cfg(test)]
    pub(crate) fn open_palette_for_test(&mut self) {
        self.open_palette(PaletteKind::File, Vec::new());
    }

    #[cfg(test)]
    pub(crate) fn start_file_scan_for_test(&mut self, root: PathBuf, cap: usize) {
        self.start_file_scan(root, HashSet::new(), cap);
    }

    /// Drop the walk and ask the worker to stop.
    pub(crate) fn stop_file_scan(&mut self) {
        if let Some(scan) = self.file_scan.take() {
            scan.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// `:recent` — just the recently-opened files.
    pub(crate) fn start_recent_files(&mut self) {
        let root = self.cwd().unwrap_or_default();
        let items: Vec<PaletteItem> = self
            .recent_files
            .iter()
            .filter(|p| p.is_file())
            .map(|p| file_item(p, &root, "recent"))
            .collect();
        if items.is_empty() {
            self.message = Some(tr(self.lang, "no recent files yet", "最近開いたファイルはありません").into());
            return;
        }
        self.open_palette(PaletteKind::File, items);
    }

    fn open_palette(&mut self, kind: PaletteKind, items: Vec<PaletteItem>) {
        // Any walk still out belongs to the picker being replaced. `:recent`
        // opens the same kind of picker as the finder, so without this a walk
        // left over from an `//` closed in the same breath would pour the
        // whole tree into the list of files you have actually opened.
        self.stop_file_scan();
        let shown = (0..items.len()).collect();
        self.open_popup(Popup::Palette { kind, query: String::new(), items, shown, cursor: 0, scroll: 0 });
    }

    /// Re-rank without losing the row under the cursor.
    ///
    /// The file finder ranks again every time the walk hands back a batch, and
    /// the plain refilter puts the cursor back at the top — which, while rows
    /// are still arriving, means the highlight will not stay where it is put.
    pub(crate) fn palette_refilter_keeping_cursor(&mut self) {
        let Popup::Palette { shown, cursor, .. } = &self.popup else { return };
        let on = shown.get(*cursor).copied();
        self.palette_refilter();
        let Popup::Palette { shown, cursor, .. } = &mut self.popup else { return };
        if let Some(at) = on.and_then(|i| shown.iter().position(|&j| j == i)) {
            *cursor = at;
        }
    }

    /// Re-rank the picker's rows against the current query (called on every edit).
    pub(crate) fn palette_refilter(&mut self) {
        if let Popup::Palette { query, items, shown, cursor, scroll, .. } = &mut self.popup {
            let labels = items.iter().map(|it| it.label.as_str());
            *shown = cian_core::fuzzy::rank(query, labels);
            *cursor = 0;
            *scroll = 0;
        }
    }

    /// Move the picker's cursor by `delta`, clamped.
    pub(crate) fn palette_move(&mut self, delta: isize) {
        if let Popup::Palette { shown, cursor, .. } = &mut self.popup {
            if shown.is_empty() {
                return;
            }
            let n = shown.len() as isize;
            *cursor = (*cursor as isize + delta).clamp(0, n - 1) as usize;
        }
    }

    /// Act on the highlighted row: run the command / prefill it, or jump.
    pub(crate) fn palette_accept(&mut self) {
        let (kind, value, takes_arg) = {
            let Popup::Palette { kind, items, shown, cursor, .. } = &self.popup else { return };
            let Some(&idx) = shown.get(*cursor) else { return };
            let it = &items[idx];
            (*kind, it.value.clone(), it.takes_arg)
        };
        self.popup = Popup::None;
        match kind {
            PaletteKind::Jump => {
                if let Some(p) = self.active_pane_mut() {
                    let _ = p.jump_to(PathBuf::from(value));
                }
            }
            PaletteKind::File => {
                // Reveal the file: jump the pane to its folder, cursor on it.
                self.reveal_path_in_pane(&PathBuf::from(value));
            }
            PaletteKind::Commands if takes_arg => {
                // Prefill command mode so the argument can be typed.
                self.command_buffer = format!("{} ", value);
                self.mode = Mode::Command;
            }
            PaletteKind::Commands => {
                self.command_buffer = value;
                self.run_command();
            }
        }
    }
}

/// The directory targets among the flat list of bookmarks (nested groups walked).
fn bookmark_dirs(entries: &[Shortcut]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(list: &[Shortcut], out: &mut Vec<PathBuf>) {
        for s in list {
            if let Some(children) = &s.children {
                walk(children, out);
            } else if let Some(t) = &s.target {
                let expanded = crate::expand_path(t);
                if expanded.is_dir() {
                    out.push(expanded);
                }
            }
        }
    }
    walk(entries, &mut out);
    out
}

/// Build a picker row for a file, labelled by its path relative to `root` (so
/// the fuzzy match sees the folders too) with the full path as the value.
fn file_item(path: &std::path::Path, root: &std::path::Path, tag: &str) -> PaletteItem {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let full = path.display().to_string();
    let detail = if tag.is_empty() { full.clone() } else { format!("{tag}  {full}") };
    PaletteItem { label: rel.display().to_string(), detail, value: full, takes_arg: false }
}

/// Walk `dir` recursively, calling `cb` for each file. `cb` returns false to
/// stop the whole walk (used to cap the count). Skips VCS metadata directories.
fn gather_files(dir: &std::path::Path, cb: &mut dyn FnMut(&std::path::Path) -> bool) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return true };
    for entry in rd.flatten() {
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if matches!(entry.file_name().to_str(), Some(".git" | ".svn" | ".hg")) {
                continue;
            }
            if !gather_files(&entry.path(), cb) {
                return false;
            }
        } else if !cb(&entry.path()) {
            return false;
        }
    }
    true
}
