//! File attributes and checksums.
//!
//! Both are things you go to a file manager for and then leave it to run a
//! command: "what are the permissions on this", "does this match the hash they
//! sent me". Hashing streams and takes a cancel flag, because the files worth
//! checksumming are usually the large ones.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

/// Which digest to compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashKind {
    Md5,
    Sha256,
}

impl HashKind {
    pub fn label(self) -> &'static str {
        match self {
            HashKind::Md5 => "MD5",
            HashKind::Sha256 => "SHA-256",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().replace('-', "").as_str() {
            "md5" => Some(HashKind::Md5),
            "sha256" | "sha2" | "" => Some(HashKind::Sha256),
            _ => None,
        }
    }
}

/// An incremental digest: feed it bytes as they arrive, then `finish` for the
/// hex string. Lets a hash be computed over a stream that never lands in one
/// file — a remote file read back over SFTP to verify a transfer, say — so the
/// hashing lives in one place instead of being reimplemented per source.
pub struct Hasher {
    kind: HashKind,
    md5: md5::Md5,
    sha: sha2::Sha256,
}

impl Hasher {
    pub fn new(kind: HashKind) -> Self {
        use md5::Digest as _;
        Self { kind, md5: md5::Md5::new(), sha: sha2::Sha256::new() }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        use md5::Digest as _;
        match self.kind {
            HashKind::Md5 => self.md5.update(bytes),
            HashKind::Sha256 => self.sha.update(bytes),
        }
    }

    pub fn finish(self) -> String {
        use md5::Digest as _;
        // Spelled out rather than `{:x}`: RustCrypto 0.11 stopped
        // implementing `LowerHex` on the digest it hands back.
        let hex = |bytes: &[u8]| -> String {
            let mut out = String::with_capacity(bytes.len() * 2);
            for b in bytes {
                use std::fmt::Write as _;
                let _ = write!(out, "{b:02x}");
            }
            out
        };
        match self.kind {
            HashKind::Md5 => hex(&self.md5.finalize()),
            HashKind::Sha256 => hex(&self.sha.finalize()),
        }
    }
}

/// What a path is, when it is not something cian can read to the end.
///
/// **It is not "missing".** A named pipe and `/dev/zero` both exist; cian's
/// own refusal said they did not, which sent the reader looking for a file
/// that was right there (2026-11-28, findings S5 and S6). The difference
/// matters more than it looks: one of these is a typo, the others are a
/// program that never returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    /// Nothing at this path.
    Missing,
    /// A directory. Real, but not a file to read.
    Directory,
    /// A named pipe. Reading waits for a writer that may never come.
    Fifo,
    /// A device. `/dev/zero` answers for ever, and the read never ends.
    Device,
    /// A socket.
    Socket,
    /// Something else that is not a regular file.
    Other,
}

/// `None` when the path is an ordinary file that can be read to the end.
///
/// Symbolic links are followed: a link to `/dev/zero` is a device, and
/// treating it as a file is how the harness managed to allocate memory
/// without limit.
pub fn special(path: &Path) -> Option<Special> {
    let md = match fs::metadata(path) {
        Ok(md) => md,
        Err(_) => return Some(Special::Missing),
    };
    if md.is_file() {
        return None;
    }
    if md.is_dir() {
        return Some(Special::Directory);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let t = md.file_type();
        if t.is_fifo() {
            return Some(Special::Fifo);
        }
        if t.is_char_device() || t.is_block_device() {
            return Some(Special::Device);
        }
        if t.is_socket() {
            return Some(Special::Socket);
        }
    }
    Some(Special::Other)
}

/// Hash a file, reading it in chunks so a large one neither loads into memory
/// nor delays a cancel until it finishes.
///
/// Returns `Ok(None)` if it was cancelled.
///
/// **Refuses anything that is not an ordinary file.** Reading a named pipe
/// waits for a writer; reading `/dev/zero` never ends, and the request thread
/// that called this is the one the whole window waits on. The callers say so
/// in the reader's own language before getting here; this is the backstop, so
/// no future caller can hang the program by forgetting.
pub fn hash_file(path: &Path, kind: HashKind, cancel: &AtomicBool) -> Result<Option<String>> {
    if let Some(what) = special(path) {
        anyhow::bail!("not an ordinary file ({what:?}): {}", path.display());
    }
    let mut f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut buf = vec![0u8; 1024 * 1024];
    let mut hasher = Hasher::new(kind);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let n = f.read(&mut buf).with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Some(hasher.finish()))
}

/// A file's permissions in the form the platform actually uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attrs {
    /// Unix mode bits, if this is a Unix filesystem.
    pub mode: Option<u32>,
    /// Whether the file is read-only. Meaningful everywhere.
    pub readonly: bool,
    /// Owner, as a name where one can be resolved.
    pub owner: Option<String>,
    /// Size in bytes. `None` for directories, where a byte count is not what
    /// anyone means by "size".
    pub size: Option<u64>,
    /// True for a directory, so callers can label it rather than show a size.
    pub is_dir: bool,
}

impl Attrs {
    /// `rwxr-xr-x`-style rendering of the mode, or a plain read-only flag
    /// where there are no mode bits to show.
    pub fn describe(&self) -> String {
        match self.mode {
            Some(m) => {
                let bit = |i: u32, c: char| if m & (1 << i) != 0 { c } else { '-' };
                format!(
                    "{}{}{}{}{}{}{}{}{}  ({:04o})",
                    bit(8, 'r'), bit(7, 'w'), bit(6, 'x'),
                    bit(5, 'r'), bit(4, 'w'), bit(3, 'x'),
                    bit(2, 'r'), bit(1, 'w'), bit(0, 'x'),
                    m & 0o7777
                )
            }
            None => {
                if self.readonly {
                    "read-only".to_string()
                } else {
                    "read/write".to_string()
                }
            }
        }
    }
}

pub fn read_attrs(path: &Path) -> Result<Attrs> {
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let readonly = meta.permissions().readonly();
    let is_dir = meta.is_dir();
    // A directory's byte length is a filesystem detail, not the "size" a user
    // means; leave it out and let the caller label it as a folder.
    let size = if is_dir { None } else { Some(meta.len()) };

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        // Resolving a uid to a name needs the passwd database; showing the
        // number is better than pretending to know.
        let owner = Some(meta.uid().to_string());
        Ok(Attrs { mode: Some(mode), readonly, owner, size, is_dir })
    }
    #[cfg(not(unix))]
    {
        Ok(Attrs { mode: None, readonly, owner: None, size, is_dir })
    }
}

/// Apply a `chmod`-style mode. Octal only: symbolic forms like `u+x` are a
/// small language of their own and guessing at them would be worse than
/// refusing.
pub fn set_mode(path: &Path, spec: &str) -> Result<()> {
    let spec = spec.trim();
    let mode = u32::from_str_radix(spec, 8)
        .with_context(|| format!("not an octal mode: {:?} (try 644 or 755)", spec))?;
    if mode > 0o7777 {
        anyhow::bail!("mode out of range: {:o}", mode);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("chmod {} {}", spec, path.display()))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // Windows has no mode bits; the closest thing is the read-only flag,
        // and silently reinterpreting 644 as "read-only" would be a lie.
        let _ = (mode, path);
        anyhow::bail!("Windows has no mode bits — use `:readonly on|off` instead")
    }
}

/// Set or clear the read-only flag. The one attribute that means something on
/// every platform cian runs on.
pub fn set_readonly(path: &Path, readonly: bool) -> Result<()> {
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let mut perms = meta.permissions();
    perms.set_readonly(readonly);
    fs::set_permissions(path, perms)
        .with_context(|| format!("set read-only on {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_match_known_values() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x");
        fs::write(&f, b"abc").unwrap();
        let cancel = AtomicBool::new(false);

        assert_eq!(
            hash_file(&f, HashKind::Md5, &cancel).unwrap().unwrap(),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(
            hash_file(&f, HashKind::Sha256, &cancel).unwrap().unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn incremental_hasher_matches_hash_file_across_chunk_boundaries() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("data");
        // Content longer than the streaming buffer, fed to the Hasher in awkward
        // slices, must still equal the whole-file hash — this is exactly what
        // transfer verification relies on when it hashes a remote read.
        let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&f, &bytes).unwrap();
        let cancel = AtomicBool::new(false);

        for kind in [HashKind::Md5, HashKind::Sha256] {
            let whole = hash_file(&f, kind, &cancel).unwrap().unwrap();
            let mut h = Hasher::new(kind);
            for chunk in bytes.chunks(7777) {
                h.update(chunk);
            }
            assert_eq!(h.finish(), whole, "{:?} streamed != whole-file", kind);
        }
    }

    /// Files worth checksumming are the big ones, so the read has to be
    /// interruptible rather than run to the end regardless.
    #[test]
    fn hashing_can_be_cancelled() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("big");
        fs::write(&f, vec![0u8; 4 * 1024 * 1024]).unwrap();
        let cancel = AtomicBool::new(true);
        assert!(hash_file(&f, HashKind::Sha256, &cancel).unwrap().is_none());
    }

    /// **「ありません」と言っていたものは、全部そこに在った。**
    ///
    /// 名前付きパイプと `/dev/zero` を「ありません」と断っていたので、読む人は
    /// 在るファイルを探しにいくことになる（2026-11-28、所見 S5・S6）。
    #[test]
    fn what_is_not_an_ordinary_file_says_what_it_is() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("plain");
        fs::write(&f, b"x").unwrap();
        assert_eq!(special(&f), None, "読める");
        assert_eq!(special(d.path()), Some(Special::Directory));
        assert_eq!(special(&d.path().join("nope")), Some(Special::Missing));
        #[cfg(unix)]
        {
            assert_eq!(special(Path::new("/dev/null")), Some(Special::Device));
            let fifo = d.path().join("pipe");
            let made = std::process::Command::new("mkfifo").arg(&fifo).status();
            if made.map(|s| s.success()).unwrap_or(false) {
                assert_eq!(special(&fifo), Some(Special::Fifo));
            }
        }
    }

    /// **読み始めたら返らないものは、読み始めない。**
    ///
    /// エンジンは要求を1本ずつ処理するので、ここで返らないことは窓が二度と
    /// 描かれないことと同じ。呼ぶ側も断るが、これは最後の砦。
    #[cfg(unix)]
    #[test]
    fn hashing_refuses_what_never_ends() {
        let d = tempfile::tempdir().unwrap();
        let fifo = d.path().join("pipe");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if !made.map(|s| s.success()).unwrap_or(false) {
            return; // mkfifo が無い環境では測れない
        }
        let cancel = AtomicBool::new(false);
        // 返ること自体が答え。固まるなら、ここでテストが終わらない。
        assert!(hash_file(&fifo, HashKind::Sha256, &cancel).is_err());
        assert!(hash_file(Path::new("/dev/zero"), HashKind::Sha256, &cancel).is_err());
    }

    #[test]
    fn hash_kind_parses_the_spellings_people_type() {
        assert_eq!(HashKind::parse("md5"), Some(HashKind::Md5));
        assert_eq!(HashKind::parse("MD-5"), Some(HashKind::Md5));
        assert_eq!(HashKind::parse("sha256"), Some(HashKind::Sha256));
        assert_eq!(HashKind::parse("SHA-256"), Some(HashKind::Sha256));
        // Bare `:hash` means the sensible default.
        assert_eq!(HashKind::parse(""), Some(HashKind::Sha256));
        assert_eq!(HashKind::parse("crc32"), None);
    }

    #[test]
    fn reading_attributes_reports_something_useful_on_every_platform() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x");
        fs::write(&f, b"hi").unwrap();
        let a = read_attrs(&f).unwrap();
        assert!(!a.readonly);
        let shown = a.describe();
        assert!(!shown.is_empty());
        #[cfg(unix)]
        assert!(shown.contains('r'), "expected an rwx rendering, got {:?}", shown);
    }

    #[test]
    fn read_only_can_be_set_and_cleared() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x");
        fs::write(&f, b"hi").unwrap();

        set_readonly(&f, true).unwrap();
        assert!(read_attrs(&f).unwrap().readonly);
        set_readonly(&f, false).unwrap();
        assert!(!read_attrs(&f).unwrap().readonly);
    }

    #[cfg(unix)]
    #[test]
    fn a_mode_is_applied_and_a_bad_one_refused() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x");
        fs::write(&f, b"hi").unwrap();

        set_mode(&f, "600").unwrap();
        assert_eq!(read_attrs(&f).unwrap().mode.unwrap() & 0o777, 0o600);
        assert_eq!(read_attrs(&f).unwrap().describe().split_whitespace().next().unwrap(), "rw-------");

        // Symbolic forms are refused rather than guessed at.
        assert!(set_mode(&f, "u+x").is_err());
        assert!(set_mode(&f, "999").is_err(), "9 is not an octal digit");
        // Unchanged after the failures.
        assert_eq!(read_attrs(&f).unwrap().mode.unwrap() & 0o777, 0o600);
    }
}
