//! What is actually in a tree, as facts an AI request can be built from.
//!
//! The three AI features that read a directory — spot the junk, propose a
//! structure, find the file I mean — were each sending the model a list of
//! **names**. Junk was the worst served: it saw one level, so a
//! `node_modules` two folders down was invisible; and a directory's size came
//! through blank, so the one question junk exists to answer — *what is taking
//! the space* — was one the model had no way to reason about. It was being
//! asked to guess from vocabulary alone, and it guessed like something
//! guessing from vocabulary alone.
//!
//! So this gathers evidence and judges nothing. **The junk list lives in the
//! prompt, not here**: the moment this module starts deciding that `target/`
//! is disposable, there are two opinions in the program and the model's is the
//! one nobody can read. What it contributes is what only the filesystem knows
//! — how big, how old, how deep, how many.
//!
//! Breadth first, like the finder, so a cap takes the deepest things rather
//! than everything after the first big folder. And a cap is **reported**: a
//! survey that quietly stopped at row 400 reads to everyone downstream as a
//! directory with 400 things in it.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// One entry of a survey.
#[derive(Debug, Clone)]
pub struct Row {
    /// Where it is, relative to the surveyed root. Always uses `/` so the
    /// listing reads the same on both platforms — the model is shown one
    /// convention rather than being asked to cope with two.
    pub rel: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// Bytes at or below it. **Recursive for a directory**, which is the whole
    /// reason this is not just a `read_dir`.
    pub size: u64,
    /// The size is a floor, not a total: summing this subtree hit
    /// [`SIZE_ENTRY_CAP`] and stopped. Rendered with a `>`, and it is still
    /// the answer — "more than two gigabytes" ranks a folder as well as the
    /// exact figure and gets there in a fraction of the time.
    pub size_capped: bool,
    /// When it last changed. `None` where the filesystem would not say.
    pub modified: Option<SystemTime>,
    /// How far below the root, with the root's own children at 1.
    pub depth: usize,
}

/// How far to go. Both bounds exist to keep one keystroke from walking a home
/// directory, and both are reported when they bite.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Levels below the root. 1 is a plain listing.
    pub depth: usize,
    /// The most rows to return.
    pub rows: usize,
    /// Whether to look inside dot-directories. Off for the tidy-up features
    /// (`.git` is not clutter), on where the person asked for a file by name.
    pub hidden: bool,
    /// How many directory entries the *whole* survey may visit while totalling
    /// subtree sizes.
    ///
    /// **One budget for the survey, not one per directory.** Capping each
    /// directory separately still cost four and a half seconds over a Rust
    /// checkout, because eight hundred rows each spent their own allowance and
    /// the nested ones walked the same files again. Spent in breadth-first
    /// order, this gives the accurate figures to the shallow rows — where
    /// every question this feeds is actually answered — and lets the deep ones
    /// say "not counted" instead of costing a second each.
    pub size_budget: usize,
}

impl Default for Limits {
    fn default() -> Self {
        // Three levels reaches `project/src/module` and the usual homes of
        // build output, without turning one keypress into a full-disk walk.
        Self { depth: 3, rows: 600, hidden: false, size_budget: 120_000 }
    }
}

/// A survey, and whether it is the whole truth.
#[derive(Debug, Clone, Default)]
pub struct Survey {
    pub rows: Vec<Row>,
    /// The row budget ran out partway through this depth, and nothing deeper
    /// was looked at.
    ///
    /// **Said as a depth rather than as a count on purpose.** The first
    /// version counted what would not fit, and over a Rust checkout it
    /// reported "42160 entries did not fit" — true, useless, and alarming.
    /// The walk is breadth first, so what it actually did was list the top two
    /// levels completely and stop, which is both a far more useful sentence
    /// and the thing somebody would want to know.
    pub stopped_at: Option<usize>,
    /// Directories not opened because the depth limit stopped there.
    pub unopened: usize,
}

impl Survey {
    /// Whether anything was left out, either way.
    pub fn partial(&self) -> bool {
        self.stopped_at.is_some() || self.unopened > 0
    }

    /// The deepest level that is known to be complete. `None` when the walk
    /// stopped inside the first level, where nothing can be claimed.
    pub fn whole_to(&self) -> Option<usize> {
        match self.stopped_at {
            None => self.rows.iter().map(|r| r.depth).max(),
            Some(1) => None,
            Some(d) => Some(d - 1),
        }
    }
}

/// Walk `root`, breadth first, within `limits`.
///
/// Directory sizes are totalled over the *whole* subtree, including the part
/// below the depth limit: the limit bounds what is listed, not what is
/// counted. A folder that is only interesting because it is four gigabytes has
/// to arrive saying so even when its contents are not listed.
pub fn survey(root: &Path, limits: Limits, cancel: &AtomicBool) -> Survey {
    let mut out = Survey::default();
    let mut budget = limits.size_budget;
    let mut queue = VecDeque::from([(root.to_path_buf(), 0usize)]);
    while let Some((dir, depth)) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return out;
        }
        // An unreadable directory is normal (permissions); skipping it beats
        // abandoning the survey.
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if cancel.load(Ordering::Relaxed) {
                return out;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if !limits.hidden && name.starts_with('.') {
                continue;
            }
            let path = e.path();
            let ft = e.file_type();
            // A symlink is itself, never followed — the same rule `du` uses,
            // and the reason a survey cannot loop.
            let link = ft.as_ref().map(|t| t.is_symlink()).unwrap_or(false);
            let is_dir = !link && ft.as_ref().map(|t| t.is_dir()).unwrap_or(false);
            let meta = e.metadata().ok();
            let modified = meta.as_ref().and_then(|m| m.modified().ok());
            let (size, size_capped) = if is_dir {
                subtree_size(&path, cancel, &mut budget)
            } else {
                (meta.as_ref().map(|m| m.len()).unwrap_or(0), false)
            };
            // **Full: stop, rather than counting what will not fit.** The
            // walk is breadth first, so the rows already gathered are the
            // shallowest ones — which for every question this feeds is where
            // the answer is. `target` matters at depth 1; the ten thousand
            // object files inside it do not, and enumerating them to say how
            // many were skipped cost a second and told nobody anything.
            if out.rows.len() >= limits.rows {
                out.stopped_at = Some(depth + 1);
                return out;
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.rows.push(Row { rel, path: path.clone(), is_dir, size, size_capped, modified, depth: depth + 1 });
            if is_dir {
                if depth + 1 < limits.depth {
                    queue.push_back((path, depth + 1));
                } else {
                    out.unopened += 1;
                }
            }
        }
    }
    out
}

/// How many entries one directory's size may cost before the answer becomes a
/// floor.
///
/// **This is a latency bound, and it was bought with a regression.** The first
/// version summed every subtree exactly, which over a Rust checkout meant
/// walking fourteen gigabytes of build output — six seconds, inside the
/// request handler, with the whole engine waiting on it. The window would have
/// looked frozen for the one keystroke whose entire purpose is finding that
/// directory.
///
/// Twenty thousand entries is well past the point where the number changes any
/// decision: nothing that takes this many files to count is going to turn out
/// to be small.
pub const SIZE_ENTRY_CAP: usize = 20_000;

/// Every regular file at or below `dir`, in bytes, and whether that is a floor.
/// Iterative, so a deep tree cannot blow the stack; symlinks are not followed.
///
/// Spends from `budget`, the survey's shared allowance, as well as its own
/// [`SIZE_ENTRY_CAP`]: one directory may not eat the whole survey, and the
/// survey as a whole may not eat the keystroke.
///
/// **A capped sum is never presented as a total.** A size that quietly stopped
/// counting is a wrong number wearing a right number's clothes, and ranking by
/// it would put the biggest directory in the middle of the list.
fn subtree_size(dir: &Path, cancel: &AtomicBool, budget: &mut usize) -> (u64, bool) {
    if *budget == 0 {
        return (0, true);
    }
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return (total, true);
        }
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            *budget = budget.saturating_sub(1);
            if seen > SIZE_ENTRY_CAP || *budget == 0 {
                return (total, true);
            }
            let ft = e.file_type();
            if ft.as_ref().map(|t| t.is_symlink()).unwrap_or(false) {
                continue;
            }
            if ft.as_ref().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(e.path());
            } else {
                total += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (total, false)
}

/// How many whole days ago, against `now`. `None` where the time is unknown or
/// in the future (a clock skew, an unpacked archive) — a negative age reads as
/// a bug in the listing rather than as what it is.
pub fn age_days(modified: Option<SystemTime>, now: SystemTime) -> Option<u64> {
    let m = modified?;
    now.duration_since(m).ok().map(|d| d.as_secs() / 86_400)
}

/// Bytes, as a person reads them. Kept here rather than in the caller because
/// three prompts render the same column and they must not disagree about what
/// "1.5G" means.
pub fn brief_size(bytes: u64) -> String {
    const UNITS: [(u64, &str); 4] =
        [(1 << 30, "G"), (1 << 20, "M"), (1 << 10, "K"), (1, "B")];
    for (scale, tag) in UNITS {
        if bytes >= scale {
            // One decimal below ten, none above: "9.4G" and "512M" are both
            // as much precision as the number can carry.
            let v = bytes as f64 / scale as f64;
            if v >= 10.0 || scale == 1 {
                return format!("{}{tag}", v.round() as u64);
            }
            // …but not a decimal point that says nothing. "4.0G" spends two
            // characters to tell you it is exactly four, which it is not.
            let one = format!("{v:.1}");
            return match one.strip_suffix(".0") {
                Some(whole) => format!("{whole}{tag}"),
                None => format!("{one}{tag}"),
            };
        }
    }
    "0B".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sandbox() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        fs::create_dir_all(r.join("src/deep/deeper")).unwrap();
        fs::create_dir_all(r.join("node_modules/pkg")).unwrap();
        fs::create_dir(r.join(".git")).unwrap();
        fs::write(r.join("README.md"), vec![b'x'; 100]).unwrap();
        fs::write(r.join("src/main.rs"), vec![b'x'; 200]).unwrap();
        fs::write(r.join("src/deep/deeper/buried.rs"), vec![b'x'; 400]).unwrap();
        fs::write(r.join("node_modules/pkg/index.js"), vec![b'x'; 8000]).unwrap();
        fs::write(r.join(".git/HEAD"), vec![b'x'; 20]).unwrap();
        d
    }

    fn find<'a>(s: &'a Survey, rel: &str) -> &'a Row {
        s.rows.iter().find(|r| r.rel == rel).unwrap_or_else(|| panic!("no row {rel}: {:?}", s.rows.iter().map(|r| &r.rel).collect::<Vec<_>>()))
    }

    /// A directory arrives with what is *under* it, which is the number the
    /// whole feature turns on. It used to arrive blank.
    #[test]
    fn a_directory_carries_its_subtree() {
        let d = sandbox();
        let s = survey(d.path(), Limits::default(), &AtomicBool::new(false));
        assert_eq!(find(&s, "node_modules").size, 8000, "the package, not the folder entry");
        assert_eq!(find(&s, "src").size, 600, "200 here and 400 buried");
    }

    /// …including the part below the depth limit. The limit bounds the
    /// listing, not the arithmetic: a folder that matters only because it is
    /// huge has to say so even when its contents are not shown.
    #[test]
    fn the_depth_limit_does_not_shrink_the_sizes() {
        let d = sandbox();
        let shallow = survey(
            d.path(),
            Limits { depth: 1, ..Limits::default() },
            &AtomicBool::new(false),
        );
        assert!(!shallow.rows.iter().any(|r| r.rel.contains('/')), "one level listed");
        assert_eq!(find(&shallow, "src").size, 600, "still counted to the bottom");
        assert!(shallow.unopened >= 2, "and it says which doors it did not open");
    }

    /// Dot-directories are not clutter, and the tidy-up features must not be
    /// shown `.git` as a candidate.
    #[test]
    fn hidden_is_a_choice() {
        let d = sandbox();
        let without = survey(d.path(), Limits::default(), &AtomicBool::new(false));
        assert!(!without.rows.iter().any(|r| r.rel.starts_with(".git")));
        let with = survey(
            d.path(),
            Limits { hidden: true, ..Limits::default() },
            &AtomicBool::new(false),
        );
        assert!(with.rows.iter().any(|r| r.rel == ".git"));
    }

    /// **A cap that is not reported is a lie.** "Nothing found" has to be
    /// distinguishable from "nothing found in the part I looked at".
    #[test]
    fn a_full_survey_says_so() {
        let d = sandbox();
        let s = survey(
            d.path(),
            Limits { rows: 2, ..Limits::default() },
            &AtomicBool::new(false),
        );
        assert_eq!(s.rows.len(), 2);
        assert_eq!(s.stopped_at, Some(1), "it stopped inside the first level");
        assert_eq!(s.whole_to(), None, "so no level is known to be complete");
        assert!(s.partial());
        // Deep enough to reach the bottom of the sandbox. At the default
        // depth of 3 this very tree has a door left shut (`src/deep/deeper`),
        // and `partial()` says so — which is the behaviour, not a flaw in the
        // fixture.
        let whole = survey(
            d.path(),
            Limits { depth: 6, ..Limits::default() },
            &AtomicBool::new(false),
        );
        assert!(!whole.partial(), "reaching the bottom claims nothing extra");
        assert_eq!(whole.whole_to(), Some(4), "and it knows how deep it went");

        // Stopping *after* a level means that level is whole. This is the
        // sentence the depth form exists to make sayable.
        let two = survey(
            d.path(),
            Limits { rows: 6, depth: 6, ..Limits::default() },
            &AtomicBool::new(false),
        );
        // Six rows is the whole top level (3) and the whole level below it
        // (3); the seventh would have been the first at depth 3.
        assert_eq!(two.stopped_at, Some(3));
        assert_eq!(two.whole_to(), Some(2), "both levels above it are complete");
    }

    #[test]
    fn age_refuses_to_be_negative() {
        let now = SystemTime::now();
        assert_eq!(age_days(Some(now - Duration::from_secs(86_400 * 3)), now), Some(3));
        assert_eq!(age_days(Some(now + Duration::from_secs(86_400)), now), None, "clock skew");
        assert_eq!(age_days(None, now), None);
    }

    #[test]
    fn sizes_read_like_sizes() {
        assert_eq!(brief_size(0), "0B");
        assert_eq!(brief_size(999), "999B");
        assert_eq!(brief_size(1536), "1.5K");
        assert_eq!(brief_size(4 << 30), "4G", "no decimal point that says nothing");
        assert_eq!(brief_size(20 * (1 << 20)), "20M");
        assert_eq!(brief_size(3 * (1 << 30) + (1 << 29)), "3.5G");
    }
}
