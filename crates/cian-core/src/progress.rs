//! Long-running file operations that report progress and can be stopped.
//!
//! The plain routines in [`crate::ops`] block until they finish, which is fine
//! for a handful of small files and unusable for anything bigger: copying a
//! 700 MB file froze the whole UI for fourteen seconds with nothing on screen
//! to say why. Everything here takes a cancel flag and a progress callback so
//! the caller can run it on a worker thread, draw a bar, and abandon it.
//!
//! Files are copied in chunks rather than with a single `fs::copy` so that
//! progress advances *within* a large file and a cancel is noticed promptly,
//! instead of only between files.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

use crate::ops::{Conflict, OpReport};

/// Copy buffer. Large enough that the syscall overhead disappears, small
/// enough that a cancel is acted on quickly even on a slow volume.
const CHUNK: usize = 1024 * 1024;

/// How far along a running operation is.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub files_done: usize,
    pub files_total: usize,
    /// The entry being worked on, for display.
    pub current: String,
}

impl Progress {
    /// Completion in 0.0..=1.0, by bytes where that is known.
    pub fn fraction(&self) -> f32 {
        if self.bytes_total > 0 {
            (self.bytes_done as f64 / self.bytes_total as f64) as f32
        } else if self.files_total > 0 {
            self.files_done as f32 / self.files_total as f32
        } else {
            0.0
        }
    }
}

/// Everything a job needs to report in and be stopped.
pub struct Ctl<'a> {
    pub cancel: &'a AtomicBool,
    pub on_progress: &'a mut dyn FnMut(&Progress),
}

impl Ctl<'_> {
    fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Every regular file under `root`, paired with its path relative to `root`.
/// A file passed directly comes back as a single entry with an empty relative
/// path.
fn walk(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    if !root.is_dir() {
        return vec![(root.to_path_buf(), PathBuf::new())];
    }
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            // Symlinks are copied as the link's target would be read; not
            // following them into a cycle matters more than fidelity here.
            if p.is_dir() && !p.is_symlink() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_path_buf();
                out.push((p, rel));
            }
        }
    }
    out
}

/// Total size of `paths`, for the progress bar's denominator.
pub fn total_size(paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .flat_map(|p| walk(p))
        .filter_map(|(abs, _)| fs::metadata(abs).ok())
        .map(|m| m.len())
        .sum()
}

/// Copy one file in chunks. Returns `false` if it was cancelled part-way, in
/// which case the partial destination is removed rather than left behind
/// looking like a complete file.
fn copy_file(src: &Path, dst: &Path, ctl: &mut Ctl, p: &mut Progress) -> Result<bool> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let mut r = fs::File::open(src).with_context(|| format!("open {}", src.display()))?;
    let mut w = fs::File::create(dst).with_context(|| format!("create {}", dst.display()))?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        if ctl.stopped() {
            drop(w);
            let _ = fs::remove_file(dst);
            return Ok(false);
        }
        let n = r.read(&mut buf).with_context(|| format!("read {}", src.display()))?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n]).with_context(|| format!("write {}", dst.display()))?;
        p.bytes_done += n as u64;
        (ctl.on_progress)(p);
    }
    // Best-effort: losing the mode should not fail an otherwise good copy.
    if let Ok(meta) = fs::metadata(src) {
        let _ = fs::set_permissions(dst, meta.permissions());
    }
    // The date is part of what was copied, not a record of when the copy ran.
    // Here rather than in a second pass over the tree, because the loop is
    // already holding both paths and a walk of its own would cost a stat per
    // file to learn what this line already knows.
    crate::ops::copy_times(src, dst);
    Ok(true)
}

/// Every directory under `root`, relative to it, deepest first — the order the
/// dates have to go on in, since writing a child resets its parent's mtime.
/// Empty for a plain file, which has no directory of its own to stamp.
fn dirs_deepest_first(root: &Path) -> Vec<PathBuf> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut out = vec![PathBuf::new()];
    let mut at = 0;
    while at < out.len() {
        let rel = out[at].clone();
        at += 1;
        let Ok(rd) = fs::read_dir(root.join(&rel)) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && !p.is_symlink() {
                out.push(rel.join(e.file_name()));
            }
        }
    }
    out.reverse();
    out
}

/// Copy `srcs` into `dest_dir`, reporting progress and honouring cancellation.
pub fn copy_many(
    srcs: &[PathBuf],
    dest_dir: &Path,
    on_conflict: Conflict,
    ctl: &mut Ctl,
) -> OpReport {
    let mut report = OpReport::default();
    let mut p = Progress {
        bytes_total: total_size(srcs),
        files_total: srcs.iter().flat_map(|s| walk(s)).count(),
        ..Default::default()
    };
    for src in srcs {
        if ctl.stopped() {
            break;
        }
        let Some(name) = src.file_name() else {
            report.note_error(format!("{}: has no name", src.display()));
            continue;
        };
        let root = dest_dir.join(name);
        if root.exists() && on_conflict == Conflict::Skip {
            report.skipped += 1;
            continue;
        }
        let mut failed = false;
        for (abs, rel) in walk(src) {
            if ctl.stopped() {
                break;
            }
            let dst = if rel.as_os_str().is_empty() { root.clone() } else { root.join(&rel) };
            p.current = abs.display().to_string();
            (ctl.on_progress)(&p);
            match copy_file(&abs, &dst, ctl, &mut p) {
                Ok(true) => p.files_done += 1,
                // Cancelled: stop without recording an error.
                Ok(false) => break,
                Err(e) => {
                    if crate::elevate::is_permission_denied(&e) {
                        report.permission_denied = true;
                    }
                    report.note_error(format!("{}: {}", abs.display(), e));
                    failed = true;
                }
            }
        }
        // The files carried their own dates across as they were written; the
        // directories could not, because every file written inside one moves
        // its mtime again. So they go on last, once nothing more will be
        // written under this root, deepest first.
        if !failed && !ctl.stopped() {
            for rel in dirs_deepest_first(src) {
                crate::ops::copy_times(&src.join(&rel), &root.join(&rel));
            }
            report.ok += 1;
        }
    }
    report
}

/// Move `srcs` into `dest_dir`.
///
/// A rename is tried first: within a volume it is instant, and no amount of
/// progress reporting beats not copying at all. Only when that fails — the
/// usual reason being a different filesystem — does it fall back to copying
/// and then removing the source.
pub fn move_many(
    srcs: &[PathBuf],
    dest_dir: &Path,
    on_conflict: Conflict,
    ctl: &mut Ctl,
) -> OpReport {
    let mut report = OpReport::default();
    let mut slow = Vec::new();
    for src in srcs {
        if ctl.stopped() {
            break;
        }
        let Some(name) = src.file_name() else { continue };
        let target = dest_dir.join(name);
        if target.exists() {
            if on_conflict == Conflict::Skip {
                report.skipped += 1;
                continue;
            }
            let _ = if target.is_dir() {
                fs::remove_dir_all(&target)
            } else {
                fs::remove_file(&target)
            };
        }
        match fs::rename(src, &target) {
            Ok(()) => report.ok += 1,
            Err(_) => slow.push(src.clone()),
        }
    }
    if slow.is_empty() {
        return report;
    }
    // Cross-volume leftovers: copy with progress, then remove the sources that
    // arrived intact.
    let copied = copy_many(&slow, dest_dir, Conflict::Overwrite, ctl);
    let had_errors = !copied.errors.is_empty();
    report.merge(copied);
    if !ctl.stopped() && !had_errors {
        for src in &slow {
            let r = if src.is_dir() { fs::remove_dir_all(src) } else { fs::remove_file(src) };
            if let Err(e) = r {
                report.note_error(format!("{}: copied but not removed: {}", src.display(), e));
            }
        }
    }
    report
}

/// Delete `srcs`, reporting progress between entries.
pub fn delete_many(
    srcs: &[PathBuf],
    mode: crate::ops::DeleteMode,
    ctl: &mut Ctl,
) -> OpReport {
    let mut report = OpReport::default();
    let mut p = Progress { files_total: srcs.len(), ..Default::default() };
    for src in srcs {
        if ctl.stopped() {
            break;
        }
        p.current = src.display().to_string();
        (ctl.on_progress)(&p);
        match crate::ops::delete_one(src, mode) {
            Ok(()) => report.ok += 1,
            Err(e) => report.note_error(format!("{}: {}", src.display(), e)),
        }
        p.files_done += 1;
        (ctl.on_progress)(&p);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Conflict;

    fn nil(_: &Progress) {}

    fn ctl<'a>(cancel: &'a AtomicBool, f: &'a mut dyn FnMut(&Progress)) -> Ctl<'a> {
        Ctl { cancel, on_progress: f }
    }

    fn mtime(p: &Path) -> std::time::SystemTime {
        fs::metadata(p).unwrap().modified().unwrap()
    }

    /// A copy carries the source's date. Not a detail: `cp -p`, robocopy,
    /// Explorer and afxw all do it, and cian stamping "now" on every file
    /// flattened whole trees to the day they were copied — invisibly, because
    /// `dirdiff` compares by content and is deliberately blind to mtime.
    ///
    /// Directories are checked too, and they are the harder half: a directory's
    /// mtime moves again every time a file is written inside it, so stamping it
    /// before its contents does nothing at all.
    #[test]
    fn a_copy_keeps_the_dates_it_copied() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("d/deeper")).unwrap();
        fs::write(src.path().join("d/a.txt"), b"a").unwrap();
        fs::write(src.path().join("d/deeper/b.txt"), b"b").unwrap();

        // Well before now, and each one different, so a test that passes by
        // accident (everything stamped with the same "now") cannot.
        let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        let want: Vec<(PathBuf, std::time::SystemTime)> = [
            ("d/deeper/b.txt", 30),
            ("d/a.txt", 20),
            ("d/deeper", 10),
            ("d", 0),
        ]
        .iter()
        .map(|(rel, off)| {
            let t = base + std::time::Duration::from_secs(*off);
            crate::ops::set_mtime(&src.path().join(rel), t);
            (PathBuf::from(rel), t)
        })
        .collect();

        let cancel = AtomicBool::new(false);
        let mut n = nil;
        let report = copy_many(
            &[src.path().join("d")],
            dst.path(),
            Conflict::Skip,
            &mut ctl(&cancel, &mut n),
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        for (rel, t) in &want {
            let at = dst.path().join(rel);
            assert_eq!(
                mtime(&at),
                *t,
                "{} came out with the date of the copy, not of the file",
                rel.display()
            );
        }
    }

    /// `:cp` and the copy-across in the comparison view do not go through
    /// `copy_many` — they call `ops::copy_one`, which hands the work to
    /// `fs_extra` and lost the date the same way. Two doors, one rule.
    #[test]
    fn the_other_copy_path_keeps_the_dates_too() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::create_dir(src.path().join("d")).unwrap();
        fs::write(src.path().join("d/a.txt"), b"a").unwrap();
        let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_100_000_000);
        let (tf, td) = (base, base + std::time::Duration::from_secs(5));
        crate::ops::set_mtime(&src.path().join("d/a.txt"), tf);
        crate::ops::set_mtime(&src.path().join("d"), td);

        crate::ops::copy_one(&src.path().join("d"), dst.path(), Conflict::Skip).unwrap();

        assert_eq!(mtime(&dst.path().join("d/a.txt")), tf, "the file lost its date");
        assert_eq!(mtime(&dst.path().join("d")), td, "the directory lost its date");
    }

    #[test]
    fn copies_a_tree_and_reports_progress_along_the_way() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::create_dir(src.path().join("d")).unwrap();
        fs::write(src.path().join("d/a.bin"), vec![7u8; 300_000]).unwrap();
        fs::write(src.path().join("d/b.bin"), vec![7u8; 200_000]).unwrap();

        let cancel = AtomicBool::new(false);
        let mut seen: Vec<u64> = Vec::new();
        let mut f = |p: &Progress| seen.push(p.bytes_done);
        let report = copy_many(
            &[src.path().join("d")],
            dst.path(),
            Conflict::Skip,
            &mut ctl(&cancel, &mut f),
        );

        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.ok, 1);
        assert_eq!(fs::read(dst.path().join("d/a.bin")).unwrap().len(), 300_000);
        assert_eq!(fs::read(dst.path().join("d/b.bin")).unwrap().len(), 200_000);

        // Progress must actually move, not jump from nothing to everything.
        assert!(seen.len() > 2, "expected several updates, got {}", seen.len());
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "progress went backwards");
        assert_eq!(*seen.last().unwrap(), 500_000);
    }

    /// Cancelling has to take effect inside a large file, not merely between
    /// files — one big copy is the case that hurts.
    #[test]
    fn cancelling_stops_partway_and_leaves_no_half_file() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let big = src.path().join("big.bin");
        fs::write(&big, vec![0u8; CHUNK * 8]).unwrap();

        let cancel = AtomicBool::new(false);
        let mut hits = 0;
        let mut f = |_: &Progress| {
            hits += 1;
            if hits == 2 {
                // Standing in for the user pressing Esc mid-copy.
                cancel.store(true, Ordering::Relaxed);
            }
        };
        let report = copy_many(&[big], dst.path(), Conflict::Skip, &mut ctl(&cancel, &mut f));

        assert!(report.errors.is_empty(), "cancelling is not an error: {:?}", report.errors);
        assert_eq!(report.ok, 0, "an abandoned copy did not succeed");
        assert!(
            !dst.path().join("big.bin").exists(),
            "a partial file must not be left looking complete"
        );
    }

    #[test]
    fn skip_and_overwrite_are_honoured() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("f.txt"), b"new").unwrap();
        fs::write(dst.path().join("f.txt"), b"old").unwrap();
        let cancel = AtomicBool::new(false);
        let mut n = nil;

        let r = copy_many(
            &[src.path().join("f.txt")],
            dst.path(),
            Conflict::Skip,
            &mut ctl(&cancel, &mut n),
        );
        assert_eq!(r.skipped, 1);
        assert_eq!(fs::read(dst.path().join("f.txt")).unwrap(), b"old");

        let r = copy_many(
            &[src.path().join("f.txt")],
            dst.path(),
            Conflict::Overwrite,
            &mut ctl(&cancel, &mut n),
        );
        assert_eq!(r.ok, 1);
        assert_eq!(fs::read(dst.path().join("f.txt")).unwrap(), b"new");
    }

    #[test]
    fn moving_within_a_volume_renames_and_removes_the_source() {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("from");
        let dst = root.path().join("to");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("x.txt"), b"data").unwrap();

        let cancel = AtomicBool::new(false);
        let mut n = nil;
        let r = move_many(&[src.join("x.txt")], &dst, Conflict::Skip, &mut ctl(&cancel, &mut n));

        assert_eq!(r.ok, 1);
        assert!(dst.join("x.txt").exists());
        assert!(!src.join("x.txt").exists(), "the source must be gone");
    }

    #[test]
    fn total_size_counts_a_whole_tree() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("sub")).unwrap();
        fs::write(d.path().join("a"), vec![0u8; 10]).unwrap();
        fs::write(d.path().join("sub/b"), vec![0u8; 25]).unwrap();
        assert_eq!(total_size(&[d.path().to_path_buf()]), 35);
    }

    #[test]
    fn fraction_prefers_bytes_and_never_divides_by_zero() {
        let p = Progress { bytes_done: 1, bytes_total: 4, ..Default::default() };
        assert!((p.fraction() - 0.25).abs() < 1e-6);
        // No byte total (a delete): fall back to counting entries.
        let p = Progress { files_done: 1, files_total: 2, ..Default::default() };
        assert!((p.fraction() - 0.5).abs() < 1e-6);
        assert_eq!(Progress::default().fraction(), 0.0);
    }
}
