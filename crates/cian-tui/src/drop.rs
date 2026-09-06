//! Files dragged from the desktop onto cian.
//!
//! A terminal program cannot take part in the OS drag-and-drop protocol — the
//! window belongs to the terminal emulator, not to us, so cian can never be a
//! *drag source*. It can be a target, though, because every terminal answers a
//! dropped file the same way: it types the path in, as if you had pasted it.
//! cian already receives that as a bracketed paste, so the whole feature is
//! recognising when a paste is really a drop.
//!
//! Recognising, not guessing: [`dropped_paths`] returns nothing unless every
//! item it parsed exists on disk. A paste of prose stays a paste of prose.
//!
//! Terminals disagree about the escaping, so all the common forms are read:
//!
//! | Terminal | What arrives |
//! |---|---|
//! | iTerm2, Terminal.app | `/Users/x/My\ File.txt` — spaces backslash-escaped |
//! | Windows Terminal | `"C:\Users\x\My File.txt"` — quoted |
//! | GNOME Terminal, Konsole | `file:///home/x/My%20File.txt` — a URI |
//! | several | one path per line |

use std::path::PathBuf;

/// The existing files named by a dropped/pasted blob, or empty when it is not
/// a drop at all.
///
/// All-or-nothing on existence: a blob where some items are paths and some are
/// words is prose that happens to mention a file, not a drop.
pub(crate) fn dropped_paths(text: &str) -> Vec<PathBuf> {
    read_dropped(text, ESCAPES_HERE, |p| p.symlink_metadata().is_ok())
}

/// Does a backslash escape the next character on this platform?
///
/// On Unix a terminal hands over `My\ File.txt` and the backslash is punctuation.
/// On Windows it is the path separator and escapes nothing — `C:\Users\taro`
/// must come through with every one of them intact. Named rather than written
/// as `cfg!(windows)` at the point of use so both readings can be tested from
/// either platform: this is exactly the kind of difference that is only ever
/// discovered on the machine one does not develop on.
const ESCAPES_HERE: bool = !cfg!(windows);

/// The reading of a drop, with the escaping convention spelled out and the
/// "is it really there" test handed in.
pub(crate) fn read_dropped(
    text: &str,
    escapes: bool,
    exists: impl Fn(&PathBuf) -> bool,
) -> Vec<PathBuf> {
    let exists_all = |paths: &[PathBuf]| !paths.is_empty() && paths.iter().all(&exists);
    // One item per line: the reading that keeps a name with a space in it.
    let whole = || -> Vec<PathBuf> {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| PathBuf::from(unescape(l.trim_matches(['"', '\''].as_slice()), escapes)))
            .collect()
    };
    // Whitespace between items: how a multi-file drop arrives from a terminal
    // that escapes the spaces inside each name.
    let split = || -> Vec<PathBuf> {
        split_items(text, escapes).iter().map(|s| PathBuf::from(unescape(s, escapes))).collect()
    };

    // Which to believe first depends on what a backslash means here.
    //
    // Where it escapes, a space between two items is a real separator and the
    // spaces *inside* a name arrive escaped — so splitting is right, and the
    // line-wise reading is the fallback for a terminal that did not escape.
    //
    // Where it does not — Windows — a space can only ever be part of a name.
    // `C:\Users\taro\My Documents\a.txt` split on whitespace becomes three
    // paths that are not anything, and the only reason it ever worked is that
    // none of the three happened to exist. Lines first there.
    let (first, second): (Vec<PathBuf>, Vec<PathBuf>) =
        if escapes { (split(), whole()) } else { (whole(), split()) };
    if exists_all(&first) {
        return first;
    }
    if exists_all(&second) {
        return second;
    }
    Vec::new()
}

/// Split a blob into candidate items: one per line, and — within a line —
/// on unescaped, unquoted whitespace, which is how a multi-file drop arrives.
fn split_items(text: &str, escapes: bool) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut cur = String::new();
        let mut quote: Option<char> = None;
        let mut escaped = false;
        for ch in line.chars() {
            if escaped {
                cur.push('\\');
                cur.push(ch);
                escaped = false;
                continue;
            }
            match ch {
                '\\' if escapes && quote.is_none() => escaped = true,
                '"' | '\'' if quote.is_none() => quote = Some(ch),
                c if Some(c) == quote => quote = None,
                c if c.is_whitespace() && quote.is_none() => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        // A trailing backslash is a literal one, not an escape of nothing.
        if escaped {
            cur.push('\\');
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

/// Turn one item into a plain path: strip a `file://` prefix (percent-decoded),
/// and drop the backslashes a shell-style escape added.
fn unescape(item: &str, escapes: bool) -> String {
    if let Some(rest) = item.strip_prefix("file://") {
        // `file:///path` — the third slash starts the path; a host (rare, and
        // not something cian can open) is dropped with it.
        let path = rest.find('/').map(|i| &rest[i..]).unwrap_or(rest);
        return percent_decode(path);
    }
    let mut out = String::with_capacity(item.len());
    let mut chars = item.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // A backslash escapes the next character — except on Windows,
            // where it is the path separator and escapes nothing.
            match chars.next() {
                Some(n) if escapes => out.push(n),
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Decode `%XX` escapes in a `file://` URI. Anything malformed is kept
/// verbatim, so a literal `%` in a name survives.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl crate::App {
    /// Take a paste that is really a drop: files dragged from Finder /
    /// Explorer / the desktop onto the terminal window. Returns true when it
    /// was handled, so an ordinary paste falls through untouched.
    ///
    /// A drop **moves** the files into the focused pane, which is what a drag
    /// between two folders means — and, like every other transfer in cian, it
    /// asks first, so the confirm dialog is where a mistaken drag is caught.
    pub(crate) fn accept_drop(&mut self, text: &str) -> bool {
        // Anything with a text field open is being typed into; a drop there is
        // someone filling in a path, and that is the paste they wanted.
        if !matches!(self.popup, crate::Popup::None) || self.mode != crate::Mode::Normal {
            return false;
        }
        // The shell gets its own pastes: dropping a file on a terminal to get
        // its path onto the command line is the oldest use of this gesture.
        if self.focused == crate::FocusedPane::Shell {
            return false;
        }
        // A synthetic listing has no directory to drop into.
        let Some(pane) = self.active_pane() else { return false };
        if pane.is_remote() || pane.archive_view().is_some() || pane.is_flat() {
            return false;
        }
        let dest = pane.cwd.clone();
        let paths = dropped_paths(text);
        if paths.is_empty() {
            return false;
        }
        // Dropping a folder onto itself, or a file onto the folder it is
        // already in, would be a no-op move; say so rather than opening a
        // dialog that does nothing.
        let already_here = paths.iter().all(|p| p.parent() == Some(dest.as_path()));
        if already_here {
            self.message = Some(crate::tr(
                self.lang,
                "those files are already here",
                "そのファイルは既にここにあります",
            )
            .into());
            return true;
        }
        self.popup = crate::transfer_popup(crate::PendingOp::Move, paths, dest);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each terminal's escaping, read back to the same real file.
    #[test]
    fn every_terminals_drop_format_finds_the_file() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("My File.txt");
        std::fs::write(&f, b"x").unwrap();
        let raw = f.display().to_string();

        let backslashed = raw.replace(' ', "\\ "); // iTerm2 / Terminal.app
        let quoted = format!("\"{}\"", raw); // Windows Terminal
        let uri = format!("file://{}", raw.replace(' ', "%20")); // GNOME / KDE

        for form in [raw.clone(), quoted, uri] {
            assert_eq!(dropped_paths(&form), vec![f.clone()], "form: {form}");
        }
        // The backslash form is a Unix convention; on Windows a backslash is
        // the separator and must survive.
        if !cfg!(windows) {
            assert_eq!(dropped_paths(&backslashed), vec![f.clone()]);
        }
    }

    /// A multi-file drop: several paths on one line, and one per line.
    #[test]
    fn several_files_arrive_together() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a.txt"), d.path().join("b b.txt"));
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let esc = |p: &PathBuf| {
            if cfg!(windows) {
                format!("\"{}\"", p.display())
            } else {
                p.display().to_string().replace(' ', "\\ ")
            }
        };
        let one_line = format!("{} {}", esc(&a), esc(&b));
        assert_eq!(dropped_paths(&one_line), vec![a.clone(), b.clone()]);
        let per_line = format!("{}\n{}\n", esc(&a), esc(&b));
        assert_eq!(dropped_paths(&per_line), vec![a.clone(), b.clone()]);
    }

    /// Ordinary pasted text must never be mistaken for a drop — including
    /// text that names one real file among words.
    #[test]
    fn prose_is_not_a_drop() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("real.txt");
        std::fs::write(&f, b"x").unwrap();

        assert!(dropped_paths("").is_empty());
        assert!(dropped_paths("just some words").is_empty());
        assert!(dropped_paths("SELECT * FROM t WHERE x = 1;").is_empty());
        assert!(
            dropped_paths(&format!("look at {} please", f.display())).is_empty(),
            "one real path among words is prose, not a drop"
        );
        assert!(
            dropped_paths(&format!("{}\n{}", f.display(), d.path().join("gone.txt").display()))
                .is_empty(),
            "all items must exist, or it is not a drop"
        );
    }
}
