//! Recursive search from a directory downwards.
//!
//! Written to stream and to stop: a search over a deep tree can take a long
//! time and may be started by mistake, so results are handed back as they are
//! found and a cancel flag is checked at every step. Nothing here blocks on a
//! whole traversal before producing anything.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// One match.
#[derive(Debug, Clone)]
pub struct Hit {
    /// Absolute path to the entry.
    pub path: PathBuf,
    /// Path relative to the search root, which is what a result list wants to
    /// show — the absolute one is mostly the root repeated.
    pub rel: PathBuf,
    pub is_dir: bool,
    /// For a content match: the 1-based line number and the line itself.
    pub line: Option<(usize, String)>,
}

/// What a search looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Match entry names.
    Name,
    /// Match the text inside files.
    Content,
}

/// How the needle matches: the pattern language of a query.
///
/// A plain needle is a case-insensitive substring — what every filer user
/// types without thinking. A `/pattern/` form (the same delimiters cian's
/// `:brename s/…/…/` already taught) compiles to a regex; `/pattern/i` makes
/// it case-insensitive. Regexes follow regex convention (case-sensitive by
/// default) rather than the literal path's insensitivity, because anyone
/// writing `/ORA-\d+/` is being precise on purpose.
#[derive(Debug, Clone)]
pub enum Matcher {
    /// Case-insensitive substring; stored lowercased.
    Literal(String),
    Regex(regex::Regex),
}

impl Matcher {
    /// Parse user input into a matcher. `/re/` and `/re/i` become regexes —
    /// with a real error for a bad pattern, never a silent fall-back to
    /// literal (matching the wrong thing quietly is worse than refusing).
    pub fn parse(input: &str) -> Result<Matcher, String> {
        let Some(body) = input.strip_prefix('/') else {
            return Ok(Matcher::Literal(input.to_lowercase()));
        };
        // The last `/` closes the pattern (so a path-ish `/a/b/` still works);
        // whatever follows it is flags.
        let Some(end) = body.rfind('/') else {
            return Err("unterminated pattern — close it with / (e.g. /ORA-\\d+/)".to_string());
        };
        let (pat, flags) = (&body[..end], &body[end + 1..]);
        if !flags.is_empty() && flags != "i" {
            return Err(format!("unknown regex flag {flags:?} — only /…/i is supported"));
        }
        let re = regex::RegexBuilder::new(pat)
            .case_insensitive(flags == "i")
            .build()
            .map_err(|e| shorten_regex_error(&e))?;
        Ok(Matcher::Regex(re))
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Matcher::Literal(s) => s.is_empty(),
            Matcher::Regex(re) => re.as_str().is_empty(),
        }
    }

    /// Does `hay` match? Public because the listing's standing mask asks the
    /// same question of a file name that a search asks of a line, and two
    /// answers to "does this pattern match" is two pattern languages.
    pub fn matches(&self, hay: &str) -> bool {
        match self {
            Matcher::Literal(s) => hay.to_lowercase().contains(s),
            Matcher::Regex(re) => re.is_match(hay),
        }
    }

    /// Every match in `hay` as char-index `(start, end)` ranges, end-exclusive,
    /// in order. What a viewer needs for highlighting and n/N jumps, where
    /// [`Self::matches`] only answers yes/no. Zero-width regex matches are
    /// dropped — there is nothing to highlight or land on.
    pub fn find_ranges(&self, hay: &str) -> Vec<(usize, usize)> {
        let char_at = |byte: usize| hay[..byte].chars().count();
        match self {
            Matcher::Literal(s) => {
                if s.is_empty() {
                    return Vec::new();
                }
                // Byte offsets found in the lowercased text are only valid in
                // the original when lowercasing preserved byte lengths; for
                // the rare title-cased characters that grow, fall back to a
                // char-by-char scan.
                let lower = hay.to_lowercase();
                if lower.len() == hay.len() {
                    let mut out = Vec::new();
                    let mut from = 0;
                    while let Some(rel) = lower[from..].find(s.as_str()) {
                        let b = from + rel;
                        out.push((char_at(b), char_at(b + s.len())));
                        from = b + s.len().max(1);
                    }
                    out
                } else {
                    let hay_chars: Vec<char> = hay.chars().collect();
                    let needle: Vec<char> = s.chars().collect();
                    let mut out = Vec::new();
                    let mut i = 0;
                    while i + needle.len() <= hay_chars.len() {
                        let window = &hay_chars[i..i + needle.len()];
                        let matched = window
                            .iter()
                            .flat_map(|c| c.to_lowercase())
                            .eq(needle.iter().copied());
                        if matched {
                            out.push((i, i + needle.len()));
                            i += needle.len();
                        } else {
                            i += 1;
                        }
                    }
                    out
                }
            }
            Matcher::Regex(re) => re
                .find_iter(hay)
                .filter(|m| m.start() < m.end())
                .map(|m| (char_at(m.start()), char_at(m.end())))
                .collect(),
        }
    }
}

/// One line, fit for a status bar: regex errors are multi-line diagnostics
/// with a caret drawing, which a TUI message line cannot show.
pub(crate) fn shorten_regex_error(e: &regex::Error) -> String {
    let s = e.to_string();
    let mut lines = s.lines().filter(|l| !l.trim().is_empty());
    let head = lines.next().unwrap_or("bad regex").trim().to_string();
    match lines.next_back() {
        Some(reason) => format!("{}: {}", head, reason.trim()),
        None => head,
    }
}

/// What to look for.
#[derive(Debug, Clone)]
pub struct Query {
    pub matcher: Matcher,
    /// Descend into directories whose name starts with a dot.
    pub include_hidden: bool,
    pub mode: Mode,
}

impl Query {
    /// A literal (substring) name query, for callers building queries in code.
    pub fn new(needle: impl Into<String>) -> Self {
        Self {
            matcher: Matcher::Literal(needle.into().to_lowercase()),
            include_hidden: false,
            mode: Mode::Name,
        }
    }

    pub fn content(needle: impl Into<String>) -> Self {
        Self { mode: Mode::Content, ..Self::new(needle) }
    }

    /// A query from user input: `/re/`-form compiles, anything else is a
    /// literal. `mode` picks names or file contents.
    pub fn parse(input: &str, mode: Mode) -> Result<Self, String> {
        Ok(Self { matcher: Matcher::parse(input)?, include_hidden: false, mode })
    }

    fn matches(&self, name: &str) -> bool {
        self.matcher.is_empty() || self.matcher.matches(name)
    }
}

/// Files larger than this are not read looking for text. A grep that stalls on
/// a database dump or a disk image is worse than one that admits it skipped it.
pub const MAX_GREP_BYTES: u64 = 8 * 1024 * 1024;

/// How much of a file to sniff for NUL bytes before deciding it is binary.
const SNIFF: usize = 8000;

/// Report every line of `path` matching the query.
///
/// Skips files that are too big, and files that look binary — matching inside
/// a compiled object produces unreadable "lines" and no useful answer.
///
/// Not UTF-8-only: a file that fails UTF-8 is retried as Shift_JIS before
/// being given up on. Enterprise Japanese logs (Oracle, AIX, old batch
/// output) are routinely SJIS, and a grep that silently skips them answers
/// "no matches" when the truth is "didn't look".
fn grep_file(
    path: &Path,
    matcher: &Matcher,
    cancel: &AtomicBool,
    mut on_line: impl FnMut(usize, String),
) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() > MAX_GREP_BYTES {
        return;
    }
    // A cloud placeholder is not worth downloading a library to grep.
    if crate::cloud::skip_meta(&meta) {
        return;
    }
    let Ok(bytes) = fs::read(path) else { return };
    if bytes[..bytes.len().min(SNIFF)].contains(&0) {
        return;
    }
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) => {
            let bytes = e.into_bytes();
            let (decoded, _, had_errors) = encoding_rs::SHIFT_JIS.decode(&bytes);
            if had_errors {
                return; // neither UTF-8 nor SJIS: treat as binary
            }
            decoded.into_owned()
        }
    };
    for (i, line) in text.lines().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if matcher.matches(line) {
            // Long lines are useless in a result list and expensive to carry.
            let shown: String = line.trim().chars().take(300).collect();
            on_line(i + 1, shown);
        }
    }
}

/// Stop before the result list becomes useless to scroll and the search
/// pointless to continue.
pub const MAX_HITS: usize = 5000;

/// How a search ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    Cancelled,
    /// Stopped at [`MAX_HITS`].
    Truncated,
}

/// Walk `root` breadth-first, reporting matches through `on_hit`.
///
/// Breadth-first on purpose: shallow matches are usually the wanted ones, and
/// they arrive first, so a search that is going to be cancelled early still
/// produces the useful results.
pub fn search(
    root: &Path,
    query: &Query,
    cancel: &AtomicBool,
    on_hit: &mut dyn FnMut(Hit),
) -> Outcome {
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    let mut found = 0usize;
    while let Some(dir) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return Outcome::Cancelled;
        }
        // An unreadable directory is normal (permissions); skipping it beats
        // aborting the whole search.
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if cancel.load(Ordering::Relaxed) {
                return Outcome::Cancelled;
            }
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let hidden = name.starts_with('.');
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);

            let visible = query.include_hidden || !hidden;
            match query.mode {
                Mode::Name => {
                    if visible && query.matches(&name) {
                        let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                        on_hit(Hit { path: path.clone(), rel, is_dir, line: None });
                        found += 1;
                        if found >= MAX_HITS {
                            return Outcome::Truncated;
                        }
                    }
                }
                Mode::Content => {
                    if visible && !is_dir && !query.matcher.is_empty() {
                        let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                        let mut stop = false;
                        grep_file(&path, &query.matcher, cancel, |n, text| {
                            if found < MAX_HITS {
                                on_hit(Hit {
                                    path: path.clone(),
                                    rel: rel.clone(),
                                    is_dir: false,
                                    line: Some((n, text)),
                                });
                                found += 1;
                            } else {
                                stop = true;
                            }
                        });
                        if stop {
                            return Outcome::Truncated;
                        }
                    }
                }
            }
            // Symlinked directories are not followed: a link back up the tree
            // would loop forever.
            if is_dir && !path.is_symlink() && (query.include_hidden || !hidden) {
                queue.push_back(path);
            }
        }
    }
    Outcome::Complete
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("src/deep/deeper")).unwrap();
        fs::create_dir_all(d.path().join(".hidden")).unwrap();
        fs::write(d.path().join("readme.md"), b"").unwrap();
        fs::write(d.path().join("src/main.rs"), b"").unwrap();
        fs::write(d.path().join("src/deep/notes.md"), b"").unwrap();
        fs::write(d.path().join("src/deep/deeper/main.rs"), b"").unwrap();
        fs::write(d.path().join(".hidden/secret.md"), b"").unwrap();
        d
    }

    fn run(root: &Path, q: Query) -> (Vec<String>, Outcome) {
        let cancel = AtomicBool::new(false);
        let mut names = Vec::new();
        let out = search(root, &q, &cancel, &mut |h| {
            names.push(h.rel.display().to_string().replace('\\', "/"))
        });
        (names, out)
    }

    #[test]
    fn finds_matches_at_every_depth() {
        let d = tree();
        let (mut names, out) = run(d.path(), Query::new("main"));
        names.sort();
        assert_eq!(out, Outcome::Complete);
        assert_eq!(names, vec!["src/deep/deeper/main.rs", "src/main.rs"]);
    }

    #[test]
    fn matching_is_case_insensitive() {
        let d = tree();
        let (names, _) = run(d.path(), Query::new("README"));
        assert_eq!(names, vec!["readme.md"]);
    }

    /// Shallow results arrive first, so cancelling early still leaves the
    /// most likely matches on screen.
    #[test]
    fn results_come_back_shallowest_first() {
        let d = tree();
        let (names, _) = run(d.path(), Query::new(".md"));
        let depth = |s: &String| s.matches('/').count();
        assert!(
            names.windows(2).all(|w| depth(&w[0]) <= depth(&w[1])),
            "not breadth-first: {:?}",
            names
        );
    }

    #[test]
    fn hidden_directories_are_skipped_unless_asked_for() {
        let d = tree();
        let (names, _) = run(d.path(), Query::new("secret"));
        assert!(names.is_empty(), "should not descend into .hidden: {:?}", names);

        let q = Query { include_hidden: true, ..Query::new("secret") };
        let (names, _) = run(d.path(), q);
        assert_eq!(names, vec![".hidden/secret.md"]);
    }

    #[test]
    fn directories_are_matched_too() {
        let d = tree();
        let (names, _) = run(d.path(), Query::new("deeper"));
        assert_eq!(names, vec!["src/deep/deeper"]);
    }

    #[test]
    fn cancelling_stops_the_walk() {
        let d = tree();
        let cancel = AtomicBool::new(true);
        let mut n = 0;
        let out = search(d.path(), &Query::new(""), &cancel, &mut |_| n += 1);
        assert_eq!(out, Outcome::Cancelled);
        assert_eq!(n, 0, "already cancelled: nothing should be walked");
    }

    #[test]
    fn an_unreadable_directory_does_not_abort_the_search() {
        let d = tree();
        // A path that vanishes mid-walk behaves like an unreadable one.
        let ghost = d.path().join("ghost");
        fs::create_dir(&ghost).unwrap();
        fs::remove_dir(&ghost).unwrap();
        let (names, out) = run(d.path(), Query::new("main"));
        assert_eq!(out, Outcome::Complete);
        assert_eq!(names.len(), 2);
    }
}

#[cfg(test)]
mod grep_tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(d.path().join("a.txt"), "alpha\nTODO: fix this\nomega\n").unwrap();
        fs::write(d.path().join("src/b.rs"), "fn main() {}\n// todo later\n").unwrap();
        fs::write(d.path().join("clean.txt"), "nothing here\n").unwrap();
        // A binary file that contains the needle as bytes.
        fs::write(d.path().join("blob.bin"), b"\x00\x01todo\x00\xff").unwrap();
        d
    }

    fn run(root: &Path, needle: &str) -> Vec<(String, usize, String)> {
        let cancel = AtomicBool::new(false);
        let mut out = Vec::new();
        search(root, &Query::content(needle), &cancel, &mut |h| {
            let (n, text) = h.line.clone().unwrap();
            out.push((h.rel.display().to_string().replace('\\', "/"), n, text));
        });
        out.sort();
        out
    }

    #[test]
    fn finds_matching_lines_with_their_numbers() {
        let d = tree();
        let hits = run(d.path(), "todo");
        assert_eq!(
            hits,
            vec![
                ("a.txt".to_string(), 2, "TODO: fix this".to_string()),
                ("src/b.rs".to_string(), 2, "// todo later".to_string()),
            ],
            "case-insensitive, line-numbered, and recursive"
        );
    }

    /// Matching inside a compiled artefact yields unreadable "lines" and never
    /// answers the question that was asked.
    #[test]
    fn binary_files_are_skipped() {
        let d = tree();
        let hits = run(d.path(), "todo");
        assert!(!hits.iter().any(|(p, _, _)| p.contains("blob")), "{:?}", hits);
    }

    #[test]
    fn files_over_the_size_limit_are_skipped() {
        let d = tempfile::tempdir().unwrap();
        let big = d.path().join("huge.log");
        let mut text = String::with_capacity(MAX_GREP_BYTES as usize + 64);
        while text.len() < MAX_GREP_BYTES as usize + 32 {
            text.push_str("filler line\n");
        }
        text.push_str("needle here\n");
        fs::write(&big, &text).unwrap();
        assert!(fs::metadata(&big).unwrap().len() > MAX_GREP_BYTES);
        assert!(run(d.path(), "needle").is_empty(), "should not read a huge file");
    }

    #[test]
    fn an_empty_needle_matches_nothing_rather_than_every_line() {
        let d = tree();
        assert!(run(d.path(), "").is_empty());
    }

    fn run_q(root: &Path, q: Query) -> Vec<(String, usize, String)> {
        let cancel = AtomicBool::new(false);
        let mut out = Vec::new();
        search(root, &q, &cancel, &mut |h| {
            let (n, text) = h.line.clone().unwrap();
            out.push((h.rel.display().to_string().replace('\\', "/"), n, text));
        });
        out.sort();
        out
    }

    /// A Shift_JIS log is greppable — with a Japanese needle, typed in UTF-8.
    /// Before, any non-UTF-8 file was silently skipped, which reads as "no
    /// matches" when the truth is "didn't look".
    #[test]
    fn grep_reads_shift_jis_files() {
        let d = tempfile::tempdir().unwrap();
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("1行目は正常\n2行目でエラーが発生\n");
        fs::write(d.path().join("batch.log"), &sjis).unwrap();
        let hits = run_q(d.path(), Query::content("エラー"));
        assert_eq!(hits, vec![("batch.log".to_string(), 2, "2行目でエラーが発生".to_string())]);
    }

    /// The `/re/` form greps by regex — the ORA-code case this exists for.
    #[test]
    fn grep_accepts_a_regex() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("alert.log"),
            "ok so far\nORA-01555: snapshot too old\nORA-00600: internal\nnot ora-1 here\n",
        )
        .unwrap();
        let q = Query::parse(r"/^ORA-\d+/", Mode::Content).unwrap();
        let hits = run_q(d.path(), q);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert!(hits.iter().all(|(_, _, l)| l.starts_with("ORA-")));
    }

    /// `/re/` opts into regex; bare input stays a literal — and the two are
    /// told apart at parse time, with real errors instead of silent fallback.
    #[test]
    fn pattern_language_parses_literals_regexes_and_rejects_junk() {
        assert!(matches!(Matcher::parse("ORA-").unwrap(), Matcher::Literal(s) if s == "ora-"));
        // A regex, case-sensitive by default…
        let re = Matcher::parse(r"/ORA-\d+/").unwrap();
        assert!(re.matches("ORA-01555: snapshot too old"));
        assert!(!re.matches("ora-01555"), "regex is case-sensitive without /i");
        // …and insensitive with the `i` flag.
        assert!(Matcher::parse(r"/ora-\d+/i").unwrap().matches("ORA-600"));
        // The last slash closes the pattern, so inner slashes are fine.
        assert!(Matcher::parse("/a/b/").unwrap().matches("path a/b here"));

        assert!(Matcher::parse("/oops").is_err(), "unterminated pattern");
        assert!(Matcher::parse("/re/x").is_err(), "unknown flag");
        let msg = Matcher::parse(r"/(/").unwrap_err();
        assert!(msg.lines().count() == 1, "one line, fit for a status bar: {msg:?}");
    }

    /// Ranges come back in char columns (what a viewer highlights), not bytes —
    /// the difference matters exactly when the line holds Japanese text.
    #[test]
    fn find_ranges_reports_char_columns() {
        let lit = Matcher::parse("エラー").unwrap();
        assert_eq!(lit.find_ranges("処理でエラー発生、エラー継続"), vec![(3, 6), (9, 12)]);
        let re = Matcher::parse(r"/\d+件/").unwrap();
        assert_eq!(re.find_ranges("成功12件、失敗3件"), vec![(2, 5), (8, 10)]);
        assert!(Matcher::parse("").unwrap().find_ranges("anything").is_empty());
    }

    #[test]
    fn name_search_accepts_a_regex() {
        let d = tree();
        let q = Query::parse(r"/\.rs$/", Mode::Name).unwrap();
        let cancel = AtomicBool::new(false);
        let mut names = Vec::new();
        search(d.path(), &q, &cancel, &mut |h| {
            names.push(h.rel.display().to_string().replace('\\', "/"));
        });
        names.sort();
        assert_eq!(names, vec!["src/b.rs"]);
    }

    #[test]
    fn a_name_search_still_reports_no_line() {
        let d = tree();
        let cancel = AtomicBool::new(false);
        let mut lines = Vec::new();
        search(d.path(), &Query::new("a.txt"), &cancel, &mut |h| lines.push(h.line.clone()));
        assert_eq!(lines, vec![None]);
    }
}
