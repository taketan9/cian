use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Context, Result};

pub mod aiprompt;
pub mod archive;
pub mod attrs;
pub mod auth;
pub mod clip;
pub mod cloud;
pub mod count;
pub mod dedup;
pub mod diff;
pub mod disk;
pub mod dirdiff;
pub mod du;
pub mod editor;
pub mod fileclip;
pub mod fuzzy;
pub mod elevate;
pub mod git;
pub mod proc;
pub mod grepedit;
pub mod highlight;
pub mod image;
pub mod inspect;
pub mod log;
pub mod markdown;
pub mod mermaid;
pub mod office;
pub mod ops;
pub mod os;
pub mod outline;
pub mod progress;
pub mod query;
pub mod rename;
pub mod search;
pub mod sharepoint;
pub mod shellwhere;
pub mod stamp;
pub mod substitute;
pub mod survey;
pub mod textops;
pub mod svn;
pub mod theme;
pub mod viewer;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    /// `name` lowercased once at read time, so sorting/filtering never has to
    /// re-lowercase inside the O(n log n) comparator.
    pub name_lower: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// Size in bytes. Meaningless for directories, which report `0`.
    pub len: u64,
    /// Last modification time, if the filesystem reports one.
    pub modified: Option<SystemTime>,
    /// A cloud placeholder: listed, but not downloaded (see [`crate::cloud`]).
    /// Reading it would pull it over the network, so sweeps skip it and the
    /// pane badges it.
    pub cloud: bool,
    /// True for the synthetic `..` row that steps up to the parent directory.
    /// It is navigable but never a target: it cannot be marked, copied, moved,
    /// renamed or deleted, and file operations skip it.
    pub is_parent: bool,
}

impl Entry {
    /// The synthetic `..` entry pointing at `parent`.
    fn parent_row(parent: PathBuf) -> Self {
        Self {
            name: "..".to_string(),
            name_lower: "..".to_string(),
            path: parent,
            is_dir: true,
            len: 0,
            modified: None,
            cloud: false,
            is_parent: true,
        }
    }

    /// An [`Entry`] for a flat listing — branch view or search results — where
    /// the row shows a path relative to the root (`rel`) instead of a bare name,
    /// so files from different folders stay distinguishable. Size/mtime are
    /// stat'd; a stat failure lists it with unknown size/time rather than
    /// dropping it. Backslashes are shown as `/` so the display is stable across
    /// platforms.
    pub fn flat(rel: &std::path::Path, path: PathBuf, is_dir: bool) -> Self {
        let name = rel.display().to_string().replace('\\', "/");
        let name_lower = name.to_lowercase();
        let meta = fs::symlink_metadata(&path).ok();
        let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = meta.as_ref().and_then(|m| m.modified().ok());
        let cloud = meta.as_ref().map(cloud::is_placeholder).unwrap_or(false);
        Self { name, name_lower, path, is_dir, len, modified, cloud, is_parent: false }
    }

    /// An [`Entry`] for a **remote** listing (SFTP): built from the values the
    /// server returned, with no local `stat` — the `path` holds the remote
    /// absolute path as a string. `is_up` marks the synthetic `..` row.
    pub fn remote(name: impl Into<String>, remote_path: impl Into<String>, is_dir: bool, size: u64, is_up: bool) -> Self {
        let name = name.into();
        Self {
            name_lower: name.to_lowercase(),
            name,
            path: PathBuf::from(remote_path.into()),
            is_dir,
            len: size,
            modified: None,
            cloud: false,
            is_parent: is_up,
        }
    }
}

/// Build an [`Entry`] straight from a `DirEntry` (Windows: its `metadata()` is
/// cached from the directory enumeration, so this is essentially free).
#[cfg(windows)]
fn entry_from_de(de: fs::DirEntry) -> Option<Entry> {
    let name = de.file_name().into_string().ok()?;
    let is_dir = de.file_type().ok()?.is_dir();
    let meta = de.metadata().ok();
    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let modified = meta.as_ref().and_then(|m| m.modified().ok());
    let cloud = meta.as_ref().map(cloud::is_placeholder).unwrap_or(false);
    let name_lower = name.to_lowercase();
    Some(Entry { name, name_lower, path: de.path(), is_dir, len, modified, cloud, is_parent: false })
}

/// Stat one raw `(name, path, is_dir)` into an [`Entry`]. `symlink_metadata`
/// (not `metadata`) matches `DirEntry::metadata`'s no-follow behaviour; a stat
/// failure (broken symlink, race) still lists the entry with unknown size/time.
#[cfg(not(windows))]
fn mk_entry((name, path, is_dir): (String, PathBuf, bool)) -> Entry {
    let meta = fs::symlink_metadata(&path).ok();
    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let modified = meta.as_ref().and_then(|m| m.modified().ok());
    let cloud = meta.as_ref().map(cloud::is_placeholder).unwrap_or(false);
    let name_lower = name.to_lowercase();
    Entry { name, name_lower, path, is_dir, len, modified, cloud, is_parent: false }
}

/// Stat every raw entry into an [`Entry`], fanning the per-file `stat` calls out
/// across threads. `stat` is latency-bound (especially on network filesystems),
/// so overlapping the calls is a big win; small directories skip the threads.
#[cfg(not(windows))]
fn stat_entries(raws: Vec<(String, PathBuf, bool)>) -> Vec<Entry> {
    let n = raws.len();
    let threads = std::thread::available_parallelism().map(|p| p.get()).unwrap_or(1).min(8);
    if n < 256 || threads <= 1 {
        return raws.into_iter().map(mk_entry).collect();
    }
    // Contiguous chunks so concatenating the results preserves readdir order
    // (apply_sort re-orders anyway, but a stable input keeps ties predictable).
    let chunk = n.div_ceil(threads);
    let mut buckets: Vec<Vec<(String, PathBuf, bool)>> = (0..threads).map(|_| Vec::new()).collect();
    for (i, r) in raws.into_iter().enumerate() {
        buckets[i / chunk].push(r);
    }
    let mut out: Vec<Entry> = Vec::with_capacity(n);
    std::thread::scope(|s| {
        let handles: Vec<_> = buckets
            .into_iter()
            .map(|b| s.spawn(move || b.into_iter().map(mk_entry).collect::<Vec<Entry>>()))
            .collect();
        for h in handles {
            if let Ok(part) = h.join() {
                out.extend(part);
            }
        }
    });
    out
}

/// Format a byte count the way a file manager should: short, aligned, and
/// never more than one decimal place.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "K", "M", "G", "T", "P", "E"];
    if bytes < 1024 {
        return format!("{}{}", bytes, UNITS[0]);
    }
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if v < 10.0 {
        format!("{:.1}{}", v, UNITS[unit])
    } else {
        format!("{:.0}{}", v, UNITS[unit])
    }
}

/// Format a timestamp as local `YYYY-MM-DD HH:MM`.
///
/// Uses chrono's `Local` rather than a hand-rolled offset: getting the zone
/// right means DST rules and per-platform system calls, and cian is built and
/// shipped for Windows from CI where that code could not be tested locally.
/// Current local time as `YYYYMMDD_HHMMSS`, for building log file names.
pub fn timestamp_compact() -> String {
    chrono::Local::now().format("%Y%m%d_%H%M%S").to_string()
}

pub fn format_time(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    dt.format("%Y-%m-%d %H:%M").to_string()
}

/// [`format_time`], remembered.
///
/// Formatting one costs about a microsecond — chrono resolves the local zone
/// for every call — and a file listing asks for the same eighty timestamps on
/// every frame it is on screen, which is eighty microseconds a frame to
/// recompute an answer that cannot have changed. A file's mtime is a fact
/// about the past.
///
/// Thread-local, so no lock; bounded, so a session that walks a million files
/// does not keep a string for each of them.
pub fn format_time_cached(t: SystemTime) -> String {
    use std::cell::RefCell;
    use std::collections::HashMap;
    /// Enough for any listing on any screen, several times over.
    const CAP: usize = 4096;
    thread_local! {
        static SEEN: RefCell<HashMap<u64, String>> = RefCell::new(HashMap::new());
    }
    let key = match t.duration_since(std::time::UNIX_EPOCH) {
        // Displayed to the minute, so that is what is keyed on.
        Ok(d) => d.as_secs() / 60,
        Err(_) => return format_time(t),
    };
    SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        if let Some(s) = seen.get(&key) {
            return s.clone();
        }
        let s = format_time(t);
        if seen.len() >= CAP {
            seen.clear();
        }
        seen.insert(key, s.clone());
        s
    })
}

/// What the listing is ordered by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Extension,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Modified => "date",
            SortKey::Extension => "ext",
        }
    }

    /// The order the picker offers, so the UI and the core agree.
    pub const ALL: [SortKey; 4] =
        [SortKey::Name, SortKey::Size, SortKey::Modified, SortKey::Extension];
}

/// How a pane's listing is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    /// Largest / newest / last-alphabetically first.
    pub reverse: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self { key: SortKey::Name, reverse: false }
    }
}

/// What a pane is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneView {
    /// The normal case: `reload` reads `cwd`.
    Dir,
    /// A flattened subtree (branch view) or a search-results listing rooted at
    /// `cwd`. The rows are handed in once — their `name` is a path relative to
    /// the root — and `reload` keeps them rather than reading a directory. The
    /// string labels the view in the pane title (e.g. "branch", "grep: todo").
    Flat(String),
    /// A directory on a remote host, browsed over SFTP. The rows are the ones
    /// the TUI fetched (it drives the SFTP calls and re-fetches on navigation);
    /// `reload` keeps them, like [`PaneView::Flat`]. `host` labels the title
    /// (e.g. `user@host`) and `path` is the remote working directory.
    Remote { host: String, path: String },
    /// Inside an archive, browsed as a folder. `archive` is the file on disk;
    /// `sub` the directory within it (`""` at the root, else `"a/b/"` with a
    /// trailing slash). The rows are synthesized by the TUI from the member
    /// list; `reload` keeps them, like the other synthetic views.
    Archive { archive: PathBuf, sub: String },
}

impl PaneView {
    /// A synthetic listing — not a live local directory — so `reload` must keep
    /// its rows instead of reading `cwd`, and the auto-refresh timer must leave
    /// it alone. True for both the flat/search view and a remote pane.
    fn is_synthetic(&self) -> bool {
        !matches!(self, PaneView::Dir)
    }
}

#[derive(Debug, Clone)]
pub struct Pane {
    pub cwd: PathBuf,
    /// Whether this is a live directory or a frozen flat/search listing.
    pub view: PaneView,
    /// The visible list: [`Pane::all_entries`] narrowed by [`Pane::filter`].
    /// Everything else (cursor, marks, file operations, rendering) works off
    /// this, so filtering automatically scopes them all to what is on screen.
    pub entries: Vec<Entry>,
    /// Every entry in `cwd`, before filtering.
    pub all_entries: Vec<Entry>,
    /// Case-insensitive substring that narrows the listing. Empty shows all.
    pub filter: String,
    /// Show entries whose name starts with a dot. Defaults to true, which is
    /// what cian has always done; most file managers hide them, so it is a
    /// toggle rather than a fixed choice.
    pub show_hidden: bool,
    /// Ordering of the listing.
    pub sort: Sort,
    pub cursor: usize,
    /// The first entry on screen. The view follows the cursor only when the
    /// cursor would leave it — it used to be derived from the cursor, with a
    /// formula that put the cursor on the *last* visible row, so clicking a
    /// file or jumping to one scrolled it to the bottom of the pane.
    pub scroll: usize,
    /// Marked entries keyed by full path (survives reload).
    pub marks: HashSet<PathBuf>,
    /// Recently visited paths for this pane (most recent first, deduped, capped).
    pub history: Vec<PathBuf>,
    /// Places `go_back` stepped away from, newest first — the other half of a
    /// browser's pair of arrows. Any fresh navigation clears it, because once
    /// you go somewhere new the branch you came back from is gone.
    pub forward: Vec<PathBuf>,
    /// `cwd`'s modification time as of the last read, used to notice changes
    /// made by anything other than cian.
    stamp: Option<SystemTime>,
}

const HISTORY_CAP: usize = 30;

impl Pane {
    pub fn new(cwd: impl Into<PathBuf>) -> Result<Self> {
        // `dunce` rather than `Path::canonicalize`, which on Windows returns an
        // extended-length path (`\\?\C:\...`). That prefix is a filesystem
        // convention the Windows *Shell* does not accept, so it would show up
        // in pane titles and, worse, break trashing a file — every entry path
        // is built by joining onto this one.
        let cwd = dunce::canonicalize(cwd.into()).context("invalid initial path")?;
        let mut pane = Self {
            cwd,
            view: PaneView::Dir,
            entries: Vec::new(),
            all_entries: Vec::new(),
            filter: String::new(),
            show_hidden: true,
            sort: Sort::default(),
            cursor: 0,
            scroll: 0,
            marks: HashSet::new(),
            history: Vec::new(),
            forward: Vec::new(),
            stamp: None,
        };
        pane.reload()?;
        pane.cursor_to_first_real();
        Ok(pane)
    }

    fn push_history(&mut self, path: PathBuf) {
        self.history.retain(|p| p != &path);
        self.history.insert(0, path);
        if self.history.len() > HISTORY_CAP {
            self.history.truncate(HISTORY_CAP);
        }
        // Navigating anywhere new ends the forward branch, exactly as a
        // browser drops "forward" once you follow a different link.
        self.forward.clear();
    }

    /// Read `cwd` as a plain directory listing again: out of any synthetic
    /// view, marks and filter dropped.
    ///
    /// Every arrival does this. It was written out six times, and the seventh
    /// place to forget one line of it would be a pane still holding the last
    /// folder's marks.
    fn reread(&mut self) -> Result<()> {
        self.view = PaneView::Dir;
        self.marks.clear();
        self.filter.clear();
        self.reload()
    }

    /// The same, with the cursor put on the first real row — which is what
    /// every arrival wants except climbing out, where the cursor belongs on
    /// the folder just left.
    fn arrive(&mut self) -> Result<()> {
        self.reread()?;
        self.cursor_to_first_real();
        Ok(())
    }

    /// Step back to the previously visited directory (`Alt+←`). The place we
    /// leave becomes the forward step, so the two arrows are inverses.
    pub fn go_back(&mut self) -> Result<bool> {
        let Some(prev) = self.history.first().cloned() else { return Ok(false) };
        if !prev.is_dir() {
            // It vanished while we were away; drop it and say nothing happened.
            self.history.remove(0);
            return Ok(false);
        }
        let leaving = self.cwd.clone();
        self.history.remove(0);
        self.forward.insert(0, leaving);
        self.cwd = prev;
        self.arrive()?;
        Ok(true)
    }

    /// Step forward again (`Alt+→`), undoing a [`Pane::go_back`].
    pub fn go_forward(&mut self) -> Result<bool> {
        let Some(next) = self.forward.first().cloned() else { return Ok(false) };
        if !next.is_dir() {
            self.forward.remove(0);
            return Ok(false);
        }
        let leaving = self.cwd.clone();
        self.forward.remove(0);
        self.history.insert(0, leaving);
        self.cwd = next;
        self.arrive()?;
        Ok(true)
    }

    /// Whether `cwd` has changed since it was last read.
    ///
    /// A directory's own mtime moves when an entry is added, removed or renamed,
    /// which covers "a file appeared while I was looking at this" — the case
    /// where a stale listing actively misleads. Checking one stat is cheap
    /// enough to do on a timer; re-reading the whole directory is not.
    ///
    /// But mtime is not reliable everywhere: **Windows/NTFS (and network shares)
    /// often do not update a directory's mtime promptly** when a file is added or
    /// removed, so a file dropped in from Explorer could sit invisible until
    /// something else moved the timestamp. So when mtime says "unchanged", fall
    /// back to comparing the entry count, which a fresh `read_dir` reports
    /// directly regardless of any timestamp caching. That extra read is cheap for
    /// ordinary directories (it never stats the entries) and is skipped for very
    /// large ones, where the single-stat mtime path is the right trade-off.
    pub fn is_stale(&self) -> bool {
        // A flat / search / remote listing is not backed by a live local
        // directory, so the auto-refresh timer must leave it alone — re-reading
        // `cwd` would throw the whole fetched set away.
        if self.view.is_synthetic() {
            return false;
        }
        let now = fs::metadata(&self.cwd).ok().and_then(|m| m.modified().ok());
        let mtime_changed = match (now, self.stamp) {
            (Some(a), Some(b)) => a != b,
            // No timestamp either time: nothing to compare, assume unchanged
            // rather than reloading forever.
            (None, None) => false,
            _ => true,
        };
        if mtime_changed {
            return true;
        }
        const COUNT_CHECK_MAX: usize = 20_000;
        if self.all_entries.len() <= COUNT_CHECK_MAX {
            if let Ok(rd) = fs::read_dir(&self.cwd) {
                // `read_dir` never yields `.`/`..`, and `all_entries` holds every
                // real entry (pre-filter, hidden included) — so the two counts
                // match exactly while the directory is unchanged.
                let count = rd.filter(|e| e.is_ok()).count();
                if count != self.all_entries.len() {
                    return true;
                }
            }
        }
        false
    }

    pub fn reload(&mut self) -> Result<()> {
        // A flat / search / remote listing has no local directory to re-read:
        // keep the rows we were given and just re-narrow and re-order them, so
        // `/` filter and the sort keys still work over the fetched set.
        if self.view.is_synthetic() {
            self.apply_sort();
            self.apply_filter();
            return Ok(());
        }
        self.stamp = fs::metadata(&self.cwd).ok().and_then(|m| m.modified().ok());
        let rd = fs::read_dir(&self.cwd)
            .with_context(|| format!("read_dir failed: {}", self.cwd.display()))?;
        // On Windows `DirEntry::metadata()` is free — size/mtime come from the
        // directory enumeration itself — so there is nothing to parallelise; the
        // straight read wins. On Unix each entry needs a `stat`, which is the
        // slow bit (worse on network mounts), so the size/mtime pass is fanned
        // out across threads (name/path/is_dir come cheaply from readdir).
        #[cfg(windows)]
        {
            self.all_entries = rd.filter_map(|r| r.ok()).filter_map(entry_from_de).collect();
        }
        #[cfg(not(windows))]
        {
            let raws: Vec<(String, PathBuf, bool)> = rd
                .filter_map(|res| res.ok())
                .filter_map(|de| {
                    let name = de.file_name().into_string().ok()?;
                    let is_dir = de.file_type().ok()?.is_dir();
                    Some((name, de.path(), is_dir))
                })
                .collect();
            self.all_entries = stat_entries(raws);
        }
        self.apply_sort();
        self.apply_filter();
        // Forget marks whose path no longer exists in this directory. This
        // checks the unfiltered list on purpose: narrowing the view must not
        // silently drop marks on entries the filter is hiding.
        let live: HashSet<PathBuf> = self.all_entries.iter().map(|e| e.path.clone()).collect();
        self.marks.retain(|p| live.contains(p));
        Ok(())
    }

    /// Order `all_entries` according to `sort`.
    ///
    /// Directories always come first regardless of key or direction — that is
    /// what navigation depends on, and burying folders among files to satisfy
    /// a size sort would make the pane much harder to move around in.
    fn apply_sort(&mut self) {
        let sort = self.sort;
        self.all_entries.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
                _ => {}
            }
            // Compares the pre-lowercased names (no per-comparison allocation).
            let by_name = |x: &Entry, y: &Entry| x.name_lower.cmp(&y.name_lower);
            let ord = match sort.key {
                SortKey::Name => by_name(a, b),
                // Ties fall back to name so the order is stable and predictable
                // rather than filesystem-dependent.
                SortKey::Size => a.len.cmp(&b.len).then_with(|| by_name(a, b)),
                SortKey::Modified => a.modified.cmp(&b.modified).then_with(|| by_name(a, b)),
                SortKey::Extension => {
                    // Extension off the already-lowercased name.
                    fn ext(e: &Entry) -> &str {
                        std::path::Path::new(&e.name_lower)
                            .extension()
                            .and_then(|x| x.to_str())
                            .unwrap_or("")
                    }
                    ext(a).cmp(ext(b)).then_with(|| by_name(a, b))
                }
            };
            if sort.reverse {
                ord.reverse()
            } else {
                ord
            }
        });
    }

    /// Change the ordering and re-apply it, keeping the filter intact.
    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = sort;
        self.apply_sort();
        self.apply_filter();
    }

    /// Rebuild `entries` from `all_entries` according to `filter` and
    /// `show_hidden`.
    fn apply_filter(&mut self) {
        let needle = self.filter.to_lowercase();
        let show_hidden = self.show_hidden;
        let mut entries: Vec<Entry> = self
            .all_entries
            .iter()
            .filter(|e| show_hidden || !e.name.starts_with('.'))
            .filter(|e| {
                // **An OR of ANDs** (`query::terms`): `仕事 週報` wants both,
                // `仕事 OR 家` takes either. One decision about what a query
                // means, in one place, so the terminal's `/` and the window's
                // filter do not agree only until somebody types two words. A
                // plain one-word filter goes down the same road and comes out
                // where it always did.
                needle.is_empty() || crate::query::hits(&e.name_lower, &needle)
            })
            .cloned()
            .collect();
        // A `..` row at the very top, so stepping up a level is a visible,
        // clickable target (as in classic file managers). Not at the filesystem
        // root, which has no parent. It always shows, even under a filter —
        // hiding the way out would be surprising — but never when it would not
        // match: it is navigation, not a listed file.
        // ...but a synthetic listing (flat / search / remote) is not a local
        // directory, so the local "up" row does not belong — the way out is to
        // leave the view (or, for a remote pane, the TUI drives the up-nav).
        if !self.view.is_synthetic() {
            if let Some(parent) = self.cwd.parent().map(|p| p.to_path_buf()) {
                entries.insert(0, Entry::parent_row(parent));
            }
        }
        self.entries = entries;
        if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len().saturating_sub(1);
        }
    }

    /// Narrow the listing. Passing an empty string shows everything again.
    pub fn set_filter(&mut self, filter: impl Into<String>) {
        self.filter = filter.into();
        self.apply_filter();
    }

    /// Show or hide dotfiles. Kept across directory changes, unlike the
    /// filter: it is a preference about how you want to look at things, not a
    /// query about one folder.
    pub fn set_show_hidden(&mut self, show: bool) {
        if self.show_hidden != show {
            self.show_hidden = show;
            self.apply_filter();
        }
    }

    /// Drop the filter. Called whenever the pane changes directory, since a
    /// filter left over from the previous folder would hide files the user
    /// has no reason to expect are missing.
    pub fn clear_filter(&mut self) {
        if !self.filter.is_empty() {
            self.filter.clear();
            self.apply_filter();
        }
    }

    /// True while showing a flat / search listing rather than a live directory.
    pub fn is_flat(&self) -> bool {
        matches!(self.view, PaneView::Flat(_))
    }

    /// The label of the current flat view, if any (for the pane title).
    pub fn flat_label(&self) -> Option<&str> {
        match &self.view {
            PaneView::Flat(l) => Some(l),
            _ => None,
        }
    }

    /// True while this pane is browsing a remote host over SFTP.
    pub fn is_remote(&self) -> bool {
        matches!(self.view, PaneView::Remote { .. })
    }

    /// True for any non-directory view (flat / search / remote) — where reload
    /// keeps the given rows and navigation is driven by the caller.
    pub fn is_synthetic(&self) -> bool {
        self.view.is_synthetic()
    }

    /// The `(host, path)` of the remote pane, if this is one — for the title and
    /// for the TUI to know where to fetch/navigate.
    pub fn remote_view(&self) -> Option<(&str, &str)> {
        match &self.view {
            PaneView::Remote { host, path } => Some((host, path)),
            _ => None,
        }
    }

    /// Does this listing hold any cloud placeholder? Drives the ☁ column, so
    /// an ordinary folder never pays for it.
    pub fn has_cloud(&self) -> bool {
        self.entries.iter().any(|e| e.cloud)
    }

    /// While browsing inside an archive: the archive file and the directory
    /// within it (`""` at the root, else with a trailing `/`).
    pub fn archive_view(&self) -> Option<(&std::path::Path, &str)> {
        match &self.view {
            PaneView::Archive { archive, sub } => Some((archive, sub)),
            _ => None,
        }
    }

    /// Show a directory inside an archive, with rows the TUI synthesized from
    /// the member list. Mirrors [`Pane::enter_remote`]: `cwd` stays the local
    /// directory holding the archive, so leaving the view lands back there.
    pub fn enter_archive(&mut self, archive: PathBuf, sub: String, entries: Vec<Entry>) {
        self.view = PaneView::Archive { archive, sub };
        self.all_entries = entries;
        self.filter.clear();
        self.marks.clear();
        self.cursor = 0;
        self.apply_sort();
        self.apply_filter();
        self.cursor_to_first_real();
    }

    /// Show a remote directory `path` on `host`, with the rows the TUI fetched
    /// over SFTP. Each row's `path` holds the remote absolute path (as an
    /// [`Entry`]), so the TUI can navigate/transfer from it. Marks and the filter
    /// are cleared — a new directory starts fresh.
    pub fn enter_remote(
        &mut self,
        host: impl Into<String>,
        path: impl Into<String>,
        entries: Vec<Entry>,
    ) {
        self.view = PaneView::Remote { host: host.into(), path: path.into() };
        self.all_entries = entries;
        self.filter.clear();
        self.marks.clear();
        self.cursor = 0;
        self.apply_sort();
        self.apply_filter();
    }

    /// Replace the listing with a flat set of rows — a flattened subtree (branch
    /// view) or search results. `cwd` stays the directory the view was launched
    /// from, so [`Pane::leave_flat`] returns there; each row's own `path` points
    /// at the real file, so marks and file operations act on it directly.
    pub fn enter_flat(&mut self, label: impl Into<String>, entries: Vec<Entry>) {
        self.view = PaneView::Flat(label.into());
        self.all_entries = entries;
        self.filter.clear();
        self.marks.clear();
        self.cursor = 0;
        self.apply_sort();
        self.apply_filter();
    }

    /// Leave a synthetic view (flat / search / remote) and read the local `cwd`
    /// again as an ordinary directory.
    pub fn leave_flat(&mut self) -> Result<()> {
        if self.view.is_synthetic() {
            self.arrive()?;
        }
        Ok(())
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len() as isize;
        let next = (self.cursor as isize + delta).clamp(0, len - 1);
        self.cursor = next as usize;
    }

    pub fn enter_selected(&mut self) -> Result<()> {
        if let Some(e) = self.entries.get(self.cursor).cloned() {
            if e.is_dir {
                let prev = self.cwd.clone();
                self.push_history(prev);
                self.cwd = e.path;
                // Stepping into a folder from a flat / search listing leaves the
                // flat view: the destination is a real directory to read.
                self.arrive()?;
            }
        }
        Ok(())
    }

    pub fn go_parent(&mut self) -> Result<()> {
        let parent_owned = self.cwd.parent().map(|p| p.to_path_buf());
        if let Some(parent) = parent_owned {
            let prev = self.cwd.clone();
            // The folder being left, so the cursor can land on it upstairs.
            let came_from = prev.file_name().map(|n| n.to_string_lossy().into_owned());
            self.push_history(prev);
            self.cwd = parent;
            self.reread()?;
            // Where you were, not where the listing starts.
            //
            // Going up from `abc/def` put the cursor on the first row of `abc`,
            // which is nowhere near `def` in a folder of any size — so climbing
            // one level and stepping back into it meant finding it again by
            // eye. Every file manager lands on the folder you came out of, and
            // it is the only row you are certainly interested in.
            match came_from.and_then(|name| self.entries.iter().position(|e| e.name == name)) {
                Some(i) => self.cursor = i,
                None => self.cursor_to_first_real(),
            }
        }
        Ok(())
    }

    /// Walk this pane to a directory named from outside — `:cd`, `z`, `o`,
    /// `O`, a bookmark, the place a grep hit lives.
    ///
    /// **This is not `Pane::new`.** The engine's `list` built a whole new
    /// pane for the path, which meant every one of those keys silently threw
    /// away the pane's history, its marks, its sort order and whether it was
    /// showing hidden files. `h` after `o` showed nothing, because `o` had
    /// just deleted the history it was going to show — reported as "自分の
    /// 履歴を塗り替えてしまってない？", and it was worse than overwriting.
    /// F5 went through the same door with the directory it was already in,
    /// so a plain refresh reset the sort.
    ///
    /// Arriving somewhere clears what belonged to the place you left — the
    /// filter, the marks, the archive or server view. It keeps what belongs
    /// to the pane: how you like it sorted, whether you want dotfiles, and
    /// where you have been.
    ///
    /// Standing still is not arriving. Asked for the directory it is already
    /// in, it re-reads and touches nothing else — otherwise `o` with the two
    /// panes already together would push the current directory onto its own
    /// history, and `h` would offer to take you where you are.
    pub fn go_to(&mut self, path: impl Into<PathBuf>) -> Result<()> {
        let path = dunce::canonicalize(path.into()).context("no such directory")?;
        if path == self.cwd && matches!(self.view, PaneView::Dir) {
            return self.reload();
        }
        let prev = self.cwd.clone();
        self.view = PaneView::Dir;
        self.filter.clear();
        self.marks.clear();
        self.push_history(prev);
        self.cwd = path;
        self.arrive()
    }

    pub fn jump_to(&mut self, path: PathBuf) -> Result<()> {
        let prev = self.cwd.clone();
        self.push_history(prev);
        self.cwd = path;
        self.arrive()?;
        Ok(())
    }

    /// Park the cursor on the first real entry, skipping the `..` row, so a
    /// freshly opened directory does not start with the cursor on "up a level".
    fn cursor_to_first_real(&mut self) {
        self.cursor = self.entries.iter().position(|e| !e.is_parent).unwrap_or(0);
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    pub fn toggle_mark_at(&mut self, idx: usize) {
        if let Some(e) = self.entries.get(idx) {
            if e.is_parent {
                return; // `..` is navigation, never a selection
            }
            let p = e.path.clone();
            if !self.marks.remove(&p) {
                self.marks.insert(p);
            }
        }
    }

    pub fn set_mark_at(&mut self, idx: usize) {
        if let Some(e) = self.entries.get(idx) {
            if e.is_parent {
                return;
            }
            self.marks.insert(e.path.clone());
        }
    }

    pub fn is_marked(&self, idx: usize) -> bool {
        self.entries
            .get(idx)
            .map(|e| self.marks.contains(&e.path))
            .unwrap_or(false)
    }

    pub fn clear_marks(&mut self) {
        self.marks.clear();
    }

    pub fn mark_count(&self) -> usize {
        self.marks.len()
    }

    /// Return marked paths, or if none marked, the cursor's path as a fallback.
    /// The synthetic `..` row is never a target — acting on the cursor while it
    /// sits on `..` (delete, copy, rename, …) yields nothing rather than
    /// operating on the parent directory.
    pub fn target_paths(&self) -> Vec<PathBuf> {
        if !self.marks.is_empty() {
            let mut v: Vec<PathBuf> = self.marks.iter().cloned().collect();
            v.sort();
            v
        } else if let Some(e) = self.selected().filter(|e| !e.is_parent) {
            vec![e.path.clone()]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pane over a temp dir containing `names` (all plain files).
    fn pane_with(names: &[&str]) -> (tempfile::TempDir, Pane) {
        let dir = tempfile::tempdir().unwrap();
        for n in names {
            fs::write(dir.path().join(n), b"").unwrap();
        }
        let pane = Pane::new(dir.path()).unwrap();
        (dir, pane)
    }

    /// Paths must stay in the form the Windows Shell understands.
    ///
    /// `Path::canonicalize` returns `\\?\C:\...` on Windows. That prefix is a
    /// filesystem convention the Shell rejects, so it would show up in pane
    /// titles and make trashing fail — and since every entry path is joined
    /// onto the pane's cwd, one bad root poisons all of them. This assertion
    /// only means anything on the Windows CI runner, which is the point.
    #[test]
    fn pane_paths_avoid_the_extended_length_prefix() {
        let (_d, pane) = pane_with(&["a.txt"]);
        assert!(pane.cwd.is_absolute(), "cwd should still be absolute");
        assert!(!pane.entries.is_empty());

        #[cfg(windows)]
        {
            assert!(
                !pane.cwd.to_string_lossy().starts_with(r"\\?\"),
                "extended-length prefix leaked into cwd: {:?}",
                pane.cwd
            );
            for e in &pane.entries {
                assert!(
                    !e.path.to_string_lossy().starts_with(r"\\?\"),
                    "extended-length prefix leaked into an entry: {:?}",
                    e.path
                );
            }
        }
    }

    #[test]
    fn sorting_by_size_orders_files_and_keeps_directories_first() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("zzz_folder")).unwrap();
        fs::write(dir.path().join("big.bin"), vec![0u8; 3000]).unwrap();
        fs::write(dir.path().join("mid.bin"), vec![0u8; 200]).unwrap();
        fs::write(dir.path().join("small.bin"), b"x").unwrap();
        let mut pane = Pane::new(dir.path()).unwrap();

        pane.set_sort(Sort { key: SortKey::Size, reverse: false });
        assert_eq!(
            names(&pane),
            vec!["zzz_folder", "small.bin", "mid.bin", "big.bin"],
            "directories stay on top even when sorting by size"
        );

        pane.set_sort(Sort { key: SortKey::Size, reverse: true });
        assert_eq!(names(&pane), vec!["zzz_folder", "big.bin", "mid.bin", "small.bin"]);
    }

    #[test]
    fn sorting_by_extension_then_name() {
        let (_d, mut pane) = pane_with(&["b.rs", "a.rs", "c.md"]);
        pane.set_sort(Sort { key: SortKey::Extension, reverse: false });
        // .md before .rs, and within .rs alphabetically.
        assert_eq!(names(&pane), vec!["c.md", "a.rs", "b.rs"]);
    }

    #[test]
    fn sorting_survives_reload_and_composes_with_the_filter() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("big.log"), vec![0u8; 3000]).unwrap();
        fs::write(dir.path().join("small.log"), b"x").unwrap();
        fs::write(dir.path().join("other.txt"), vec![0u8; 900]).unwrap();
        let mut pane = Pane::new(dir.path()).unwrap();

        pane.set_sort(Sort { key: SortKey::Size, reverse: true });
        pane.set_filter("log");
        assert_eq!(names(&pane), vec!["big.log", "small.log"], "sort applies within the filter");

        pane.reload().unwrap();
        assert_eq!(pane.sort.key, SortKey::Size, "reload must not reset the order");
        assert_eq!(names(&pane), vec!["big.log", "small.log"]);
    }

    #[test]
    fn default_order_is_name_ascending_with_directories_first() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("zdir")).unwrap();
        fs::write(dir.path().join("a.txt"), b"").unwrap();
        let pane = Pane::new(dir.path()).unwrap();
        assert_eq!(pane.sort, Sort::default());
        assert_eq!(names(&pane), vec!["zdir", "a.txt"]);
    }

    /// cian only ever reloaded after its own actions, so a file created by
    /// anything else never appeared.
    #[test]
    fn a_pane_notices_its_directory_changing_underneath_it() {
        let (dir, mut pane) = pane_with(&["a.txt"]);
        assert!(!pane.is_stale(), "nothing has happened yet");

        // Directory mtimes have coarse resolution on some filesystems; make
        // sure the change lands in a later tick.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(dir.path().join("appeared.txt"), b"x").unwrap();
        assert!(pane.is_stale(), "a new entry should show as stale");

        pane.reload().unwrap();
        assert!(!pane.is_stale(), "reloading clears it");
        assert!(names(&pane).contains(&"appeared.txt".to_string()));

        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::remove_file(dir.path().join("appeared.txt")).unwrap();
        assert!(pane.is_stale(), "a removed entry counts too");
        pane.reload().unwrap();
        assert!(!names(&pane).contains(&"appeared.txt".to_string()));
    }

    /// Windows/NTFS (and network shares) often do not bump a directory's mtime
    /// when a file is added, so a file dropped in from Explorer would stay
    /// invisible until F5. The entry-count fallback must catch it even with the
    /// timestamp pinned to its old value.
    #[test]
    fn a_new_entry_is_noticed_even_when_the_directory_mtime_does_not_move() {
        let (dir, mut pane) = pane_with(&["a.txt"]);
        assert!(!pane.is_stale());
        // The mtime the pane recorded at load time.
        let pinned = fs::metadata(dir.path()).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(dir.path().join("b.txt"), b"x").unwrap();
        // Simulate the OS not moving the directory timestamp on the add.
        filetime::set_file_mtime(dir.path(), filetime::FileTime::from_system_time(pinned)).unwrap();

        assert!(
            pane.is_stale(),
            "the new entry must be noticed via the count fallback, not just mtime"
        );
        pane.reload().unwrap();
        assert!(names(&pane).contains(&"b.txt".to_string()));
        // And with the mtime still pinned and the count now matching, it settles.
        filetime::set_file_mtime(dir.path(), filetime::FileTime::from_system_time(pinned)).unwrap();
        assert!(!pane.is_stale(), "no phantom change once the count agrees again");
    }

    #[test]
    fn hidden_entries_can_be_toggled_and_the_choice_outlives_a_reload() {
        let (_d, mut pane) = pane_with(&["a.txt", ".config", ".env"]);
        assert!(pane.show_hidden, "cian has always shown them; that is the default");
        assert_eq!(names(&pane).len(), 3);

        pane.set_show_hidden(false);
        assert_eq!(names(&pane), vec!["a.txt"]);

        // A preference about how to look at things, not a query about one
        // folder, so it survives a reload — unlike the filter.
        pane.reload().unwrap();
        assert_eq!(names(&pane), vec!["a.txt"]);

        pane.set_show_hidden(true);
        assert_eq!(names(&pane).len(), 3);
    }

    #[test]
    fn hiding_composes_with_the_filter() {
        let (_d, mut pane) = pane_with(&["notes.txt", ".notes.swp", "other.md"]);
        pane.set_show_hidden(false);
        pane.set_filter("notes");
        assert_eq!(names(&pane), vec!["notes.txt"], "the dotfile stays hidden");
    }

    /// The listed names, excluding the synthetic `..` navigation row (a temp
    /// dir always has a parent, so it is always present).
    fn names(pane: &Pane) -> Vec<String> {
        pane.entries.iter().filter(|e| !e.is_parent).map(|e| e.name.clone()).collect()
    }

    /// Count of real entries (without the `..` row).
    fn real_len(pane: &Pane) -> usize {
        pane.entries.iter().filter(|e| !e.is_parent).count()
    }

    #[test]
    fn a_remote_view_holds_fetched_rows_and_leaves_back_to_the_local_dir() {
        let (dir, mut pane) = pane_with(&["local.txt"]);
        // Rows as the TUI would build them from an SFTP listing (remote paths).
        let rows = vec![
            Entry::flat(std::path::Path::new("etc"), "/etc".into(), true),
            Entry::flat(std::path::Path::new("motd"), "/motd".into(), false),
        ];
        pane.enter_remote("root@web1", "/", rows);

        assert!(pane.is_remote());
        assert_eq!(pane.remote_view(), Some(("root@web1", "/")));
        // No local `..` row, and a remote pane never auto-refreshes off cwd.
        assert!(pane.entries.iter().all(|e| !e.is_parent), "no local up-row remotely");
        assert!(!pane.is_stale());
        // reload keeps the fetched rows (the TUI owns the SFTP calls).
        pane.reload().unwrap();
        assert_eq!(pane.entries.iter().filter(|e| !e.is_parent).count(), 2);

        // Leaving returns to the real local directory.
        pane.leave_flat().unwrap();
        assert!(!pane.is_remote());
        assert!(names(&pane).contains(&"local.txt".to_string()));
        assert!(pane.entries.iter().any(|e| e.is_parent), "the local up-row is back");
        let _ = dir;
    }

    #[test]
    fn a_flat_view_shows_relative_paths_has_no_up_row_and_survives_reload() {
        let (dir, mut pane) = pane_with(&["a.txt"]);
        let deep = dir.path().join("src/deep");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("main.rs"), b"x").unwrap();

        let rows = vec![
            Entry::flat(std::path::Path::new("a.txt"), dir.path().join("a.txt"), false),
            Entry::flat(std::path::Path::new("src/deep/main.rs"), deep.join("main.rs"), false),
        ];
        pane.enter_flat("branch", rows);

        assert!(pane.is_flat());
        assert_eq!(pane.flat_label(), Some("branch"));
        // Relative paths are the row names, and there is NO `..` row in a flat
        // view — the whole listing is real files.
        assert!(pane.entries.iter().all(|e| !e.is_parent), "no up-row in flat view");
        let mut got = names(&pane);
        got.sort();
        assert_eq!(got, vec!["a.txt", "src/deep/main.rs"]);

        // A flat listing is not backed by a directory, so it is never "stale"
        // and a reload keeps the rows (it only re-sorts / re-filters).
        assert!(!pane.is_stale());
        pane.reload().unwrap();
        assert_eq!(real_len(&pane), 2, "reload must not throw the flat set away");

        // The filter narrows the flattened set by relative path.
        pane.set_filter("deep");
        assert_eq!(names(&pane), vec!["src/deep/main.rs"]);
        pane.clear_filter();

        // Leaving returns to the live directory (with its `..` row back).
        pane.leave_flat().unwrap();
        assert!(!pane.is_flat());
        let back = names(&pane);
        assert!(back.contains(&"a.txt".to_string()) && back.contains(&"src".to_string()),
            "back to the real cwd listing: {:?}", back);
        assert!(pane.entries.iter().any(|e| e.is_parent), "the up-row is back");
    }

    #[test]
    fn filter_narrows_and_is_case_insensitive() {
        let (_d, mut pane) = pane_with(&["Alpha.rs", "beta.rs", "gamma.txt"]);
        assert_eq!(real_len(&pane), 3);

        pane.set_filter("RS");
        assert_eq!(names(&pane), vec!["Alpha.rs", "beta.rs"]);

        pane.set_filter("alp");
        assert_eq!(names(&pane), vec!["Alpha.rs"]);
    }

    #[test]
    fn clearing_filter_restores_every_entry() {
        let (_d, mut pane) = pane_with(&["a.txt", "b.txt", "c.md"]);
        pane.set_filter("md");
        assert_eq!(real_len(&pane), 1);
        pane.clear_filter();
        assert_eq!(real_len(&pane), 3);
    }

    #[test]
    fn filter_clamps_cursor_into_range() {
        let (_d, mut pane) = pane_with(&["a.txt", "b.txt", "c.txt"]);
        pane.cursor = 3;
        pane.set_filter("a.txt");
        // Only `..` and the one match survive, so the cursor must not dangle
        // past the end of the list.
        assert_eq!(real_len(&pane), 1);
        assert_eq!(pane.entries.len(), 2, "`..` plus the match");
        assert!(pane.cursor < pane.entries.len());
    }

    #[test]
    fn no_match_yields_empty_list_and_zero_cursor() {
        let (_d, mut pane) = pane_with(&["a.txt"]);
        pane.set_filter("zzz");
        // No real matches, but `..` stays as the way out.
        assert_eq!(real_len(&pane), 0);
        assert_eq!(pane.cursor, 0);
        assert!(pane.selected().map(|e| e.is_parent).unwrap_or(false), "only `..` remains");
        // `..` is never a target, so acting on the cursor yields nothing.
        assert!(pane.target_paths().is_empty());
    }

    /// `o`, `O`, `z`, `:cd` and F5 all went through the engine's `list`,
    /// which built a whole new `Pane` — so each of them silently emptied the
    /// history, the marks and the sort. Reported as `h` showing nothing after
    /// `o`.
    #[test]
    fn walking_to_a_directory_keeps_what_belongs_to_the_pane() {
        let (dir, mut pane) = pane_with(&["a.txt", "b.txt"]);
        let below = dir.path().join("sub");
        fs::create_dir(&below).unwrap();
        fs::write(below.join("c.txt"), b"").unwrap();

        pane.sort = Sort { key: SortKey::Size, reverse: true };
        pane.show_hidden = false;
        let from = pane.cwd.clone();

        pane.go_to(&below).unwrap();

        assert_eq!(pane.sort.key, SortKey::Size, "the sort is the pane's, not the directory's");
        assert!(pane.sort.reverse);
        assert!(!pane.show_hidden);
        assert_eq!(pane.history.first(), Some(&from), "where we came from is the history");
        assert!(pane.entries.iter().any(|e| e.name == "c.txt"));
    }

    /// Standing still is not arriving: `o` with both panes already together
    /// used to push the current directory onto its own history, so `h`
    /// offered to take you where you already were.
    #[test]
    fn going_where_we_already_are_does_not_touch_the_history() {
        let (_d, mut pane) = pane_with(&["a.txt"]);
        let here = pane.cwd.clone();
        pane.go_to(&here).unwrap();
        assert!(pane.history.is_empty(), "no move, no history entry");
        assert!(pane.entries.iter().any(|e| e.name == "a.txt"), "but it did re-read");
    }

    /// Regression guard: reload() prunes marks against the *unfiltered* list,
    /// so a mark on a hidden entry must survive a reload while filtered.
    #[test]
    fn reload_while_filtered_keeps_marks_on_hidden_entries() {
        let (_d, mut pane) = pane_with(&["keep.txt", "hidden.md"]);
        let hidden = pane
            .all_entries
            .iter()
            .find(|e| e.name == "hidden.md")
            .unwrap()
            .path
            .clone();
        pane.marks.insert(hidden.clone());

        pane.set_filter("keep");
        assert_eq!(names(&pane), vec!["keep.txt"]);

        pane.reload().unwrap();
        assert!(pane.marks.contains(&hidden), "mark on a filtered-out entry was dropped");
    }

    #[test]
    fn filter_survives_reload_but_not_directory_change() {
        let (dir, mut pane) = pane_with(&["a.txt", "b.md"]);
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub").join("inner.txt"), b"").unwrap();

        pane.set_filter("md");
        pane.reload().unwrap();
        assert_eq!(pane.filter, "md", "reload must not drop the filter");

        pane.jump_to(dir.path().join("sub")).unwrap();
        assert_eq!(pane.filter, "", "changing directory must clear the filter");
        assert_eq!(names(&pane), vec!["inner.txt"]);
    }

    #[test]
    fn target_paths_prefers_marks_over_cursor() {
        let (_d, mut pane) = pane_with(&["a.txt", "b.txt"]);
        // Move off the `..` row (index 0) onto a real entry first.
        pane.cursor = 1;
        assert_eq!(pane.target_paths().len(), 1, "falls back to the cursor");
        // set_mark_at skips `..`, so marking indices 1 and 2 marks both files.
        pane.set_mark_at(1);
        pane.set_mark_at(2);
        assert_eq!(pane.target_paths().len(), 2);
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn human_size_scales_and_stays_short() {
        assert_eq!(human_size(0), "0B");
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(1024), "1.0K");
        assert_eq!(human_size(1536), "1.5K");
        assert_eq!(human_size(10 * 1024), "10K");
        assert_eq!(human_size(1024 * 1024), "1.0M");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0G");
        // Never wider than 5 columns, so the column can be fixed-width.
        for n in [0u64, 1, 999, 1023, 1024, u64::MAX] {
            assert!(human_size(n).len() <= 5, "{} -> {}", n, human_size(n));
        }
    }

    /// The bug this replaced: timestamps rendered in UTC instead of local
    /// time. 2021-01-01 00:00 UTC is 09:00 the same day in JST, so a UTC
    /// implementation shows the wrong hour (and, near midnight, wrong date).
    #[test]
    fn format_time_uses_the_local_zone_not_utc() {
        let t = UNIX_EPOCH + Duration::from_secs(1_609_459_200); // 2021-01-01 00:00 UTC
        let local = chrono::DateTime::<chrono::Local>::from(t);
        let offset_secs = local.offset().local_minus_utc() as i64;

        let s = format_time(t);
        // Derive what local time *should* be from the offset the OS reports,
        // so this passes in any zone the test happens to run in.
        let expect = chrono::DateTime::<chrono::Utc>::from(t) + chrono::Duration::seconds(offset_secs);
        assert_eq!(s, expect.format("%Y-%m-%d %H:%M").to_string());

        // And specifically: in a +09:00 zone this instant must read 09:00.
        if offset_secs == 9 * 3600 {
            assert_eq!(s, "2021-01-01 09:00", "JST should be UTC+9");
        }
    }

    #[test]
    fn format_time_renders_a_sortable_stamp() {
        // 2021-01-01 00:00:00 UTC
        let t = UNIX_EPOCH + Duration::from_secs(1_609_459_200);
        let s = format_time(t);
        assert_eq!(s.len(), 16, "fixed width for column alignment: {:?}", s);
        assert!(s.starts_with("202"), "{}", s);
        // Shape must be YYYY-MM-DD HH:MM regardless of the machine's zone.
        let bytes = s.as_bytes();
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert_eq!(bytes[10], b' ');
        assert_eq!(bytes[13], b':');
    }
}

#[cfg(test)]
mod perf_bench {
    use super::*;
    #[test]
    #[ignore]
    fn bench_reload_and_sort_big_dir() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..5000 { std::fs::write(d.path().join(format!("file_{i:05}.rs")), b"x").unwrap(); }
        let mut p = Pane::new(d.path()).unwrap();
        // warm
        p.reload().unwrap();
        let n = 50;
        let t = std::time::Instant::now();
        for _ in 0..n { p.reload().unwrap(); }
        println!("reload: {:?}/call (5000 files)", t.elapsed()/n);
        let t = std::time::Instant::now();
        for i in 0..n { p.set_sort(Sort{ key: if i%2==0 {SortKey::Name} else {SortKey::Size}, reverse:false }); }
        println!("set_sort: {:?}/call", t.elapsed()/n);
    }
}
#[test]
#[ignore]
fn bench_formatters() {
    use std::time::{Instant, SystemTime, Duration, UNIX_EPOCH};
    let times: Vec<SystemTime> =
        (0..80).map(|i| UNIX_EPOCH + Duration::from_secs(1_700_000_000 + i * 3600)).collect();
    let t = Instant::now();
    let mut n = 0usize;
    for _ in 0..1000 {
        for &ts in &times {
            n += crate::format_time(ts).len();
        }
    }
    eprintln!("format_time: {:?} per call ({n})", t.elapsed() / 80_000);
    let t = Instant::now();
    let mut n = 0usize;
    for _ in 0..1000 {
        for i in 0..80u64 {
            n += crate::human_size(i * 12345).len();
        }
    }
    eprintln!("human_size: {:?} per call ({n})", t.elapsed() / 80_000);
}

#[cfg(test)]
mod log_destination_tests {
    /// A log that cannot be written where it was asked for lands somewhere it
    /// can, rather than nowhere at all. The path that prompted this —
    /// `%USERPROFILE%\Desktop` on a machine whose Desktop is OneDrive's — does
    /// not exist, and the silence cost an evening.
    ///
    /// The resolution happens once per process, so this is one test rather than
    /// several: a second would see the first one's answer.
    #[test]
    fn a_log_path_is_resolved_once_and_never_silently() {
        // Off unless asked for, which is the normal case.
        if std::env::var_os("CIAN_LOG").is_none() {
            assert!(crate::log::destination().is_none(), "no CIAN_LOG, no log");
            assert!(!crate::log::enabled());
            return;
        }
        // Asked for: wherever it ended up, it takes a line.
        let where_it_went = crate::log::destination().expect("CIAN_LOG is set");
        crate::log::log("a line from the test suite");
        assert!(where_it_went.exists(), "the log exists at {}", where_it_went.display());
    }
}

/// A transfer cap written the way a person writes one: `2M`, `500k`, `off`.
///
/// Moved here from cian-tui so both front ends read the same string the same
/// way. Two parsers for one setting is two answers to "how fast is 2M".
pub fn parse_rate(text: &str) -> Option<u64> {
    let t = text.trim().to_lowercase();
    let t = t.strip_suffix("/s").unwrap_or(&t).trim().to_string();
    let t = t.strip_suffix("bps").or_else(|| t.strip_suffix('b')).unwrap_or(&t).trim().to_string();
    if t.is_empty() || t == "off" || t == "none" || t == "0" {
        return None;
    }
    let (num, scale) = match t.chars().last()? {
        'k' => (&t[..t.len() - 1], 1_000f64),
        'm' => (&t[..t.len() - 1], 1_000_000f64),
        'g' => (&t[..t.len() - 1], 1_000_000_000f64),
        _ => (t.as_str(), 1f64),
    };
    let n: f64 = num.trim().parse().ok()?;
    if n <= 0.0 {
        return None;
    }
    Some((n * scale) as u64)
}
