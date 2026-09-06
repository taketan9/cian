//! Filesystem operations used by the file panes.
//!
//! Every routine here is non-interactive: it succeeds, fails, or returns a
//! conflict so the UI layer can decide how to react (overwrite / skip / etc).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs_extra::dir::{self, CopyOptions as DirCopyOptions};
use fs_extra::file::{self, CopyOptions as FileCopyOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// Skip a single destination if it already exists.
    Skip,
    /// Overwrite the destination unconditionally.
    Overwrite,
}

#[derive(Debug, Default, Clone)]
pub struct OpReport {
    pub ok: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    /// Optional trailing note for the summary line (e.g. which transport a
    /// transfer used). Not an error; purely informational.
    pub note: Option<String>,
    /// At least one error was an OS "permission denied". On Windows this is the
    /// signal to offer an elevated (administrator) retry.
    pub permission_denied: bool,
}

impl OpReport {
    pub fn merge(&mut self, other: OpReport) {
        self.ok += other.ok;
        self.skipped += other.skipped;
        self.errors.extend(other.errors);
        self.permission_denied |= other.permission_denied;
        if other.note.is_some() {
            self.note = other.note;
        }
    }
    pub fn note_error(&mut self, msg: impl Into<String>) {
        self.errors.push(msg.into());
    }
}

/// Give `dst` the modification time `src` has.
///
/// **A copy is not a new file.** Every other tool that moves bytes around —
/// `cp -p`, robocopy, Explorer, Finder, afxw — hands the destination the
/// source's date, because the date is part of what was being copied: it is how
/// a backup is read, how "which of these two is current" is answered, and how
/// a delivery is dated. cian used to stamp every copy with the moment it ran,
/// which quietly flattened whole trees to "today" and could not be noticed
/// afterwards — `dirdiff` compares by content, so cian's own comparison is
/// deliberately blind to exactly this.
///
/// Best-effort, like the permission copy beside it: a destination whose time
/// could not be set is still a good copy of the bytes, and failing the whole
/// operation over the date would be the worse trade.
pub fn copy_times(src: &Path, dst: &Path) {
    if let Ok(t) = fs::symlink_metadata(src).and_then(|m| m.modified()) {
        set_mtime(dst, t);
    }
}

/// Stamp one path — file **or directory** — with a modification time.
///
/// Through `filetime` rather than `File::set_modified`, which needs the path
/// opened for writing: a directory cannot be opened that way on Unix at all,
/// and a read-only file cannot either, so half the paths that need a date
/// would silently keep the wrong one. `utimensat` / `SetFileTime` take the
/// path instead and answer for both.
pub fn set_mtime(path: &Path, t: std::time::SystemTime) {
    let _ = filetime::set_file_mtime(path, filetime::FileTime::from_system_time(t));
}

/// Every path under `root`, with its modification time, as paths relative to
/// `root`. Taken **before** a transfer, so a move that had to fall back to
/// copy-and-delete can still be given the dates it destroyed.
///
/// Files come first and directories last, deepest first: a directory's own
/// mtime is reset by the filesystem every time something is written inside it,
/// so a parent stamped before its children is stamped again by the next child
/// and the work is thrown away.
pub fn times_of(root: &Path) -> Vec<(PathBuf, std::time::SystemTime)> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    if !root.is_dir() {
        if let Ok(t) = fs::metadata(root).and_then(|m| m.modified()) {
            files.push((PathBuf::new(), t));
        }
        return files;
    }
    let mut queue = vec![PathBuf::new()];
    let mut at = 0;
    while at < queue.len() {
        let rel = queue[at].clone();
        at += 1;
        if let Ok(t) = fs::metadata(root.join(&rel)).and_then(|m| m.modified()) {
            dirs.push((rel.clone(), t));
        }
        let Ok(rd) = fs::read_dir(root.join(&rel)) else { continue };
        for e in rd.flatten() {
            let child = rel.join(e.file_name());
            let p = root.join(&child);
            if p.is_dir() && !p.is_symlink() {
                queue.push(child);
            } else if let Ok(t) = fs::symlink_metadata(&p).and_then(|m| m.modified()) {
                files.push((child, t));
            }
        }
    }
    dirs.reverse();
    files.extend(dirs);
    files
}

/// Put back what [`times_of`] took, against a destination root.
pub fn apply_times(dst_root: &Path, times: &[(PathBuf, std::time::SystemTime)]) {
    for (rel, t) in times {
        let p = if rel.as_os_str().is_empty() { dst_root.to_path_buf() } else { dst_root.join(rel) };
        set_mtime(&p, *t);
    }
}

/// Are these two paths the same file on disk?
///
/// Compared by what the filesystem says rather than by the text, because the
/// same file has many spellings — a symlinked folder, a case-insensitive
/// volume, `.` in the middle — and the one that matters is whichever the user
/// typed by accident.
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        // A destination that does not exist yet cannot be the source.
        _ => false,
    }
}

fn dest_for(src: &Path, dest_dir: &Path) -> PathBuf {
    let name = src
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    dest_dir.join(name)
}

/// Copy one entry into `dest_dir`, keeping its name. `false` means the target
/// was already there and the conflict rule said to leave it.
pub fn copy_one(src: &Path, dest_dir: &Path, on_conflict: Conflict) -> Result<bool> {
    transfer_one(src, dest_dir, on_conflict, false)
}

/// What copying `srcs` into `dest_dir` would bring into being: the destination
/// roots that are not there yet.
///
/// This is the whole safety argument for undoing a copy. A copy is additive,
/// so taking one back means deleting — and a key that sometimes deletes is
/// only trustworthy if it can never reach something that was not the copy's.
/// A root that already exists was either skipped or written over, and in both
/// cases what is under that name is partly or wholly somebody else's; it is
/// left out here and the copy of it is simply not undoable. What is left is
/// exactly what did not exist a moment ago.
///
/// Derived *before* the work, like the pairs a move remembers — afterwards
/// every root exists and the two cases are indistinguishable.
pub fn copy_creates(srcs: &[PathBuf], dest_dir: &Path) -> Vec<PathBuf> {
    srcs.iter()
        .filter(|src| src.file_name().is_some())
        .map(|src| dest_for(src, dest_dir))
        .filter(|root| !root.exists())
        .collect()
}

/// Which of `srcs` would land on something that is already there.
///
/// The other half of [`copy_creates`], and the one the person has to see
/// *before* deciding: the confirmation offers "skip the duplicates" and
/// "overwrite", and until this was on screen both were answered blind — there
/// was no way to tell one collision from thirty, or to know there were none.
pub fn clashes(srcs: &[PathBuf], dest_dir: &Path) -> Vec<PathBuf> {
    srcs.iter()
        .filter(|src| src.file_name().is_some())
        .filter(|src| dest_for(src, dest_dir).exists())
        .cloned()
        .collect()
}

/// The same, but the source does not survive it.
pub fn move_one(src: &Path, dest_dir: &Path, on_conflict: Conflict) -> Result<bool> {
    transfer_one(src, dest_dir, on_conflict, true)
}

/// The two above. They were written out twice and differed in one call each —
/// `dir::copy` against `dir::move_dir`, `file::copy` against `file::move_file`
/// — plus the verb in the message when it goes wrong.
fn transfer_one(
    src: &Path,
    dest_dir: &Path,
    on_conflict: Conflict,
    moving: bool,
) -> Result<bool> {
    let target = dest_for(src, dest_dir);

    // Where it already is, and where it cannot go.
    //
    // Copying a file into its own directory used to be carried out: the
    // destination resolved to the same path, the copy opened it for writing —
    // the same inode — truncated it, and wrote the nothing it could now read.
    // Eight bytes in, zero out, reported as a success. Two panes on one folder
    // is a keystroke away, so this was reachable by accident.
    //
    // A directory into itself is the other shape of it, and never ends.
    if same_file(src, &target) {
        anyhow::bail!("{} is already there — a file cannot be copied onto itself", src.display());
    }
    if src.is_dir() && dest_dir.starts_with(src) {
        anyhow::bail!("{} cannot be put inside itself", src.display());
    }

    if target.exists() && on_conflict == Conflict::Skip {
        return Ok(false);
    }
    // Read before the work, because the work can destroy the answer: a move is
    // a rename where it can be, and copy-and-delete where it cannot, and in the
    // second case the source is gone by the time anyone could ask it.
    let times = times_of(src);
    let verb = if moving { "move" } else { "copy" };
    if src.is_dir() {
        let mut opts = DirCopyOptions::new();
        opts.overwrite = on_conflict == Conflict::Overwrite;
        opts.copy_inside = false;
        let done = if moving {
            dir::move_dir(src, dest_dir, &opts)
        } else {
            dir::copy(src, dest_dir, &opts)
        };
        done.with_context(|| {
            format!("{verb} dir {} -> {}", src.display(), dest_dir.display())
        })?;
    } else {
        let mut opts = FileCopyOptions::new();
        opts.overwrite = on_conflict == Conflict::Overwrite;
        let done = if moving {
            file::move_file(src, &target, &opts)
        } else {
            file::copy(src, &target, &opts)
        };
        done.with_context(|| {
            format!("{verb} file {} -> {}", src.display(), target.display())
        })?;
    }
    apply_times(&target, &times);
    Ok(true)
}

/// How a delete disposes of its targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    /// Move to the OS trash (Finder's Trash / the Windows Recycle Bin), so the
    /// user can undo a mistake. The default: `d` is one keystroke away from
    /// destroying work, and a file manager used daily must be forgiving.
    Trash,
    /// Unlink immediately. Unrecoverable.
    Permanent,
}

pub fn delete_one(src: &Path, mode: DeleteMode) -> Result<()> {
    match mode {
        #[cfg(feature = "desktop")]
        DeleteMode::Trash => trash::delete(src)
            .with_context(|| format!("move to trash: {}", src.display()))?,
        // A build without a desktop under it. Refusing is the only honest
        // answer: the caller asked for the recoverable delete, and doing the
        // permanent one instead would be the opposite of what was asked.
        #[cfg(not(feature = "desktop"))]
        DeleteMode::Trash => anyhow::bail!(
            "この版にゴミ箱はありません（完全削除なら Permanent）: {}",
            src.display()
        ),
        DeleteMode::Permanent => {
            if src.is_dir() {
                fs::remove_dir_all(src).with_context(|| format!("rm -r {}", src.display()))?;
            } else {
                fs::remove_file(src).with_context(|| format!("rm {}", src.display()))?;
            }
        }
    }
    Ok(())
}

pub fn rename_in_place(src: &Path, new_name: &str) -> Result<PathBuf> {
    let parent = src
        .parent()
        .with_context(|| format!("no parent for {}", src.display()))?;
    let dest = parent.join(new_name);
    fs::rename(src, &dest)
        .with_context(|| format!("rename {} -> {}", src.display(), dest.display()))?;
    Ok(dest)
}

/// Strip a UTF-8 byte-order mark from the head of `path`, in place (via a
/// sibling temp + rename, so a crash never half-writes). Returns what
/// happened: `Some(true)` stripped, `Some(false)` no UTF-8 BOM to strip, and
/// `None` for a UTF-16 BOM — which is left alone on purpose: without it a
/// UTF-16 file's byte order is anyone's guess, so there it is load-bearing.
pub fn strip_utf8_bom(path: &Path) -> Result<Option<bool>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Ok(None);
    }
    if !bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Ok(Some(false));
    }
    let tmp = path.with_extension("cian-bom-tmp");
    fs::write(&tmp, &bytes[3..]).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(Some(true))
}

pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf> {
    let p = parent.join(name);
    if p.exists() {
        anyhow::bail!("already exists: {}", p.display());
    }
    fs::File::create(&p).with_context(|| format!("touch {}", p.display()))?;
    Ok(p)
}

pub fn create_dir(parent: &Path, name: &str) -> Result<PathBuf> {
    let p = parent.join(name);
    if p.exists() {
        anyhow::bail!("already exists: {}", p.display());
    }
    fs::create_dir(&p).with_context(|| format!("mkdir {}", p.display()))?;
    Ok(p)
}

/// `mkdir`, optionally `-p`.
///
/// `spec` may contain path separators (`a/b/c`); without `parents` every
/// component but the last must already exist, matching plain `mkdir`. With
/// `parents` the whole chain is made and an existing target is not an error,
/// matching `mkdir -p`.
pub fn make_dir(parent: &Path, spec: &str, parents: bool) -> Result<PathBuf> {
    let p = parent.join(spec);
    if parents {
        fs::create_dir_all(&p).with_context(|| format!("mkdir -p {}", p.display()))?;
    } else {
        if p.exists() {
            anyhow::bail!("already exists: {} (use -p to ignore)", p.display());
        }
        fs::create_dir(&p).with_context(|| format!("mkdir {}", p.display()))?;
    }
    Ok(p)
}

/// `touch`: create the file if missing, otherwise bump its modification time.
pub fn touch(parent: &Path, name: &str) -> Result<PathBuf> {
    let p = parent.join(name);
    let existed = p.exists();
    let f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .with_context(|| format!("touch {}", p.display()))?;
    if existed {
        // Only worth moving the clock on a file that was already there; a
        // fresh one is already stamped now.
        f.set_modified(std::time::SystemTime::now())
            .with_context(|| format!("touch {}", p.display()))?;
    }
    Ok(p)
}

/// Bulk copy with a single conflict policy applied to every source.
pub fn copy_many(srcs: &[PathBuf], dest_dir: &Path, on_conflict: Conflict) -> OpReport {
    let mut report = OpReport::default();
    for src in srcs {
        match copy_one(src, dest_dir, on_conflict) {
            Ok(true) => report.ok += 1,
            Ok(false) => report.skipped += 1,
            Err(e) => report.note_error(format!("{}: {}", src.display(), e)),
        }
    }
    report
}

pub fn move_many(srcs: &[PathBuf], dest_dir: &Path, on_conflict: Conflict) -> OpReport {
    let mut report = OpReport::default();
    for src in srcs {
        match move_one(src, dest_dir, on_conflict) {
            Ok(true) => report.ok += 1,
            Ok(false) => report.skipped += 1,
            Err(e) => report.note_error(format!("{}: {}", src.display(), e)),
        }
    }
    report
}

pub fn delete_many(srcs: &[PathBuf], mode: DeleteMode) -> OpReport {
    let mut report = OpReport::default();
    for src in srcs {
        match delete_one(src, mode) {
            Ok(()) => report.ok += 1,
            // `{e:#}` rather than `{e}`: anyhow's plain Display prints only
            // the outermost context, so every trash failure read "move to
            // trash: <path>" and named no cause at all. The cause underneath
            // is the half that says what to do — a permission macOS is
            // withholding reads nothing like a volume that has no trash.
            Err(e) => report.note_error(format!("{}: {:#}", src.display(), e)),
        }
    }
    report
}

#[cfg(test)]
mod transfer_tests {
    use super::*;

    /// Copying a file into the directory it is already in destroyed it.
    ///
    /// Two panes showing the same folder is one keystroke away — they start
    /// there — and `c` then asked the filesystem to copy a file over itself.
    /// It opened the destination for writing, which is the same inode,
    /// truncated it to nothing, and copied the nothing. Nine bytes in, zero
    /// bytes out, reported as a success.
    #[test]
    fn a_copy_onto_itself_is_refused_rather_than_emptying_the_file() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("note.txt");
        std::fs::write(&f, b"sample 1").unwrap();

        let err = copy_one(&f, d.path(), Conflict::Overwrite).unwrap_err();
        assert!(err.to_string().contains("itself"), "says what is wrong: {err}");
        assert_eq!(std::fs::read(&f).unwrap(), b"sample 1", "and the file is untouched");

        let err = move_one(&f, d.path(), Conflict::Overwrite).unwrap_err();
        assert!(err.to_string().contains("itself"), "a move is the same mistake");
        assert!(f.exists(), "still there");
    }

    /// And a directory cannot be given a home inside itself, which never ends.
    #[test]
    fn a_directory_is_refused_a_home_inside_itself() {
        let d = tempfile::tempdir().unwrap();
        let inner = d.path().join("outer/inner");
        std::fs::create_dir_all(&inner).unwrap();

        let err = copy_one(&d.path().join("outer"), &inner, Conflict::Overwrite).unwrap_err();
        assert!(err.to_string().contains("itself"), "says so: {err}");
    }
}

#[cfg(test)]
mod make_touch_tests {
    use super::*;

    #[test]
    fn mkdir_p_creates_a_chain_and_tolerates_existing() {
        let d = tempfile::tempdir().unwrap();
        let made = make_dir(d.path(), "a/b/c", true).unwrap();
        assert!(made.is_dir());
        assert!(d.path().join("a/b/c").is_dir());
        // -p run twice is not an error.
        assert!(make_dir(d.path(), "a/b/c", true).is_ok());
    }

    #[test]
    fn plain_mkdir_needs_the_parent_and_refuses_an_existing_dir() {
        let d = tempfile::tempdir().unwrap();
        // No parent yet: plain mkdir fails.
        assert!(make_dir(d.path(), "x/y", false).is_err());
        make_dir(d.path(), "x", false).unwrap();
        make_dir(d.path(), "x/y", false).unwrap();
        // Existing: refused without -p.
        assert!(make_dir(d.path(), "x", false).is_err());
    }

    #[test]
    fn touch_creates_then_bumps_the_mtime() {
        let d = tempfile::tempdir().unwrap();
        let p = touch(d.path(), "note.txt").unwrap();
        assert!(p.is_file());
        // Content is preserved when touched again (append mode, nothing written).
        fs::write(&p, b"keep me").unwrap();
        let before = fs::metadata(&p).unwrap().modified().unwrap();
        // Force a distinctly older stamp, then touch and confirm it advanced.
        // The handle must be writable to set times on Windows, so open for
        // write rather than read.
        let old = before - std::time::Duration::from_secs(120);
        fs::OpenOptions::new().write(true).open(&p).unwrap().set_modified(old).unwrap();
        touch(d.path(), "note.txt").unwrap();
        let after = fs::metadata(&p).unwrap().modified().unwrap();
        assert!(after > old, "mtime moved forward");
        assert_eq!(fs::read(&p).unwrap(), b"keep me", "contents untouched");
    }

    #[test]
    fn copy_creates_names_only_what_is_not_there_yet() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("new.txt"), b"a").unwrap();
        fs::write(src.path().join("clash.txt"), b"b").unwrap();
        fs::create_dir(src.path().join("tree")).unwrap();
        // Already at the destination: whatever happens to it, undoing the copy
        // must not reach it.
        fs::write(dst.path().join("clash.txt"), b"theirs").unwrap();

        let srcs = vec![
            src.path().join("new.txt"),
            src.path().join("clash.txt"),
            src.path().join("tree"),
        ];
        let made = copy_creates(&srcs, dst.path());
        assert_eq!(
            made,
            vec![dst.path().join("new.txt"), dst.path().join("tree")],
            "the pre-existing name is left out, in source order"
        );
    }
}
