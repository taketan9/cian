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
}

impl OpReport {
    pub fn merge(&mut self, other: OpReport) {
        self.ok += other.ok;
        self.skipped += other.skipped;
        self.errors.extend(other.errors);
    }
    pub fn note_error(&mut self, msg: impl Into<String>) {
        self.errors.push(msg.into());
    }
}

fn dest_for(src: &Path, dest_dir: &Path) -> PathBuf {
    let name = src
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    dest_dir.join(name)
}

pub fn copy_one(src: &Path, dest_dir: &Path, on_conflict: Conflict) -> Result<bool> {
    let target = dest_for(src, dest_dir);
    if target.exists() && on_conflict == Conflict::Skip {
        return Ok(false);
    }
    if src.is_dir() {
        let mut opts = DirCopyOptions::new();
        opts.overwrite = on_conflict == Conflict::Overwrite;
        opts.copy_inside = false;
        dir::copy(src, dest_dir, &opts)
            .with_context(|| format!("copy dir {} -> {}", src.display(), dest_dir.display()))?;
    } else {
        let mut opts = FileCopyOptions::new();
        opts.overwrite = on_conflict == Conflict::Overwrite;
        file::copy(src, &target, &opts)
            .with_context(|| format!("copy file {} -> {}", src.display(), target.display()))?;
    }
    Ok(true)
}

pub fn move_one(src: &Path, dest_dir: &Path, on_conflict: Conflict) -> Result<bool> {
    let target = dest_for(src, dest_dir);
    if target.exists() && on_conflict == Conflict::Skip {
        return Ok(false);
    }
    if src.is_dir() {
        let mut opts = DirCopyOptions::new();
        opts.overwrite = on_conflict == Conflict::Overwrite;
        opts.copy_inside = false;
        dir::move_dir(src, dest_dir, &opts)
            .with_context(|| format!("move dir {} -> {}", src.display(), dest_dir.display()))?;
    } else {
        let mut opts = FileCopyOptions::new();
        opts.overwrite = on_conflict == Conflict::Overwrite;
        file::move_file(src, &target, &opts)
            .with_context(|| format!("move file {} -> {}", src.display(), target.display()))?;
    }
    Ok(true)
}

pub fn delete_one(src: &Path) -> Result<()> {
    if src.is_dir() {
        fs::remove_dir_all(src).with_context(|| format!("rm -r {}", src.display()))?;
    } else {
        fs::remove_file(src).with_context(|| format!("rm {}", src.display()))?;
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

pub fn delete_many(srcs: &[PathBuf]) -> OpReport {
    let mut report = OpReport::default();
    for src in srcs {
        match delete_one(src) {
            Ok(()) => report.ok += 1,
            Err(e) => report.note_error(format!("{}: {}", src.display(), e)),
        }
    }
    report
}
