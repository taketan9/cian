//! The small "tell me about this" commands: `df`, `wc`, `file`, `head`, `tail`.
//!
//! Each answers in one line or one popup what you would otherwise drop to a
//! shell for. They read boundedly and never block on a whole huge file: the
//! point is a quick answer, not a faithful reimplementation of the coreutils.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};

/// Free and total space on the filesystem that holds a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskSpace {
    pub total: u64,
    /// Space actually available to this user (excludes root-reserved blocks).
    pub available: u64,
}

impl DiskSpace {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    /// Percentage used, as `df` prints it, rounding up so a nearly-full disk
    /// never reads as a reassuring 99%.
    pub fn percent_used(&self) -> u64 {
        if self.total == 0 {
            return 0;
        }
        (self.used() as u128 * 100).div_ceil(self.total as u128) as u64
    }
}

/// How `df` should print its sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// `-h`: the largest unit that keeps the number small.
    Human,
    /// `-k`, `-m`, `-g`: fixed unit.
    Kib,
    Mib,
    Gib,
    /// Raw bytes.
    Bytes,
}

impl Unit {
    /// Parse the flag `df` was given. `-h` is the default when bare.
    pub fn parse(flag: &str) -> Option<Self> {
        match flag.trim().trim_start_matches('-') {
            "h" | "" => Some(Unit::Human),
            "k" => Some(Unit::Kib),
            "m" => Some(Unit::Mib),
            "g" => Some(Unit::Gib),
            "b" => Some(Unit::Bytes),
            _ => None,
        }
    }

    pub fn format(self, bytes: u64) -> String {
        match self {
            Unit::Human => crate::human_size(bytes),
            Unit::Kib => format!("{}K", bytes / 1024),
            Unit::Mib => format!("{}M", bytes / (1024 * 1024)),
            Unit::Gib => format!("{}G", bytes / (1024 * 1024 * 1024)),
            Unit::Bytes => bytes.to_string(),
        }
    }
}

/// Space on the filesystem holding `path`.
pub fn disk_space(path: &Path) -> Result<DiskSpace> {
    let total = fs4::total_space(path).with_context(|| format!("df {}", path.display()))?;
    let available =
        fs4::available_space(path).with_context(|| format!("df {}", path.display()))?;
    Ok(DiskSpace { total, available })
}

/// Line, word and byte counts, as `wc` reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub lines: usize,
    pub words: usize,
    pub bytes: u64,
}

/// Count lines, words and bytes in a file.
///
/// "Lines" is the number of newlines, matching `wc -l` — a file whose last
/// line has no trailing newline counts one fewer than it visually has, which
/// is exactly what `wc` does and what a script comparing to it expects.
pub fn count(path: &Path) -> Result<Counts> {
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if meta.is_dir() {
        anyhow::bail!("{} is a directory", path.display());
    }
    let bytes = meta.len();
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let lines = data.iter().filter(|&&b| b == b'\n').count();
    // Words are runs of non-whitespace, counted over the text as UTF-8 with
    // invalid bytes replaced — a binary file gives a meaningless but harmless
    // number rather than an error.
    let words = String::from_utf8_lossy(&data).split_whitespace().count();
    Ok(Counts { lines, words, bytes })
}

/// A short human description of what a file is, in the spirit of `file(1)`.
pub fn classify(path: &Path) -> Result<String> {
    let meta =
        fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(path).ok();
        return Ok(match target {
            Some(t) => format!("symbolic link to {}", t.display()),
            None => "symbolic link".to_string(),
        });
    }
    if meta.is_dir() {
        return Ok("directory".to_string());
    }
    if meta.len() == 0 {
        return Ok("empty".to_string());
    }

    // Sniff a prefix: enough to see a magic number and to judge text vs binary.
    let mut f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut head = [0u8; 512];
    let n = f.read(&mut head).with_context(|| format!("read {}", path.display()))?;
    let head = &head[..n];

    if let Some(magic) = magic(head) {
        return Ok(magic.to_string());
    }
    if head.contains(&0) {
        return Ok("binary data".to_string());
    }
    // No NUL and valid-ish UTF-8: call it text, and note the executable bit on
    // Unix since that is the distinction that matters for a script.
    let ascii = std::str::from_utf8(head).is_ok();
    let kind = if head.starts_with(b"#!") {
        "script text"
    } else if ascii {
        "text"
    } else {
        "text (non-UTF-8)"
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return Ok(format!("{}, executable", kind));
        }
    }
    Ok(kind.to_string())
}

/// Recognise a handful of common formats by their leading bytes.
fn magic(head: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| head.starts_with(sig);
    Some(match head {
        _ if starts(b"\x7fELF") => "ELF executable",
        _ if starts(b"MZ") => "PE/DOS executable",
        _ if starts(b"\x89PNG\r\n\x1a\n") => "PNG image",
        _ if starts(b"\xff\xd8\xff") => "JPEG image",
        _ if starts(b"GIF87a") || starts(b"GIF89a") => "GIF image",
        _ if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") => "zip archive",
        _ if starts(b"%PDF-") => "PDF document",
        _ if starts(b"\x1f\x8b") => "gzip compressed data",
        _ if starts(b"BZh") => "bzip2 compressed data",
        _ if starts(b"\xfd7zXZ\x00") => "xz compressed data",
        _ if starts(b"7z\xbc\xaf\x27\x1c") => "7-zip archive",
        _ if starts(b"Rar!\x1a\x07") => "RAR archive",
        _ if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WAVE" => "WAV audio",
        _ if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"AVI " => "AVI video",
        _ if head.len() >= 12 && &head[4..8] == b"ftyp" => "MP4/MOV media",
        _ if starts(b"ID3") || starts(b"\xff\xfb") => "MP3 audio",
        _ if starts(b"\xca\xfe\xba\xbe") => "Java class",
        _ => return None,
    })
}

/// Which end of a file to take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Head,
    Tail,
}

/// The most any `head`/`tail` will read from the tail end, so the operation
/// stays instant on a multi-gigabyte log. Generous enough for any sane `-n`.
const TAIL_WINDOW: u64 = 1024 * 1024;

/// The first or last `n` lines of a file.
///
/// `tail` seeks near the end rather than reading the whole file, so it is cheap
/// on a huge log; if the last `n` lines do not fit in the window it returns as
/// many as do, with a note.
pub fn peek(path: &Path, end: End, n: usize) -> Result<Vec<String>> {
    // **Refuses anything that is not an ordinary file.** Reading only the
    // first lines is no protection: `File::open` on a named pipe waits for a
    // writer before a single byte is asked for, and the tail end reads to the
    // end of something that has none. The callers say so in the reader's own
    // language before getting here; this is the backstop (2026-11-28, 所見
    // S12), the same one `hash_file` carries.
    if let Some(what) = crate::attrs::special(path) {
        anyhow::bail!("not an ordinary file ({what:?}): {}", path.display());
    }
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if meta.is_dir() {
        anyhow::bail!("{} is a directory", path.display());
    }
    match end {
        End::Head => {
            let mut f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
            // A bounded read is enough for the first n lines of anything.
            let mut buf = vec![0u8; TAIL_WINDOW as usize];
            let read = f.read(&mut buf).with_context(|| format!("read {}", path.display()))?;
            let text = String::from_utf8_lossy(&buf[..read]);
            Ok(text.lines().take(n).map(|l| l.to_string()).collect())
        }
        End::Tail => {
            let len = meta.len();
            let from = len.saturating_sub(TAIL_WINDOW);
            let mut f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
            f.seek(SeekFrom::Start(from)).with_context(|| format!("seek {}", path.display()))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).with_context(|| format!("read {}", path.display()))?;
            let text = String::from_utf8_lossy(&buf);
            let mut lines: Vec<&str> = text.lines().collect();
            // If the window started mid-file, its first (partial) line is not a
            // real line boundary and would be a fragment; drop it.
            if from > 0 && lines.len() > 1 {
                lines.remove(0);
            }
            let start = lines.len().saturating_sub(n);
            Ok(lines[start..].iter().map(|l| l.to_string()).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_space_reports_a_plausible_total() {
        let d = tempfile::tempdir().unwrap();
        let s = disk_space(d.path()).unwrap();
        assert!(s.total > 0);
        assert!(s.available <= s.total);
        assert!(s.percent_used() <= 100);
    }

    #[test]
    fn units_parse_and_format() {
        assert_eq!(Unit::parse("-h"), Some(Unit::Human));
        assert_eq!(Unit::parse("-m"), Some(Unit::Mib));
        assert_eq!(Unit::parse("g"), Some(Unit::Gib));
        assert_eq!(Unit::parse(""), Some(Unit::Human));
        assert_eq!(Unit::parse("-x"), None);
        assert_eq!(Unit::Mib.format(3 * 1024 * 1024), "3M");
        assert_eq!(Unit::Kib.format(2048), "2K");
        assert_eq!(Unit::Bytes.format(500), "500");
    }

    /// A full disk must not round down to a comfortable-looking number.
    #[test]
    fn percent_used_rounds_up() {
        let s = DiskSpace { total: 1000, available: 1 };
        assert_eq!(s.used(), 999);
        assert_eq!(s.percent_used(), 100);
        let s = DiskSpace { total: 1000, available: 995 };
        assert_eq!(s.percent_used(), 1, "5/1000 rounds up to 1, not down to 0");
    }

    #[test]
    fn wc_matches_the_coreutil_convention() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        // Three newlines, so wc -l says 3 even though there are 4 visual lines.
        fs::write(&f, "one two\nthree\n\nfour and five\n").unwrap();
        let c = count(&f).unwrap();
        assert_eq!(c.lines, 4);
        assert_eq!(c.words, 6);
        assert_eq!(c.bytes, "one two\nthree\n\nfour and five\n".len() as u64);
    }

    #[test]
    fn wc_on_a_file_without_a_final_newline() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        fs::write(&f, "no newline here").unwrap();
        let c = count(&f).unwrap();
        assert_eq!(c.lines, 0, "wc -l counts newlines, and there are none");
        assert_eq!(c.words, 3);
    }

    #[test]
    fn classify_names_common_things() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("sub");
        fs::create_dir(&dir).unwrap();
        assert_eq!(classify(&dir).unwrap(), "directory");

        let empty = d.path().join("empty");
        fs::write(&empty, b"").unwrap();
        assert_eq!(classify(&empty).unwrap(), "empty");

        let png = d.path().join("x.png");
        fs::write(&png, b"\x89PNG\r\n\x1a\nrest of the file").unwrap();
        assert_eq!(classify(&png).unwrap(), "PNG image");

        let zip = d.path().join("x.zip");
        fs::write(&zip, b"PK\x03\x04and more").unwrap();
        assert_eq!(classify(&zip).unwrap(), "zip archive");

        let txt = d.path().join("x.txt");
        fs::write(&txt, b"just some words\n").unwrap();
        assert_eq!(classify(&txt).unwrap(), "text");

        let sh = d.path().join("run");
        fs::write(&sh, b"#!/bin/sh\necho hi\n").unwrap();
        assert!(classify(&sh).unwrap().starts_with("script text"));

        let bin = d.path().join("x.bin");
        fs::write(&bin, b"text\x00\x01\x02more").unwrap();
        assert_eq!(classify(&bin).unwrap(), "binary data");
    }

    #[cfg(unix)]
    #[test]
    fn classify_notes_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("prog");
        fs::write(&f, b"plain text content\n").unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(classify(&f).unwrap(), "text, executable");
    }

    #[test]
    fn head_and_tail_take_the_right_ends() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        let text: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
        fs::write(&f, &text).unwrap();

        let h = peek(&f, End::Head, 3).unwrap();
        assert_eq!(h, vec!["line 1", "line 2", "line 3"]);

        let t = peek(&f, End::Tail, 3).unwrap();
        assert_eq!(t, vec!["line 98", "line 99", "line 100"]);
    }

    #[test]
    fn asking_for_more_lines_than_exist_returns_them_all() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        fs::write(&f, "only\ntwo\n").unwrap();
        assert_eq!(peek(&f, End::Head, 10).unwrap(), vec!["only", "two"]);
        assert_eq!(peek(&f, End::Tail, 10).unwrap(), vec!["only", "two"]);
    }

    /// Tail of a file bigger than the seek window must still return real
    /// last lines, not a fragment of one.
    #[test]
    fn tail_of_a_large_file_reads_only_the_end() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("big.log");
        let line = "x".repeat(200);
        let mut text = String::new();
        let mut i = 0;
        while (text.len() as u64) < TAIL_WINDOW + 4096 {
            text.push_str(&format!("{} {}\n", i, line));
            i += 1;
        }
        fs::write(&f, &text).unwrap();
        let last = format!("{} {}", i - 1, line);
        let t = peek(&f, End::Tail, 2).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[1], last, "the actual final line");
        // And no leading fragment sneaks in.
        assert!(t[0].contains(&line), "a whole line, not a fragment");
    }
}

#[cfg(test)]
mod peek_refuses {
    use super::*;

    /// The failure this pins: `head` on a named pipe never came back. It is
    /// not the reading that blocks — it is `File::open`, before a byte has
    /// been asked for — so "only the first few lines" was no protection at
    /// all (2026-11-28, 所見 S12). The test would hang rather than fail if
    /// the guard went, which is the honest shape for this one.
    #[test]
    fn peeking_refuses_what_never_opens() {
        if cfg!(windows) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cian-peek-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if !matches!(made, Ok(s) if s.success()) {
            return;
        }
        for end in [End::Head, End::Tail] {
            let e = peek(&fifo, end, 5).unwrap_err().to_string();
            assert!(e.contains("Fifo"), "it says what it is: {e}");
        }
        assert!(peek(Path::new("/dev/zero"), End::Head, 5).is_err());
        fs::remove_dir_all(&dir).ok();
    }
}
