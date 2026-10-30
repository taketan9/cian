//! Reading a file for display without leaving the file manager.
//!
//! The point of an F3 viewer is to answer "what is in here" in a second, so
//! this reads a bounded prefix rather than the whole file: opening a 4 GB log
//! to look at its first page should not cost 4 GB of memory or any noticeable
//! wait. Binary files are rendered as hex instead of as mojibake.
//!
//! The raw prefix is kept so the text can be re-decoded in another encoding on
//! demand — a Shift_JIS or UTF-16 file opened as UTF-8 is mojibake until the
//! viewer is told which encoding it really is.

use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};

/// How much of a file is read. Generous enough that ordinary files are shown
/// whole, small enough that a huge one is still instant.
pub const VIEW_LIMIT: u64 = 4 * 1024 * 1024;

/// How wide a tab is drawn, as a process-wide setting.
///
/// One number, so the viewer's rendering, the mouse's idea of which character
/// was clicked, the block selection and `:expand`'s default cannot disagree
/// about where a tab stop is.
///
/// Worth changing rather than assuming: a tab-separated file lines up only
/// when every field is narrower than the stop. With stops every four, `col1`
/// fills one exactly and its tab moves on to the next — so the column after
/// it starts at eight, while a two-column `あ` in the same place starts at
/// four. That is what tabs do, and eight is the width at which such a file
/// reads as a table.
static TAB_WIDTH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(4);

pub fn tab_width() -> usize {
    TAB_WIDTH.load(std::sync::atomic::Ordering::Relaxed).max(1)
}

/// Set from `cian.set_option("tab_width", n)` at startup and on reload.
pub fn set_tab_width(n: usize) {
    TAB_WIDTH.store(n.clamp(1, 16), std::sync::atomic::Ordering::Relaxed);
}

/// How much of the prefix is inspected when deciding text vs binary.
const SNIFF: usize = 8000;

/// A file's line ending, detected on read and written back unchanged.
///
/// `str::lines()` swallows the difference, so without carrying it explicitly
/// the viewer would silently rewrite every CRLF file as LF the first time it
/// was saved — a change nobody asked for, invisible until some Windows tool
/// downstream complained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,
    Crlf,
    /// Classic Mac. Rare, but cheap to keep once the other two are carried.
    Cr,
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::Crlf => "\r\n",
            Eol::Cr => "\r",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
            Eol::Cr => "CR",
        }
    }

    /// The dominant ending in `text`. Ties and empty files read as LF, which
    /// is the safer default on every platform cian runs on.
    pub fn detect(text: &str) -> Eol {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count() - crlf;
        let cr = text.matches('\r').count() - crlf;
        if crlf > lf && crlf >= cr {
            Eol::Crlf
        } else if cr > lf && cr > crlf {
            Eol::Cr
        } else {
            Eol::Lf
        }
    }
}

/// What the viewer is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    Text,
    /// Rendered as a hex dump because it is not text.
    Binary,
}

/// Text encodings the viewer can decode. Deliberately short — the ones that
/// actually turn up on a Japanese Windows machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    ShiftJis,
    Utf16Le,
    Utf16Be,
}

impl TextEncoding {
    /// All encodings, in the order a picker offers them.
    pub const ALL: [TextEncoding; 4] = [
        TextEncoding::Utf8,
        TextEncoding::ShiftJis,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::ShiftJis => "Shift_JIS",
            TextEncoding::Utf16Le => "UTF-16LE",
            TextEncoding::Utf16Be => "UTF-16BE",
        }
    }

    /// The next encoding in a cycle, for a "switch encoding" key.
    pub fn next(self) -> Self {
        match self {
            TextEncoding::Utf8 => TextEncoding::ShiftJis,
            TextEncoding::ShiftJis => TextEncoding::Utf16Le,
            TextEncoding::Utf16Le => TextEncoding::Utf16Be,
            TextEncoding::Utf16Be => TextEncoding::Utf8,
        }
    }

    fn engine(self) -> &'static encoding_rs::Encoding {
        match self {
            TextEncoding::Utf8 => encoding_rs::UTF_8,
            TextEncoding::ShiftJis => encoding_rs::SHIFT_JIS,
            TextEncoding::Utf16Le => encoding_rs::UTF_16LE,
            TextEncoding::Utf16Be => encoding_rs::UTF_16BE,
        }
    }

    /// Decode `bytes` in this encoding. Strips a leading BOM and never fails —
    /// invalid bytes become U+FFFD, which is what a viewer (or a terminal
    /// showing a mis-encoded stream) wants.
    pub fn decode(self, bytes: &[u8]) -> String {
        self.engine().decode(bytes).0.into_owned()
    }

    /// Encode `text` back to this encoding, for saving an edited file in the
    /// encoding it was read in. UTF-16 is not one of encoding_rs' output
    /// encodings, so those two are written by hand.
    /// The characters this encoding cannot hold, in the order they first
    /// appear. encoding_rs writes them as `&#128512;` and reports success, so
    /// a save that does not ask first leaves that text in the file (2026-10-30).
    pub fn cannot_hold(self, text: &str) -> Vec<char> {
        let mut out: Vec<char> = Vec::new();
        if self != TextEncoding::ShiftJis || !self.engine().encode(text).2 {
            return out;
        }
        let mut buf = [0u8; 4];
        for c in text.chars() {
            if !out.contains(&c) && self.engine().encode(c.encode_utf8(&mut buf)).2 {
                out.push(c);
            }
        }
        out
    }

    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            TextEncoding::Utf8 => text.as_bytes().to_vec(),
            TextEncoding::ShiftJis => self.engine().encode(text).0.into_owned(),
            TextEncoding::Utf16Le => text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect(),
            TextEncoding::Utf16Be => text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct View {
    pub kind: ViewKind,
    /// Display lines, already split and expanded, in the current encoding.
    pub lines: Vec<String>,
    /// Total size of the file, which may exceed what was read.
    pub total_bytes: u64,
    /// True when the file was longer than [`VIEW_LIMIT`].
    pub truncated: bool,
    /// The encoding `lines` were decoded with.
    pub encoding: TextEncoding,
    /// The file opened with a byte-order mark. Worth a badge: an invisible
    /// three bytes at the start of a script is a classic way to break it.
    pub bom: bool,
    /// The line ending the file arrived with, and the one a save writes back.
    pub eol: Eol,
    /// The raw prefix that was read, kept so [`View::redecode`] can rebuild
    /// `lines` in a different encoding.
    bytes: Vec<u8>,
}

impl View {
    /// Build a text view from already-extracted text (e.g. an Office/PDF
    /// document decoded by [`crate::office`]). The text itself becomes the kept
    /// bytes, so the encoding switch and everything else in the viewer behave
    /// exactly as for a real text file.
    pub fn from_text(text: String, total_bytes: u64, truncated: bool) -> View {
        let eol = Eol::detect(&text);
        let bytes = text.into_bytes();
        let lines = to_lines(&String::from_utf8_lossy(&bytes));
        View { kind: ViewKind::Text, lines, total_bytes, truncated, encoding: TextEncoding::Utf8, bom: false, eol, bytes }
    }

    /// Re-decode the kept bytes as `enc`, switching to text if it was showing
    /// hex (choosing an encoding is a deliberate "read this as text").
    pub fn redecode(&mut self, enc: TextEncoding) {
        self.encoding = enc;
        self.kind = ViewKind::Text;
        let decoded = enc.decode(&self.bytes);
        self.eol = Eol::detect(&decoded);
        self.lines = to_lines(&decoded);
    }
}

/// Split decoded text into lines, exactly as they are.
///
/// Tabs are deliberately left alone. Expanding them here — which this used to
/// do, so that columns lined up on screen — made every save write the file
/// back with its tabs spent, quietly turning a Makefile into one that does not
/// build. Drawing is the renderer's job; this is the buffer a save writes.
fn to_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

/// Guess the encoding from a byte-order mark, if present.
fn bom_encoding(bytes: &[u8]) -> Option<TextEncoding> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some(TextEncoding::Utf8)
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Some(TextEncoding::Utf16Le)
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Some(TextEncoding::Utf16Be)
    } else {
        None
    }
}

/// Read the beginning of `path` for display.
pub fn view_file(path: &Path) -> Result<View> {
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if meta.is_dir() {
        anyhow::bail!("{} is a directory", path.display());
    }
    let total_bytes = meta.len();
    let truncated = total_bytes > VIEW_LIMIT;

    let mut f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut buf = Vec::with_capacity(total_bytes.min(VIEW_LIMIT) as usize);
    f.by_ref()
        .take(VIEW_LIMIT)
        .read_to_end(&mut buf)
        .with_context(|| format!("read {}", path.display()))?;

    // A UTF-16 BOM means text even though the bytes are full of NULs; honour it
    // before the NUL sniff writes the file off as binary.
    if let Some(enc) = bom_encoding(&buf) {
        let decoded = enc.decode(&buf);
        let eol = Eol::detect(&decoded);
        let lines = to_lines(&decoded);
        return Ok(View { kind: ViewKind::Text, lines, total_bytes, truncated, encoding: enc, bom: true, eol, bytes: buf });
    }

    // A NUL in the first few KB is the usual signal; UTF-8 text does not
    // contain one, and every binary format seems to within its header.
    let binary = buf[..buf.len().min(SNIFF)].contains(&0);
    if binary {
        let lines = hex_dump(&buf);
        let eol = Eol::Lf; // a hex dump has no line endings of its own
        return Ok(View {
            kind: ViewKind::Binary,
            lines,
            total_bytes,
            truncated,
            encoding: TextEncoding::Utf8,
            bom: false, eol, bytes: buf,
        });
    }

    // UTF-8 first; when that trips on invalid sequences, try Shift_JIS — the
    // same fallback grep uses, because the logs this viewer meets (Oracle
    // alert logs, AIX batch output) are still written in it. Only a clean
    // SJIS decode wins; otherwise the U+FFFD-marked UTF-8 stands, which at
    // least shows *where* the bytes are broken.
    // **A file cut at the limit is judged on whole lines** (2026-10-30). The
    // cut can land inside a two-byte Shift_JIS character, the SJIS decode
    // then fails on that last byte alone, and a whole 4MB log fell back to
    // UTF-8 and showed as nothing but U+FFFD.
    if truncated {
        if let Some(end) = buf.iter().rposition(|&b| b == b'\n') {
            buf.truncate(end + 1);
        }
    }
    let (utf8, _, utf8_errors) = encoding_rs::UTF_8.decode(&buf);
    let (text, encoding) = if utf8_errors {
        let (sjis, _, sjis_errors) = encoding_rs::SHIFT_JIS.decode(&buf);
        if sjis_errors {
            (utf8.into_owned(), TextEncoding::Utf8)
        } else {
            (sjis.into_owned(), TextEncoding::ShiftJis)
        }
    } else {
        (utf8.into_owned(), TextEncoding::Utf8)
    };
    let eol = Eol::detect(&text);
    let lines = to_lines(&text);
    Ok(View { kind: ViewKind::Text, lines, total_bytes, truncated, encoding, bom: false, eol, bytes: buf })
}

impl View {
    /// The raw bytes backing the view (the read prefix of the file).
    pub fn raw_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Overwrite one byte and regenerate just its hex-dump line. The unit of
    /// the hex editor: overwrite-only, so offsets never shift and the file
    /// size cannot change out from under anyone.
    pub fn hex_set_byte(&mut self, idx: usize, val: u8) {
        if idx >= self.bytes.len() || self.kind != ViewKind::Binary {
            return;
        }
        self.bytes[idx] = val;
        let line = idx / 16;
        let start = line * 16;
        let chunk = &self.bytes[start..(start + 16).min(self.bytes.len())];
        if line < self.lines.len() {
            self.lines[line] = hex_dump_line(line, chunk);
        }
    }

    /// Replace the whole buffer (the hex editor's undo) and re-render.
    pub fn set_raw_bytes(&mut self, bytes: Vec<u8>) {
        self.bytes = bytes;
        if self.kind == ViewKind::Binary {
            self.lines = hex_dump(&self.bytes);
        }
    }
}

/// One `offset  hex bytes  |ascii|` line of the dump (16 bytes).
fn hex_dump_line(index: usize, chunk: &[u8]) -> String {
    let mut hex = String::with_capacity(48);
    for (j, b) in chunk.iter().enumerate() {
        if j == 8 {
            hex.push(' ');
        }
        hex.push_str(&format!("{:02x} ", b));
    }
    let ascii: String = chunk
        .iter()
        .map(|b| if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '.' })
        .collect();
    format!("{:08x}  {:<49}|{}|", index * 16, hex, ascii)
}

/// `offset  hex bytes  |ascii|`, sixteen bytes to a line.
fn hex_dump(bytes: &[u8]) -> Vec<String> {
    bytes.chunks(16).enumerate().map(|(i, chunk)| hex_dump_line(i, chunk)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The buffer is what a save writes back, so it holds the file's own
    /// characters — tabs included. Drawing them four columns wide is the
    /// renderer's business; doing it here spent every tab on the first save.
    #[test]
    fn text_is_split_into_lines_with_its_tabs_intact() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        fs::write(&f, "one\n\ttwo\nthree\n").unwrap();
        let v = view_file(&f).unwrap();
        assert_eq!(v.kind, ViewKind::Text);
        assert_eq!(v.lines, vec!["one", "\ttwo", "three"]);
        assert!(!v.truncated);
        assert_eq!(v.encoding, TextEncoding::Utf8);
    }

    /// Showing a compiled file as text produces a screen of mojibake that
    /// answers nothing; hex at least shows the header.
    #[test]
    fn binary_becomes_a_hex_dump() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.bin");
        fs::write(&f, b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00").unwrap();
        let v = view_file(&f).unwrap();
        assert_eq!(v.kind, ViewKind::Binary);
        assert!(v.lines[0].starts_with("00000000  7f 45 4c 46"), "got {:?}", v.lines[0]);
        assert!(v.lines[0].contains("|.ELF"), "ascii column missing: {:?}", v.lines[0]);
    }

    #[test]
    fn a_huge_file_is_read_only_up_to_the_limit() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("big.log");
        let line = "x".repeat(999);
        let mut text = String::new();
        while (text.len() as u64) < VIEW_LIMIT + 4096 {
            text.push_str(&line);
            text.push('\n');
        }
        fs::write(&f, &text).unwrap();

        let v = view_file(&f).unwrap();
        assert!(v.truncated, "should say it stopped early");
        assert!(v.total_bytes > VIEW_LIMIT);
        let read: usize = v.lines.iter().map(|l| l.len() + 1).sum();
        assert!(read as u64 <= VIEW_LIMIT + 1024, "read {} bytes", read);
    }

    #[test]
    fn invalid_utf8_is_shown_rather_than_refused() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("latin.txt");
        // Not valid UTF-8, but no NUL either: still meant to be readable.
        fs::write(&f, b"caf\xe9 au lait\n").unwrap();
        let v = view_file(&f).unwrap();
        assert_eq!(v.kind, ViewKind::Text);
        assert!(v.lines[0].contains("caf"), "got {:?}", v.lines);
    }

    #[test]
    fn an_empty_file_views_as_nothing_rather_than_failing() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("empty");
        fs::write(&f, b"").unwrap();
        let v = view_file(&f).unwrap();
        assert!(v.lines.is_empty());
        assert_eq!(v.total_bytes, 0);
    }

    #[test]
    fn a_directory_is_refused() {
        let d = tempfile::tempdir().unwrap();
        assert!(view_file(d.path()).is_err());
    }

    /// A Shift_JIS file opens readable straight away: UTF-8 trips on its
    /// bytes, so the automatic fallback decodes it — no trip through the
    /// encoding picker. The picker still works for overriding.
    #[test]
    fn shift_jis_auto_decodes_on_open() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("sjis.txt");
        // "日本語" in Shift_JIS.
        let sjis = [0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea, b'\n'];
        fs::write(&f, sjis).unwrap();
        let mut v = view_file(&f).unwrap();
        assert_eq!(v.kind, ViewKind::Text);
        assert_eq!(v.lines[0], "日本語", "auto-detected as Shift_JIS");
        assert_eq!(v.encoding, TextEncoding::ShiftJis, "so saving writes SJIS back");
        // The picker can still force another reading of the same bytes.
        v.redecode(TextEncoding::Utf8);
        assert_ne!(v.lines[0], "日本語", "an override still overrides");
    }

    #[test]
    fn encode_round_trips_each_encoding() {
        // What the in-viewer editor relies on to save in the file's own encoding.
        let text = "日本語 abc\n";
        for enc in TextEncoding::ALL {
            let bytes = enc.encode(text);
            assert_eq!(enc.decode(&bytes), text, "{:?} round-trips", enc);
        }
        // Shift_JIS and UTF-16 really change the bytes (not just UTF-8).
        assert_ne!(TextEncoding::ShiftJis.encode(text), text.as_bytes());
        assert_ne!(TextEncoding::Utf16Le.encode(text), text.as_bytes());
    }

    /// A UTF-16LE file has NULs, so it is hex by default; choosing UTF-16
    /// switches it to readable text.
    #[test]
    fn utf16_without_bom_is_hex_until_chosen() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("u16.txt");
        // "Hi" in UTF-16LE, no BOM.
        fs::write(&f, [0x48, 0x00, 0x69, 0x00]).unwrap();
        let mut v = view_file(&f).unwrap();
        assert_eq!(v.kind, ViewKind::Binary, "NULs make it look binary");
        v.redecode(TextEncoding::Utf16Le);
        assert_eq!(v.kind, ViewKind::Text);
        assert_eq!(v.lines[0], "Hi");
    }

    /// A UTF-16LE BOM is recognised as text straight away.
    #[test]
    fn a_utf16_bom_is_read_as_text() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("bom.txt");
        fs::write(&f, [0xFF, 0xFE, 0x48, 0x00, 0x69, 0x00]).unwrap();
        let v = view_file(&f).unwrap();
        assert_eq!(v.kind, ViewKind::Text);
        assert_eq!(v.encoding, TextEncoding::Utf16Le);
        assert_eq!(v.lines[0], "Hi");
    }
}

#[cfg(test)]
mod cut_at_the_limit {
    use super::*;

    #[test]
    fn a_large_shift_jis_log_is_still_shift_jis() {
        let d = tempfile::tempdir().unwrap();
        for shift in 0..2usize {
            let p = d.path().join(format!("big{shift}.log"));
            let mut bytes = vec![b'x'; shift];
            let line = encoding_rs::SHIFT_JIS.encode("障害が発生しました\n").0.into_owned();
            while (bytes.len() as u64) <= VIEW_LIMIT + 100 {
                bytes.extend_from_slice(&line);
            }
            std::fs::write(&p, &bytes).unwrap();
            let v = view_file(&p).unwrap();
            assert!(v.truncated);
            assert_eq!(v.encoding, TextEncoding::ShiftJis, "shift {shift}");
            assert!(v.lines[1].contains("障害"), "{:?}", v.lines[1]);
        }
    }
}
