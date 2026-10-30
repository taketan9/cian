//! Replace across the files a grep found — the bulk half of `:s`.
//!
//! Sakura Editor's grep-replace is the feature this exists for: grep a tree,
//! look at what came back, and change it everywhere in one pass. The dangerous
//! part is not the matching, it is that a bulk write has no undo, so the rules
//! here are all about being able to look before leaping:
//!
//! * [`plan`] only reads. It hands back every line it would change, with the
//!   before and after text, so the caller can show them and let the user
//!   uncheck the ones it got wrong. Nothing is written until [`apply`].
//! * [`apply`] re-reads each file and refuses any line whose text no longer
//!   matches what the plan showed. Between planning and applying, a log file
//!   can grow and an editor can save; writing a line number computed against
//!   an older copy of the file is how bulk tools eat data.
//! * A file that cannot be round-tripped losslessly is refused rather than
//!   rewritten. Being told "3 files skipped, here is why" is recoverable;
//!   discovering later that a binary was mangled is not.
//!
//! It also owns its own read and write rather than borrowing the F3 viewer's,
//! because the viewer expands tabs for display. That is right for looking at a
//! file and wrong for writing one back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::substitute::{self, Substitution};
use crate::viewer::{Eol, TextEncoding};

/// Refuse anything larger than this. It is the same ceiling grep uses, so a
/// file that could produce hits is always one replace can also write back.
pub const MAX_BYTES: u64 = crate::search::MAX_GREP_BYTES;

/// A text file read for editing, with everything needed to write it back the
/// way it arrived: its encoding, its byte-order mark, and its line ending.
#[derive(Debug, Clone)]
pub struct TextFile {
    pub lines: Vec<String>,
    pub encoding: TextEncoding,
    pub bom: bool,
    pub eol: Eol,
    /// The file ended with a line break. Without remembering this, every save
    /// either adds a trailing newline to files that had none or strips one
    /// from files that had it — a diff on every file the replace touched.
    pub trailing_eol: bool,
}

/// Read `path` as text, preserving what a save has to put back.
///
/// Tabs stay tabs — unlike the viewer's read, which expands them so columns
/// line up on screen. A replace that turned every tab in a Makefile into
/// spaces would be a considerably worse bug than the one being fixed.
pub fn read_text(path: &Path) -> Result<TextFile> {
    let meta = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if meta.is_dir() {
        anyhow::bail!("is a directory");
    }
    if meta.len() > MAX_BYTES {
        anyhow::bail!("larger than {} MB", MAX_BYTES / (1024 * 1024));
    }
    if crate::cloud::skip_meta(&meta) {
        anyhow::bail!("not downloaded (cloud placeholder)");
    }
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        anyhow::bail!("looks binary");
    }
    let (encoding, bom) = match () {
        _ if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) => (TextEncoding::Utf8, true),
        _ if bytes.starts_with(&[0xFF, 0xFE]) => (TextEncoding::Utf16Le, true),
        _ if bytes.starts_with(&[0xFE, 0xFF]) => (TextEncoding::Utf16Be, true),
        _ => {
            // The same UTF-8-then-Shift_JIS order grep uses, for the same
            // reason: the logs and batch output this meets are still SJIS.
            let (_, _, bad) = encoding_rs::UTF_8.decode(&bytes);
            if bad {
                let (_, _, sjis_bad) = encoding_rs::SHIFT_JIS.decode(&bytes);
                if sjis_bad {
                    anyhow::bail!("neither UTF-8 nor Shift_JIS");
                }
                (TextEncoding::ShiftJis, false)
            } else {
                (TextEncoding::Utf8, false)
            }
        }
    };
    let text = encoding.decode(&bytes);
    let eol = Eol::detect(&text);
    let trailing_eol = text.ends_with('\n') || text.ends_with('\r');
    Ok(TextFile { lines: text.lines().map(str::to_string).collect(), encoding, bom, eol, trailing_eol })
}

/// Write `file` back to `path` in the encoding, BOM and line ending it had.
pub fn write_text(path: &Path, file: &TextFile) -> Result<()> {
    let sep = file.eol.as_str();
    let mut text = file.lines.join(sep);
    if file.trailing_eol {
        text.push_str(sep);
    }
    let mut bytes = Vec::new();
    if file.bom {
        bytes.extend_from_slice(match file.encoding {
            TextEncoding::Utf8 => &[0xEF, 0xBB, 0xBF][..],
            TextEncoding::Utf16Le => &[0xFF, 0xFE][..],
            TextEncoding::Utf16Be => &[0xFE, 0xFF][..],
            TextEncoding::ShiftJis => &[][..],
        });
    }
    let lost = file.encoding.cannot_hold(&text);
    if !lost.is_empty() {
        anyhow::bail!(
            "{} is Shift_JIS, which has no {} — not saved",
            path.display(),
            lost.iter().collect::<String>()
        );
    }
    bytes.extend_from_slice(&file.encoding.encode(&text));
    std::fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

/// One line a replace would change, as it will be shown for approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    /// 0-based index into the file's lines. Displayed as `line + 1`.
    pub line: usize,
    pub before: String,
    pub after: String,
    /// Whether this one is included. Everything starts checked: the common
    /// case is "yes, all of them", and unchecking the exceptions is less work
    /// than checking the rest.
    pub picked: bool,
}

/// A file the replace will not touch, and why — always reported rather than
/// dropped, so "0 hits in that one" is never confused with "never looked".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    pub why: String,
}

/// Work out every change `sub` would make across `paths`, without writing.
pub fn plan(paths: &[PathBuf], sub: &Substitution) -> (Vec<Change>, Vec<Skipped>) {
    let mut changes = Vec::new();
    let mut skipped = Vec::new();
    for path in paths {
        let file = match read_text(path) {
            Ok(f) => f,
            Err(e) => {
                skipped.push(Skipped { path: path.clone(), why: root_cause(&e) });
                continue;
            }
        };
        let hits = substitute::find(sub, &file.lines, None);
        // `apply` may split lines when the replacement holds a newline, which
        // would make a line-for-line pairing lie. Group per source line and
        // rebuild that line's own result instead.
        let mut by_line: BTreeMap<usize, Vec<substitute::Hit>> = BTreeMap::new();
        for h in hits {
            by_line.entry(h.line).or_default().push(h);
        }
        for (no, group) in by_line {
            let Some(before) = file.lines.get(no) else { continue };
            let one = [before.clone()];
            let flat: Vec<substitute::Hit> =
                group.into_iter().map(|h| substitute::Hit { line: 0, ..h }).collect();
            let after = substitute::apply(&one, &flat).join("\n");
            if &after == before {
                continue;
            }
            changes.push(Change { path: path.clone(), line: no, before: before.clone(), after, picked: true });
        }
    }
    (changes, skipped)
}

/// What [`apply`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub files: usize,
    pub lines: usize,
    /// Lines whose text had changed on disk since the plan was made, so the
    /// replace declined to touch them.
    pub stale: usize,
    pub errors: Vec<String>,
}

/// Write the picked changes.
///
/// Each file is re-read and every line checked against the `before` the user
/// approved. A file with even one stale line still gets its other lines
/// written — the alternative, refusing the whole file, punishes the user for
/// something a log appender did.
pub fn apply(changes: &[Change]) -> Report {
    let mut report = Report::default();
    let mut by_file: BTreeMap<&Path, Vec<&Change>> = BTreeMap::new();
    for c in changes.iter().filter(|c| c.picked) {
        by_file.entry(c.path.as_path()).or_default().push(c);
    }
    for (path, group) in by_file {
        let mut file = match read_text(path) {
            Ok(f) => f,
            Err(e) => {
                report.errors.push(format!("{}: {}", path.display(), root_cause(&e)));
                continue;
            }
        };
        let mut touched = 0usize;
        // Highest line first, so a replacement holding a newline can grow the
        // buffer without shifting the lines still to be written.
        let mut group = group;
        group.sort_by_key(|c| std::cmp::Reverse(c.line));
        for c in group {
            match file.lines.get(c.line) {
                Some(cur) if cur == &c.before => {}
                _ => {
                    report.stale += 1;
                    continue;
                }
            }
            let split: Vec<String> = c.after.split('\n').map(str::to_string).collect();
            file.lines.splice(c.line..=c.line, split);
            touched += 1;
        }
        if touched == 0 {
            continue;
        }
        match write_text(path, &file) {
            Ok(()) => {
                report.files += 1;
                report.lines += touched;
            }
            Err(e) => report.errors.push(format!("{}: {}", path.display(), root_cause(&e))),
        }
    }
    report
}

/// The innermost message of an error chain — "read /x/y: permission denied"
/// says more in a one-line report than the outermost context alone.
fn root_cause(e: &anyhow::Error) -> String {
    e.chain().last().map(|c| c.to_string()).unwrap_or_else(|| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("cian-grepedit-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }
    fn write(name: &str, bytes: &[u8]) -> PathBuf {
        let p = tmp().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// A replace must hand the file back the way it found it: same encoding,
    /// same BOM, same line ending, same tabs. Each of those has its own way of
    /// going silently wrong.
    #[test]
    fn a_file_survives_the_round_trip_it_arrived_in() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("crlf.txt", b"one\r\ntwo\r\n".to_vec()),
            ("tabs.mk", b"target:\n\tcommand one\n".to_vec()),
            ("no-final-nl.txt", b"one\ntwo".to_vec()),
            ("bom.txt", [&[0xEF, 0xBB, 0xBF][..], "one\ntwo\n".as_bytes()].concat()),
            ("sjis.txt", encoding_rs::SHIFT_JIS.encode("one\n日本語\n").0.into_owned()),
        ];
        for (name, bytes) in cases {
            let p = write(name, &bytes);
            let f = read_text(&p).unwrap();
            write_text(&p, &f).unwrap();
            assert_eq!(std::fs::read(&p).unwrap(), bytes, "{name} changed by a no-op save");
        }
    }

    #[test]
    fn plan_reads_only_and_reports_what_it_would_not_touch() {
        let a = write("plan-a.txt", b"ORA-600 here\nfine\nORA-600 again\n");
        let bin = write("plan-b.bin", b"\0\0\0");
        let sub = substitute::parse("s/ORA-600/ORA-7445/g").unwrap();
        let (changes, skipped) = plan(&[a.clone(), bin.clone()], &sub);

        assert_eq!(changes.len(), 2, "one per changed line, not per file");
        assert_eq!(changes[0].line, 0);
        assert_eq!(changes[0].after, "ORA-7445 here");
        assert!(changes.iter().all(|c| c.picked), "everything starts checked");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].path, bin);
        assert_eq!(
            std::fs::read(&a).unwrap(),
            b"ORA-600 here\nfine\nORA-600 again\n",
            "planning must not write",
        );
    }

    #[test]
    fn only_the_picked_changes_are_written() {
        let p = write("picked.txt", b"a\na\na\n");
        let sub = substitute::parse("s/a/b/").unwrap();
        let (mut changes, _) = plan(std::slice::from_ref(&p), &sub);
        changes[1].picked = false;
        let r = apply(&changes);
        assert_eq!((r.files, r.lines, r.stale), (1, 2, 0));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "b\na\nb\n");
    }

    /// The whole point of re-reading at apply time: a line that moved on since
    /// the preview is left alone rather than overwritten from a stale plan.
    #[test]
    fn a_line_that_changed_since_the_preview_is_refused() {
        let p = write("stale.txt", b"keep\nORA-600\n");
        let sub = substitute::parse("s/ORA-600/fixed/").unwrap();
        let (changes, _) = plan(std::slice::from_ref(&p), &sub);
        assert_eq!(changes.len(), 1);
        // Someone else edits the file in the meantime.
        std::fs::write(&p, b"keep\nsomething else entirely\n").unwrap();
        let r = apply(&changes);
        assert_eq!((r.files, r.lines, r.stale), (0, 0, 1));
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "keep\nsomething else entirely\n",
            "the other edit is not clobbered",
        );
    }

    /// A replacement holding a newline splits one line into several. Applied
    /// bottom-up, the lines below must still be the ones that were approved.
    #[test]
    fn a_replacement_that_adds_lines_does_not_shift_the_others() {
        let p = write("split.txt", b"a;b\nx\na;b\n");
        let sub = substitute::parse(r"s/;/;\n/g").unwrap();
        let (changes, _) = plan(std::slice::from_ref(&p), &sub);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].after, "a;\nb", "the preview shows the split");
        let r = apply(&changes);
        assert_eq!((r.files, r.lines, r.stale), (1, 2, 0));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a;\nb\nx\na;\nb\n");
    }

    #[test]
    fn a_file_with_no_hits_is_left_completely_alone() {
        let p = write("untouched.txt", b"nothing to see\n");
        let sub = substitute::parse("s/zzz/yyy/").unwrap();
        let (changes, skipped) = plan(std::slice::from_ref(&p), &sub);
        assert!(changes.is_empty() && skipped.is_empty());
        assert_eq!(apply(&changes), Report::default());
    }
}
