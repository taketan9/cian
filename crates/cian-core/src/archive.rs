//! Looking inside a zip, and taking things out of it.
//!
//! Browsing an archive as though it were a directory is the feature people
//! come to a two-pane file manager for, and the thing they miss first when it
//! is absent. Only zip is handled: it is what Windows produces and consumes
//! without extra software, and claiming to open archives while silently
//! failing on half of them would be worse than being clear about the one.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use anyhow::{Context, Result};

use crate::ops::OpReport;
use crate::progress::{Ctl, Progress};

/// One member of an archive.
#[derive(Debug, Clone)]
pub struct Member {
    /// Path within the archive.
    pub name: String,
    pub is_dir: bool,
    /// Size once extracted.
    pub size: u64,
    pub compressed: u64,
}

/// The archive formats cian can look inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zip,
    Tar,
    TarGz,
}

/// Classify by name. `.tar.gz`/`.tgz` are matched on the whole filename because
/// `Path::extension` only sees the trailing `.gz`.
pub fn kind(path: &Path) -> Option<Kind> {
    let name = path.file_name()?.to_str()?.to_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        return Some(Kind::TarGz);
    }
    if name.ends_with(".tar") {
        return Some(Kind::Tar);
    }
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
        Some("zip") | Some("jar") | Some("xpi") | Some("whl") | Some("epub") => Some(Kind::Zip),
        _ => None,
    }
}

/// Whether cian can look inside this file, judged by extension.
///
/// Extension rather than content sniffing: the answer decides what a keypress
/// does, so it has to be known before the file is opened, and being wrong
/// merely means falling back to the viewer.
pub fn is_archive(path: &Path) -> bool {
    kind(path).is_some()
}

/// True when `path` is a zip with at least one encrypted member — so the caller
/// knows to ask for a password before extracting. (tar has no encryption.)
pub fn zip_needs_password(path: &Path) -> bool {
    if kind(path) != Some(Kind::Zip) {
        return false;
    }
    let Ok(f) = fs::File::open(path) else { return false };
    let Ok(mut zip) = zip::ZipArchive::new(f) else { return false };
    (0..zip.len()).any(|i| zip.by_index_raw(i).map(|e| e.encrypted() && !e.is_dir()).unwrap_or(false))
}

/// A reader over a tarball, transparently gunzipped when `gz`.
fn tar_reader(path: &Path, gz: bool) -> Result<Box<dyn Read>> {
    let f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    Ok(if gz {
        Box::new(flate2::read::GzDecoder::new(f))
    } else {
        Box::new(f)
    })
}

/// List an archive's contents.
pub fn list(path: &Path) -> Result<Vec<Member>> {
    match kind(path) {
        Some(Kind::Tar) => list_tar(tar_reader(path, false)?),
        Some(Kind::TarGz) => list_tar(tar_reader(path, true)?),
        // Zip, and anything unrecognised, are tried as a zip.
        _ => list_zip(path),
    }
}

fn list_zip(path: &Path) -> Result<Vec<Member>> {
    let f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    // listing visits every member's local header (a seek and a read
    // each — two system calls per member). A zip small enough to hold is
    // read once and listed from memory; a large one stays on disk.
    const IN_MEMORY: u64 = 64 << 20;
    if f.metadata().map(|m| m.len() <= IN_MEMORY).unwrap_or(false) {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut &f, &mut bytes).with_context(|| format!("read {}", path.display()))?;
        return list_zip_from(path, std::io::Cursor::new(bytes));
    }
    list_zip_from(path, f)
}

fn list_zip_from<R: std::io::Read + std::io::Seek>(path: &Path, f: R) -> Result<Vec<Member>> {
    let mut zip = zip::ZipArchive::new(f)
        .with_context(|| format!("not a readable zip: {}", path.display()))?;
    let mut out = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        // `by_index_raw` reads the entry's metadata without decrypting it, so a
        // password-protected zip still lists its members (names and sizes live
        // in the central directory, not behind the password).
        let e = zip.by_index_raw(i).with_context(|| format!("entry {} of {}", i, path.display()))?;
        out.push(Member {
            name: member_name(e.name(), e.name_raw()),
            is_dir: e.is_dir(),
            size: e.size(),
            compressed: e.compressed_size(),
        });
    }
    // Archives are stored in whatever order they were written; a listing wants
    // to be predictable.
    out.sort_by_cached_key(|m| m.name.to_lowercase()); // one lowercase per member, not per comparison
    Ok(out)
}

fn list_tar(reader: Box<dyn Read>) -> Result<Vec<Member>> {
    let mut ar = tar::Archive::new(reader);
    let mut out = Vec::new();
    for entry in ar.entries().context("read tar")? {
        let entry = entry.context("tar entry")?;
        let header = entry.header();
        let is_dir = header.entry_type().is_dir();
        let size = header.size().unwrap_or(0);
        let name = entry.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        // A tar has no per-entry compressed size; show the stored size.
        out.push(Member { name, is_dir, size, compressed: size });
    }
    out.sort_by_cached_key(|m| m.name.to_lowercase()); // one lowercase per member, not per comparison
    Ok(out)
}

/// Reject member paths that would escape the destination directory.
///
/// A zip can name `../../etc/passwd`, or an absolute path, and a naive
/// extractor will happily write there. Everything is resolved to a plain
/// relative path underneath `dest` or refused.
fn safe_join(dest: &Path, name: &str) -> Option<PathBuf> {
    let mut out = dest.to_path_buf();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return None,
            p => {
                // A Windows drive letter or a leading slash would otherwise
                // make the join absolute and throw away `dest` entirely.
                if p.contains(':') {
                    return None;
                }
                out.push(p);
            }
        }
    }
    if out.starts_with(dest) {
        Some(out)
    } else {
        None
    }
}

/// Extract `members` — or all of them, if empty — into `dest`. `password` is
/// used for encrypted zip members (ignored for tar, which has no encryption).
/// `strip` is a member-path prefix removed before writing (`""` keeps full
/// paths): extracting `a/b/c.txt` with `strip = "a/b/"` lands `dest/c.txt`.
/// This is what "copy out of the folder I am in inside the archive" means —
/// without it every copy-out would rebuild the archive's whole tree.
pub fn extract(
    archive: &Path,
    members: &[String],
    dest: &Path,
    password: Option<&str>,
    strip: &str,
    ctl: &mut Ctl,
) -> OpReport {
    match kind(archive) {
        Some(Kind::Tar) => extract_tar(archive, false, members, dest, strip, ctl),
        Some(Kind::TarGz) => extract_tar(archive, true, members, dest, strip, ctl),
        _ => extract_zip(archive, members, dest, password, strip, ctl),
    }
}

/// Stream a tarball (optionally gunzipped), writing the wanted members. Tar is
/// sequential, so this walks the whole archive once, skipping over anything not
/// requested. A first header-only pass gives the progress bar its totals.
fn extract_tar(
    archive: &Path,
    gz: bool,
    members: &[String],
    dest: &Path,
    strip: &str,
    ctl: &mut Ctl,
) -> OpReport {
    use std::io::Write as _;
    let mut report = OpReport::default();
    let wants = |name: &str| members.is_empty() || members.iter().any(|m| m == name);

    // Totals from a header scan (re-decompresses for .gz, cheap enough for a
    // denominator).
    let (files_total, bytes_total) = match tar_reader(archive, gz).and_then(list_tar) {
        Ok(list) => (
            list.iter().filter(|m| !m.is_dir && wants(&m.name)).count(),
            list.iter().filter(|m| !m.is_dir && wants(&m.name)).map(|m| m.size).sum(),
        ),
        Err(e) => {
            report.note_error(format!("{}: {}", archive.display(), e));
            return report;
        }
    };
    let mut p = Progress { files_total, bytes_total, ..Default::default() };

    let reader = match tar_reader(archive, gz) {
        Ok(r) => r,
        Err(e) => {
            report.note_error(format!("{}: {}", archive.display(), e));
            return report;
        }
    };
    let mut ar = tar::Archive::new(reader);
    let entries = match ar.entries() {
        Ok(e) => e,
        Err(e) => {
            report.note_error(format!("{}: {}", archive.display(), e));
            return report;
        }
    };
    for entry in entries {
        if ctl.cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut e = match entry {
            Ok(e) => e,
            Err(err) => {
                report.note_error(format!("tar entry: {}", err));
                continue;
            }
        };
        let name = e.path().map(|pp| pp.to_string_lossy().into_owned()).unwrap_or_default();
        if name.is_empty() || !wants(&name) {
            continue;
        }
        let rel = name.strip_prefix(strip).unwrap_or(&name);
        let Some(target) = safe_join(dest, rel) else {
            report.note_error(format!("{}: refused, escapes the destination", name));
            continue;
        };
        if e.header().entry_type().is_dir() {
            if let Err(err) = fs::create_dir_all(&target) {
                report.note_error(format!("{}: {}", name, err));
            }
            continue;
        }
        if let Some(parent) = target.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                report.note_error(format!("{}: {}", name, err));
                continue;
            }
        }
        p.current = name.clone();
        (ctl.on_progress)(&p);
        let mut out = match fs::File::create(&target) {
            Ok(f) => f,
            Err(err) => {
                report.note_error(format!("{}: {}", name, err));
                continue;
            }
        };
        let mut buf = vec![0u8; 256 * 1024];
        let mut failed = false;
        loop {
            if ctl.cancel.load(Ordering::Relaxed) {
                drop(out);
                let _ = fs::remove_file(&target);
                return report;
            }
            match e.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Err(err) = out.write_all(&buf[..n]) {
                        report.note_error(format!("{}: {}", name, err));
                        failed = true;
                        break;
                    }
                    p.bytes_done += n as u64;
                    (ctl.on_progress)(&p);
                }
                Err(err) => {
                    report.note_error(format!("{}: {}", name, err));
                    failed = true;
                    break;
                }
            }
        }
        if !failed {
            report.ok += 1;
        }
        p.files_done += 1;
    }
    report
}

fn extract_zip(
    archive: &Path,
    members: &[String],
    dest: &Path,
    password: Option<&str>,
    strip: &str,
    ctl: &mut Ctl,
) -> OpReport {
    let mut report = OpReport::default();
    let f = match fs::File::open(archive) {
        Ok(f) => f,
        Err(e) => {
            report.note_error(format!("{}: {}", archive.display(), e));
            return report;
        }
    };
    let mut zip = match zip::ZipArchive::new(f) {
        Ok(z) => z,
        Err(e) => {
            report.note_error(format!("{}: {}", archive.display(), e));
            return report;
        }
    };

    // Metadata reads use `by_index_raw` so an encrypted zip can still be
    // filtered and sized without the password.
    let wanted: Vec<usize> = (0..zip.len())
        .filter(|i| {
            members.is_empty()
                || zip.by_index_raw(*i)
                    .map(|e| members.iter().any(|m| *m == member_name(e.name(), e.name_raw())))
                    .unwrap_or(false)
        })
        .collect();
    let mut p = Progress {
        files_total: wanted.len(),
        bytes_total: wanted
            .iter()
            .filter_map(|i| zip.by_index_raw(*i).ok().map(|e| e.size()))
            .sum(),
        ..Default::default()
    };

    for i in wanted {
        if ctl.cancel.load(Ordering::Relaxed) {
            break;
        }
        // Decrypt with the password only for entries that need it; a wrong
        // password surfaces as a per-member error, so the run reports it.
        let encrypted = zip.by_index_raw(i).map(|e| e.encrypted()).unwrap_or(false);
        let opened = if encrypted {
            match password {
                Some(pw) => zip.by_index_decrypt(i, pw.as_bytes()),
                None => {
                    report.note_error(format!("entry {}: encrypted — a password is required", i));
                    continue;
                }
            }
        } else {
            zip.by_index(i)
        };
        let mut e = match opened {
            Ok(e) => e,
            Err(err) => {
                report.note_error(format!("entry {}: {}", i, err));
                continue;
            }
        };
        let name = member_name(e.name(), e.name_raw());
        let rel = name.strip_prefix(strip).unwrap_or(&name);
        let Some(target) = safe_join(dest, rel) else {
            report.note_error(format!("{}: refused, escapes the destination", name));
            continue;
        };
        p.current = name.clone();
        (ctl.on_progress)(&p);

        if e.is_dir() {
            if let Err(err) = fs::create_dir_all(&target) {
                report.note_error(format!("{}: {}", name, err));
            }
            continue;
        }
        if let Some(parent) = target.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                report.note_error(format!("{}: {}", name, err));
                continue;
            }
        }
        let mut out = match fs::File::create(&target) {
            Ok(f) => f,
            Err(err) => {
                report.note_error(format!("{}: {}", name, err));
                continue;
            }
        };
        // Copied in chunks so progress moves inside a large member and a
        // cancel is noticed without waiting for it to finish.
        let mut buf = vec![0u8; 256 * 1024];
        let mut failed = false;
        loop {
            if ctl.cancel.load(Ordering::Relaxed) {
                drop(out);
                let _ = fs::remove_file(&target);
                return report;
            }
            match e.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    use std::io::Write as _;
                    if let Err(err) = out.write_all(&buf[..n]) {
                        report.note_error(format!("{}: {}", name, err));
                        failed = true;
                        break;
                    }
                    p.bytes_done += n as u64;
                    (ctl.on_progress)(&p);
                }
                Err(err) => {
                    report.note_error(format!("{}: {}", name, err));
                    failed = true;
                    break;
                }
            }
        }
        if !failed {
            report.ok += 1;
        }
        p.files_done += 1;
    }
    report
}

/// Modify a zip in place: drop members, rename members, and/or add local
/// files under `add_prefix` (directories recurse). One entry point for every
/// mutation the archive browser makes.
///
/// Always works on a sibling temp file and renames it over the original only
/// on success, so a cancel or crash never leaves a half-written archive —
/// the fast path (nothing dropped/renamed, no name collisions) copies the
/// file and appends to the copy; the general path raw-copies the kept
/// members (no recompression) into a fresh zip.
///
/// Password-protected zips are refused: mixing cleartext additions into an
/// AES archive produces a file that *looks* protected but is not.
/// Change a tarball: drop members, rename members, add files.
///
/// The same four verbs as [`zip_modify`], and the same shape of answer, so the
/// callers do not have to know which they are holding.
///
/// **A tar is rewritten, not edited.** There is no central directory to patch
/// and no random access — a gzipped one cannot even be seeked. So this streams
/// the old archive into a new one, leaving out what was dropped and renaming
/// what was renamed, appends the additions, and swaps the file at the end.
/// That is also what `zip_modify` does underneath; the difference is only that
/// a zip *could* have been patched and a tar could not.
///
/// The original is replaced only once the new one is complete. A rewrite that
/// fails halfway leaves the archive it was given, rather than a truncated one:
/// this is somebody's only copy often enough to be worth the temporary file.
pub fn tar_modify(
    archive: &Path,
    drop_members: &[String],
    rename_members: &[(String, String)],
    add_sources: &[PathBuf],
    add_prefix: &str,
    ctl: &mut Ctl,
) -> OpReport {
    let mut report = OpReport::default();
    let gz = matches!(kind(archive), Some(Kind::TarGz));

    // The additions, flattened the way `zip_modify` flattens them.
    let mut jobs: Vec<(PathBuf, String)> = Vec::new();
    for src in add_sources {
        let base = match src.file_name().and_then(|n| n.to_str()) {
            Some(n) => format!("{add_prefix}{n}"),
            None => {
                report.note_error(format!("{}: unusable name", src.display()));
                continue;
            }
        };
        if src.is_dir() {
            collect_tree(src, &base, &mut jobs, &mut report);
        } else {
            jobs.push((src.clone(), base));
        }
    }

    let renamed: std::collections::HashMap<&str, &str> =
        rename_members.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let dropped: std::collections::HashSet<&str> =
        drop_members.iter().map(String::as_str).collect();

    // Beside the archive, so the final rename cannot cross a filesystem —
    // `/tmp` is a different device often enough that a cross-device rename is
    // a real failure rather than a theoretical one.
    let tmp = archive.with_extension(format!(
        "{}.cian-new",
        archive.extension().and_then(|e| e.to_str()).unwrap_or("tar")
    ));

    let outcome = (|| -> Result<()> {
        let out = fs::File::create(&tmp)
            .with_context(|| format!("write {}", tmp.display()))?;
        let mut builder: Box<dyn TarSink> = if gz {
            Box::new(tar::Builder::new(flate2::write::GzEncoder::new(
                out,
                flate2::Compression::default(),
            )))
        } else {
            Box::new(tar::Builder::new(out))
        };

        let reader = tar_reader(archive, gz)?;
        let mut ar = tar::Archive::new(reader);
        for entry in ar.entries().context("read tar")? {
            if ctl.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!("cancelled");
            }
            let mut entry = entry.context("read tar entry")?;
            let name = entry.path().context("tar entry name")?.to_string_lossy().into_owned();
            if dropped.contains(name.as_str()) {
                continue;
            }
            // Its own header, so mode and mtime survive the rewrite. A tar
            // that comes back with everything 0644 and today's date is a tar
            // that has lost what a tar is for.
            let mut header = entry.header().clone();
            let to = renamed.get(name.as_str()).copied().unwrap_or(name.as_str());
            let mut body = Vec::new();
            entry.read_to_end(&mut body).context("read tar entry body")?;
            header.set_size(body.len() as u64);
            header.set_cksum();
            builder.append_named(&header, to, &body)?;
            report.ok += 1;
        }

        for (from, name) in &jobs {
            if ctl.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!("cancelled");
            }
            let body = fs::read(from).with_context(|| format!("read {}", from.display()))?;
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_named(&header, name, &body)?;
            report.ok += 1;
        }
        builder.finish()
    })();

    if let Err(e) = outcome {
        let _ = fs::remove_file(&tmp);
        report.ok = 0;
        report.note_error(format!("{e:#}"));
        return report;
    }
    if let Err(e) = fs::rename(&tmp, archive) {
        let _ = fs::remove_file(&tmp);
        report.ok = 0;
        report.note_error(format!("replace {}: {e}", archive.display()));
    }
    report
}

/// What `tar_modify` writes through, so the gzipped and plain cases are one
/// loop rather than two copies of it.
trait TarSink {
    fn append_named(&mut self, header: &tar::Header, name: &str, body: &[u8]) -> Result<()>;
    fn finish(self: Box<Self>) -> Result<()>;
}

impl<W: std::io::Write> TarSink for tar::Builder<W> {
    fn append_named(&mut self, header: &tar::Header, name: &str, body: &[u8]) -> Result<()> {
        let mut h = header.clone();
        h.set_cksum();
        self.append_data(&mut h, name, body).context("write tar entry")
    }
    fn finish(mut self: Box<Self>) -> Result<()> {
        tar::Builder::finish(&mut *self).context("finish tar")?;
        Ok(())
    }
}

pub fn zip_modify(
    archive: &Path,
    drop_members: &[String],
    rename_members: &[(String, String)],
    add_sources: &[PathBuf],
    add_prefix: &str,
    ctl: &mut Ctl,
) -> OpReport {
    use std::io::Write as _;

    let mut report = OpReport::default();
    if zip_needs_password(archive) {
        report.note_error("password-protected zip — modifying is not supported".to_string());
        return report;
    }

    // The additions, flattened (dirs recurse), members named under the prefix.
    let mut jobs: Vec<(PathBuf, String)> = Vec::new();
    for src in add_sources {
        let base = match src.file_name().and_then(|n| n.to_str()) {
            Some(n) => format!("{}{}", add_prefix, n),
            None => {
                report.note_error(format!("{}: unusable name", src.display()));
                continue;
            }
        };
        if src.is_dir() {
            collect_tree(src, &base, &mut jobs, &mut report);
        } else {
            jobs.push((src.clone(), base));
        }
    }

    let existing: std::collections::HashSet<String> = match list(archive) {
        Ok(m) => m.into_iter().map(|m| m.name).collect(),
        Err(e) => {
            report.note_error(format!("{}: {}", archive.display(), e));
            return report;
        }
    };
    let collides = jobs.iter().any(|(_, name)| existing.contains(name));
    let append_only = drop_members.is_empty() && rename_members.is_empty() && !collides;

    let tmp = archive.with_extension("cian-zip-tmp");
    let fail = |report: &mut OpReport, tmp: &Path, msg: String| {
        report.note_error(msg);
        let _ = fs::remove_file(tmp);
    };

    let mut p = Progress {
        files_total: jobs.len(),
        bytes_total: jobs.iter().filter_map(|(pth, _)| fs::metadata(pth).ok().map(|m| m.len())).sum(),
        ..Default::default()
    };

    let mut zip = if append_only {
        // Fast path: copy, then append to the copy. The original is untouched
        // until the final rename, so cancelling can simply delete the temp.
        if let Err(e) = fs::copy(archive, &tmp) {
            report.note_error(format!("{}: {}", archive.display(), e));
            return report;
        }
        let f = match fs::OpenOptions::new().read(true).write(true).open(&tmp) {
            Ok(f) => f,
            Err(e) => return { fail(&mut report, &tmp, e.to_string()); report },
        };
        match zip::ZipWriter::new_append(f) {
            Ok(z) => z,
            Err(e) => return { fail(&mut report, &tmp, e.to_string()); report },
        }
    } else {
        // General path: raw-copy the kept members into a fresh zip — the
        // stored bytes move as-is, so nothing is recompressed.
        let src_f = match fs::File::open(archive) {
            Ok(f) => f,
            Err(e) => {
                report.note_error(format!("{}: {}", archive.display(), e));
                return report;
            }
        };
        let mut src = match zip::ZipArchive::new(src_f) {
            Ok(z) => z,
            Err(e) => {
                report.note_error(format!("{}: {}", archive.display(), e));
                return report;
            }
        };
        let out = match fs::File::create(&tmp) {
            Ok(f) => f,
            Err(e) => {
                report.note_error(e.to_string());
                return report;
            }
        };
        let mut zip = zip::ZipWriter::new(out);
        let dropped: std::collections::HashSet<&str> =
            drop_members.iter().map(|s| s.as_str()).collect();
        let added: std::collections::HashSet<&str> =
            jobs.iter().map(|(_, n)| n.as_str()).collect();
        for i in 0..src.len() {
            if ctl.cancel.load(Ordering::Relaxed) {
                drop(zip);
                let _ = fs::remove_file(&tmp);
                return report;
            }
            let entry = match src.by_index_raw(i) {
                Ok(e) => e,
                Err(e) => {
                    report.note_error(format!("entry {}: {}", i, e));
                    continue;
                }
            };
            let name = member_name(entry.name(), entry.name_raw());
            // Dropped, or about to be replaced by an addition of the same name.
            if dropped.contains(name.as_str()) || added.contains(name.as_str()) {
                continue;
            }
            let renamed = rename_members.iter().find(|(from, _)| *from == name);
            let res = match renamed {
                Some((_, to)) => zip.raw_copy_file_rename(entry, to.as_str()),
                None => zip.raw_copy_file(entry),
            };
            if let Err(e) = res {
                fail(&mut report, &tmp, format!("{}: {}", name, e));
                return report;
            }
        }
        zip
    };

    // Append the additions (both paths end here).
    let base_opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (path, name) in &jobs {
        if ctl.cancel.load(Ordering::Relaxed) {
            drop(zip);
            let _ = fs::remove_file(&tmp);
            return report;
        }
        p.current = name.clone();
        (ctl.on_progress)(&p);
        // **The file is opened before its entry is started**, and a failure
        // abandons the whole rewrite (2026-10-30). Started first, an unreadable
        // replacement left the member at zero bytes — and the original had
        // already been left out, so the archive was rewritten without it.
        let mut src_f = match fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                drop(zip);
                fail(&mut report, &tmp, format!("{}: {}", name, e));
                return report;
            }
        };
        if let Err(e) = zip.start_file(name, base_opts) {
            report.note_error(format!("{}: {}", name, e));
            continue;
        }
        let mut buf = vec![0u8; 256 * 1024];
        let mut failed = false;
        loop {
            match src_f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Err(e) = zip.write_all(&buf[..n]) {
                        report.note_error(format!("{}: {}", name, e));
                        failed = true;
                        break;
                    }
                    p.bytes_done += n as u64;
                    (ctl.on_progress)(&p);
                }
                Err(e) => {
                    report.note_error(format!("{}: {}", name, e));
                    failed = true;
                    break;
                }
            }
        }
        if !failed {
            report.ok += 1;
        }
        p.files_done += 1;
    }
    // Deletions and renames count as completed work too.
    report.ok += drop_members.len() + rename_members.len();

    if let Err(e) = zip.finish() {
        fail(&mut report, &tmp, format!("{}: {}", archive.display(), e));
        return report;
    }
    if let Err(e) = fs::rename(&tmp, archive) {
        fail(&mut report, &tmp, format!("{}: {}", archive.display(), e));
    }
    report
}

/// Build a zip at `dest` from `sources`, optionally encrypted with `password`.
///
/// Directories are added recursively, with their tree preserved relative to
/// the source's own name, so zipping `proj/` yields members `proj/...`. When a
/// password is given the members are AES-256 encrypted — strong, but note that
/// Windows Explorer's built-in unzip cannot open AES zips (7-Zip and the like
/// can); the caller is expected to have warned about that.
pub fn create_zip(
    sources: &[PathBuf],
    dest: &Path,
    password: Option<&str>,
    ctl: &mut Ctl,
) -> OpReport {
    use std::io::Write as _;

    let mut report = OpReport::default();
    let f = match fs::File::create(dest) {
        Ok(f) => f,
        Err(e) => {
            report.note_error(format!("{}: {}", dest.display(), e));
            return report;
        }
    };
    let mut zip = zip::ZipWriter::new(f);

    // Gather the members first so progress has a total and so an empty
    // selection is caught before a file is created.
    let mut jobs: Vec<(PathBuf, String)> = Vec::new();
    for src in sources {
        let base = match src.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => {
                report.note_error(format!("{}: unusable name", src.display()));
                continue;
            }
        };
        if src.is_dir() {
            collect_tree(src, &base, &mut jobs, &mut report);
        } else {
            jobs.push((src.clone(), base));
        }
    }

    let mut p = Progress {
        files_total: jobs.len(),
        bytes_total: jobs.iter().filter_map(|(pth, _)| fs::metadata(pth).ok().map(|m| m.len())).sum(),
        ..Default::default()
    };

    let base_opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for (path, name) in jobs {
        if ctl.cancel.load(Ordering::Relaxed) {
            drop(zip);
            let _ = fs::remove_file(dest);
            return report;
        }
        p.current = name.clone();
        (ctl.on_progress)(&p);

        // `FileOptions` is Copy, so `base_opts` is reused each iteration.
        let opts = match password {
            Some(pw) => base_opts.with_aes_encryption(zip::AesMode::Aes256, pw),
            None => base_opts,
        };
        if let Err(e) = zip.start_file(&name, opts) {
            report.note_error(format!("{}: {}", name, e));
            continue;
        }
        let mut src_f = match fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                report.note_error(format!("{}: {}", name, e));
                continue;
            }
        };
        let mut buf = vec![0u8; 256 * 1024];
        let mut failed = false;
        loop {
            match src_f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Err(e) = zip.write_all(&buf[..n]) {
                        report.note_error(format!("{}: {}", name, e));
                        failed = true;
                        break;
                    }
                    p.bytes_done += n as u64;
                    (ctl.on_progress)(&p);
                }
                Err(e) => {
                    report.note_error(format!("{}: {}", name, e));
                    failed = true;
                    break;
                }
            }
        }
        if !failed {
            report.ok += 1;
        }
        p.files_done += 1;
    }

    if let Err(e) = zip.finish() {
        report.note_error(format!("{}: {}", dest.display(), e));
    }
    report
}

/// Create a tarball at `dest` from `sources`; `gz` gzips it (`.tar.gz`).
/// Directories are added recursively with their tree preserved under the
/// source's own name, matching [`create_zip`]. A cancelled run removes the
/// partial file, so a half-written archive is never left behind.
pub fn create_tar(sources: &[PathBuf], dest: &Path, gz: bool, ctl: &mut Ctl) -> OpReport {
    use std::io::{BufWriter, Write};

    let mut report = OpReport::default();

    // Same gathering as create_zip: flatten directories to file members first.
    let mut jobs: Vec<(PathBuf, String)> = Vec::new();
    for src in sources {
        let base = match src.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => {
                report.note_error(format!("{}: unusable name", src.display()));
                continue;
            }
        };
        if src.is_dir() {
            collect_tree(src, &base, &mut jobs, &mut report);
        } else {
            jobs.push((src.clone(), base));
        }
    }

    let mut p = Progress {
        files_total: jobs.len(),
        bytes_total: jobs.iter().filter_map(|(pth, _)| fs::metadata(pth).ok().map(|m| m.len())).sum(),
        ..Default::default()
    };

    let f = match fs::File::create(dest) {
        Ok(f) => f,
        Err(e) => {
            report.note_error(format!("{}: {}", dest.display(), e));
            return report;
        }
    };

    // Generic over the writer so the gzip and plain paths share the member loop
    // while each still owns a concrete writer to finish/flush correctly.
    fn write_members<W: Write>(
        w: W,
        jobs: &[(PathBuf, String)],
        ctl: &mut Ctl<'_>,
        report: &mut OpReport,
        p: &mut Progress,
    ) -> std::io::Result<W> {
        let mut b = tar::Builder::new(w);
        for (path, name) in jobs {
            if ctl.cancel.load(Ordering::Relaxed) {
                break;
            }
            p.current = name.clone();
            (ctl.on_progress)(p);
            match b.append_path_with_name(path, name) {
                Ok(()) => {
                    report.ok += 1;
                    p.files_done += 1;
                    p.bytes_done += fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                    (ctl.on_progress)(p);
                }
                Err(e) => report.note_error(format!("{}: {}", name, e)),
            }
        }
        b.into_inner()
    }

    let result = if gz {
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        write_members(enc, &jobs, ctl, &mut report, &mut p).and_then(|enc| enc.finish().map(|_| ()))
    } else {
        let bw = BufWriter::new(f);
        write_members(bw, &jobs, ctl, &mut report, &mut p).and_then(|mut bw| bw.flush())
    };
    if let Err(e) = result {
        report.note_error(format!("{}: {}", dest.display(), e));
    }
    if ctl.cancel.load(Ordering::Relaxed) {
        let _ = fs::remove_file(dest);
    }
    report
}

/// Add every file under `dir` to `jobs`, naming each member relative to
/// `prefix` so the directory structure is kept inside the zip.
fn collect_tree(dir: &Path, prefix: &str, jobs: &mut Vec<(PathBuf, String)>, report: &mut OpReport) {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            report.note_error(format!("{}: {}", dir.display(), e));
            return;
        }
    };
    for e in rd.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        // Forward slashes: the zip format mandates them, and a backslash from
        // Windows would otherwise be stored as part of the name.
        let member = format!("{}/{}", prefix, name);
        if path.is_dir() {
            collect_tree(&path, &member, jobs, report);
        } else {
            jobs.push((path, member));
        }
    }
}

// ---- Reading an archive as if it were a directory ----
//
// Down here rather than in a front end because it is arithmetic on member
// names and nothing else: which of them are directly under this prefix, which
// deeper ones collapse into a directory row. Two front ends browsing the same
// zip and disagreeing about what is in it would be a strange thing to ship.

/// The rows for directory `sub` (`""` = root) of `members`: the `..` row,
/// then direct child directories (explicit or implied by deeper members),
/// then direct child files. Pure, so the tests can pin the shape down.
pub fn archive_rows(archive: &Path, members: &[Member], sub: &str) -> Vec<crate::Entry> {
    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut files: Vec<(String, u64)> = Vec::new();
    for m in members {
        let name = m.name.trim_end_matches('/');
        let Some(rest) = name.strip_prefix(sub) else { continue };
        if rest.is_empty() {
            continue; // the directory entry for `sub` itself
        }
        match rest.split_once('/') {
            // Deeper than one level: only its first segment shows, as a dir.
            Some((head, _)) => {
                dirs.insert(head.to_string());
            }
            None => {
                if m.is_dir {
                    dirs.insert(rest.to_string());
                } else {
                    files.push((rest.to_string(), m.size));
                }
            }
        }
    }
    files.sort();
    // The `..` row: its synthetic path is the archive-or-parent target, but
    // navigation intercepts it before anything touches the path.
    let up = crate::Entry::remote("..", archive.display().to_string(), true, 0, true);
    let mut out = vec![up];
    for d in dirs {
        let p = format!("{}/{}{}", archive.display(), sub, d);
        out.push(crate::Entry::remote(d, p, true, 0, false));
    }
    for (f, size) in files {
        let p = format!("{}/{}{}", archive.display(), sub, f);
        out.push(crate::Entry::remote(f, p, false, size, false));
    }
    out
}

/// The member names under `prefix` (the entries a dir row stands for),
/// including the file `prefix` itself when it names a file directly.
pub fn members_under<'a>(members: &'a [Member], prefix: &str) -> Vec<&'a Member> {
    members
        .iter()
        .filter(|m| {
            let name = m.name.trim_end_matches('/');
            name == prefix.trim_end_matches('/') || name.starts_with(&format!("{}/", prefix.trim_end_matches('/')))
        })
        .collect()
}


/// One rule for both front ends (2026-10-30): the window extracted straight
/// into the current folder and overwrote what was there with the same name.
///
/// The base name of an archive without its extension, handling the two-part
/// `.tar.gz` / `.tar.bz2` / `.tar.xz` / `.tgz` cases: `proj.tar.gz` → `proj`.
pub fn archive_stem(name: &str) -> String {
    let low = name.to_lowercase();
    for suf in [".tar.gz", ".tar.bz2", ".tar.xz", ".tgz"] {
        if low.ends_with(suf) {
            return name[..name.len() - suf.len()].to_string();
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name.to_string(),
    }
}

/// A directory path under `parent` named `stem`, made unique by appending
/// `-1`, `-2`, … so extracting never merges into an existing folder.
pub fn unique_dir(parent: &Path, stem: &str) -> PathBuf {
    let base = parent.join(stem);
    if !base.exists() {
        return base;
    }
    for n in 1.. {
        let cand = parent.join(format!("{stem}-{n}"));
        if !cand.exists() {
            return cand;
        }
    }
    base
}


/// A member's name as a person would read it.
///
/// A zip made by Explorer on a Japanese Windows writes names in CP932 and does
/// not set the UTF-8 flag, so the zip crate reads them as CP437:
/// `日本語.txt` arrived as `ô·û{îΩ.txt` — listed, extracted and matched by that
/// name (2026-10-30). Valid UTF-8 is taken as it is (that covers every name
/// with the flag set, and plain ASCII); otherwise a clean CP932 decode wins;
/// otherwise the zip crate's reading stands. One function, because the name is
/// also the key a member is found by when extracting or rewriting.
fn member_name(read: &str, raw: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    let (s, _, bad) = encoding_rs::SHIFT_JIS.decode(raw);
    if bad { read.to_string() } else { s.into_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::AtomicBool;

    fn make_zip(dir: &Path) -> PathBuf {
        let path = dir.join("a.zip");
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        w.add_directory("sub/", opts).unwrap();
        w.start_file("readme.txt", opts).unwrap();
        w.write_all(b"hello from the archive").unwrap();
        w.start_file("sub/inner.txt", opts).unwrap();
        w.write_all(b"nested").unwrap();
        w.finish().unwrap();
        path
    }

    fn ctl<'a>(c: &'a AtomicBool, f: &'a mut dyn FnMut(&Progress)) -> Ctl<'a> {
        Ctl { cancel: c, on_progress: f }
    }

    /// Build a tarball at `dir/a.tar` (or `.tar.gz` when `gz`) with the same
    /// shape as `make_zip`, so the tests can compare list/extract behaviour.
    fn make_tar(dir: &Path, gz: bool) -> PathBuf {
        let path = dir.join(if gz { "a.tar.gz" } else { "a.tar" });
        let write_tar = |w: &mut dyn Write| {
            let mut b = tar::Builder::new(w);
            let mut add = |name: &str, body: &[u8]| {
                let mut h = tar::Header::new_gnu();
                h.set_size(body.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                b.append_data(&mut h, name, body).unwrap();
            };
            add("readme.txt", b"hello from the archive");
            add("sub/inner.txt", b"nested");
            b.finish().unwrap();
        };
        let f = fs::File::create(&path).unwrap();
        if gz {
            let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
            write_tar(&mut enc);
            enc.finish().unwrap();
        } else {
            let mut bw = std::io::BufWriter::new(f);
            write_tar(&mut bw);
            bw.flush().unwrap();
        }
        path
    }

    #[test]
    fn recognises_archives_by_extension() {
        assert!(is_archive(Path::new("x.zip")));
        assert!(is_archive(Path::new("X.ZIP")));
        assert!(is_archive(Path::new("lib.jar")));
        assert!(is_archive(Path::new("src.tar")));
        assert!(is_archive(Path::new("src.tar.gz")));
        assert!(is_archive(Path::new("src.TGZ")));
        assert!(!is_archive(Path::new("notes.txt")));
        assert!(!is_archive(Path::new("noext")));
    }

    #[test]
    fn lists_and_extracts_tar_and_tar_gz() {
        for gz in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let t = make_tar(d.path(), gz);
            let members = list(&t).unwrap_or_else(|e| panic!("list (gz={gz}): {e}"));
            let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
            assert!(names.contains(&"readme.txt"), "gz={gz}: {names:?}");
            assert!(names.contains(&"sub/inner.txt"), "gz={gz}: {names:?}");

            let out = d.path().join("out");
            let cancel = AtomicBool::new(false);
            let mut prog = |_: &Progress| {};
            let mut c = ctl(&cancel, &mut prog);
            let report = extract(&t, &[], &out, None, "", &mut c);
            assert!(report.errors.is_empty(), "gz={gz}: {:?}", report.errors);
            assert_eq!(
                fs::read_to_string(out.join("readme.txt")).unwrap(),
                "hello from the archive"
            );
            assert_eq!(fs::read_to_string(out.join("sub/inner.txt")).unwrap(), "nested");
        }
    }

    #[test]
    fn creates_tar_and_tar_gz_that_round_trip() {
        for gz in [false, true] {
            let d = tempfile::tempdir().unwrap();
            // A directory to zip up: proj/main.rs and proj/sub/mod.rs.
            let proj = d.path().join("proj");
            fs::create_dir_all(proj.join("sub")).unwrap();
            fs::write(proj.join("main.rs"), b"fn main() {}").unwrap();
            fs::write(proj.join("sub/mod.rs"), b"// mod").unwrap();

            let dest = d.path().join(if gz { "out.tar.gz" } else { "out.tar" });
            let cancel = AtomicBool::new(false);
            let mut prog = |_: &Progress| {};
            let mut c = ctl(&cancel, &mut prog);
            let report = create_tar(std::slice::from_ref(&proj), &dest, gz, &mut c);
            assert!(report.errors.is_empty(), "gz={gz}: {:?}", report.errors);
            assert_eq!(report.ok, 2, "gz={gz}: two files added");

            // Read it back and extract; the tree is preserved under `proj/`.
            let members = list(&dest).unwrap();
            let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
            assert!(names.contains(&"proj/main.rs"), "gz={gz}: {names:?}");
            assert!(names.contains(&"proj/sub/mod.rs"), "gz={gz}: {names:?}");

            let out = d.path().join("out");
            let mut c2 = ctl(&cancel, &mut prog);
            extract(&dest, &[], &out, None, "", &mut c2);
            assert_eq!(fs::read_to_string(out.join("proj/main.rs")).unwrap(), "fn main() {}");
        }
    }

    #[test]
    fn lists_members_sorted_with_sizes() {
        let d = tempfile::tempdir().unwrap();
        let z = make_zip(d.path());
        let members = list(&z).unwrap();
        let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["readme.txt", "sub/", "sub/inner.txt"]);
        let readme = &members[0];
        assert_eq!(readme.size, "hello from the archive".len() as u64);
        assert!(members[1].is_dir);
    }

    #[test]
    fn extracts_everything_when_no_member_is_named() {
        let d = tempfile::tempdir().unwrap();
        let z = make_zip(d.path());
        let out = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let mut n = |_: &Progress| {};
        let report = extract(&z, &[], out.path(), None, "", &mut ctl(&cancel, &mut n));

        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(fs::read_to_string(out.path().join("readme.txt")).unwrap(), "hello from the archive");
        assert_eq!(fs::read_to_string(out.path().join("sub/inner.txt")).unwrap(), "nested");
    }

    #[test]
    fn extracts_only_the_named_member() {
        let d = tempfile::tempdir().unwrap();
        let z = make_zip(d.path());
        let out = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let mut n = |_: &Progress| {};
        extract(&z, &["readme.txt".to_string()], out.path(), None, "", &mut ctl(&cancel, &mut n));

        assert!(out.path().join("readme.txt").exists());
        assert!(!out.path().join("sub/inner.txt").exists(), "only what was asked for");
    }

    /// A zip can name `../../etc/passwd`; a naive extractor writes there.
    #[test]
    fn member_paths_cannot_escape_the_destination() {
        let dest = Path::new("/tmp/dest");
        assert_eq!(safe_join(dest, "a/b.txt"), Some(PathBuf::from("/tmp/dest/a/b.txt")));
        assert_eq!(safe_join(dest, "./a.txt"), Some(PathBuf::from("/tmp/dest/a.txt")));
        // Traversal, absolute paths and drive letters are all refused.
        assert_eq!(safe_join(dest, "../evil"), None);
        assert_eq!(safe_join(dest, "a/../../evil"), None);
        assert_eq!(safe_join(dest, "C:/Windows/system32/evil"), None);
        // A leading slash is stripped rather than making the join absolute.
        assert_eq!(safe_join(dest, "/etc/passwd"), Some(PathBuf::from("/tmp/dest/etc/passwd")));
    }

    #[test]
    fn a_traversing_member_is_reported_and_the_rest_still_extract() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("evil.zip");
        {
            let f = fs::File::create(&path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            w.start_file("../escaped.txt", opts).unwrap();
            w.write_all(b"nope").unwrap();
            w.start_file("fine.txt", opts).unwrap();
            w.write_all(b"ok").unwrap();
            w.finish().unwrap();
        }
        let out = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let mut n = |_: &Progress| {};
        let report = extract(&path, &[], out.path(), None, "", &mut ctl(&cancel, &mut n));

        assert_eq!(report.ok, 1, "the safe member still came out");
        assert!(report.errors.iter().any(|e| e.contains("escapes")), "{:?}", report.errors);
        assert!(out.path().join("fine.txt").exists());
        assert!(!out.path().parent().unwrap().join("escaped.txt").exists());
    }

    #[test]
    fn a_file_that_is_not_a_zip_reports_rather_than_panicking() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("notazip.zip");
        fs::write(&f, b"just text").unwrap();
        assert!(list(&f).is_err());
    }

    #[test]
    fn creates_a_zip_preserving_directory_structure() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("proj/sub")).unwrap();
        fs::write(d.path().join("proj/top.txt"), b"top").unwrap();
        fs::write(d.path().join("proj/sub/inner.txt"), b"inner").unwrap();
        fs::write(d.path().join("loose.txt"), b"loose").unwrap();

        let out = d.path().join("bundle.zip");
        let cancel = AtomicBool::new(false);
        let mut n = |_: &Progress| {};
        let report = create_zip(
            &[d.path().join("proj"), d.path().join("loose.txt")],
            &out,
            None,
            &mut ctl(&cancel, &mut n),
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.ok, 3);

        let names: Vec<String> = list(&out).unwrap().into_iter().map(|m| m.name).collect();
        assert!(names.contains(&"proj/top.txt".to_string()), "{:?}", names);
        assert!(names.contains(&"proj/sub/inner.txt".to_string()), "{:?}", names);
        assert!(names.contains(&"loose.txt".to_string()), "{:?}", names);
    }

    /// A password-protected member must actually refuse the wrong password and
    /// yield its contents with the right one.
    #[test]
    fn a_password_zip_round_trips_only_with_the_password() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("secret.txt"), b"classified").unwrap();
        let out = d.path().join("locked.zip");
        let cancel = AtomicBool::new(false);
        let mut n = |_: &Progress| {};
        let report =
            create_zip(&[d.path().join("secret.txt")], &out, Some("hunter2"), &mut ctl(&cancel, &mut n));
        assert_eq!(report.ok, 1, "{:?}", report.errors);

        let f = fs::File::open(&out).unwrap();
        let mut zip = zip::ZipArchive::new(f).unwrap();
        // Wrong password is refused.
        assert!(zip.by_name_decrypt("secret.txt", b"wrong").is_err());
        // Right password yields the bytes.
        let mut e = zip.by_name_decrypt("secret.txt", b"hunter2").unwrap();
        let mut got = String::new();
        e.read_to_string(&mut got).unwrap();
        assert_eq!(got, "classified");
    }

    #[test]
    fn encrypted_zip_lists_and_extracts_with_the_password() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("secret.txt"), b"classified").unwrap();
        let out = d.path().join("locked.zip");
        let cancel = AtomicBool::new(false);
        let mut n = |_: &Progress| {};
        create_zip(&[d.path().join("secret.txt")], &out, Some("hunter2"), &mut ctl(&cancel, &mut n));

        // Listing works without the password (this is what F3 does), and the zip
        // is flagged as needing one.
        assert!(zip_needs_password(&out), "flagged encrypted");
        let names: Vec<String> = list(&out).unwrap().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["secret.txt"], "lists despite encryption");

        // No / wrong password → a reported error, nothing written.
        let none = d.path().join("none");
        let r = extract(&out, &[], &none, None, "", &mut ctl(&cancel, &mut n));
        assert!(!r.errors.is_empty(), "no password is refused");
        assert!(!none.join("secret.txt").exists());

        let wrong = d.path().join("wrong");
        let r = extract(&out, &[], &wrong, Some("nope"), "", &mut ctl(&cancel, &mut n));
        assert!(!r.errors.is_empty(), "wrong password is refused");

        // Right password extracts the plaintext.
        let ok = d.path().join("ok");
        let r = extract(&out, &[], &ok, Some("hunter2"), "", &mut ctl(&cancel, &mut n));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(fs::read_to_string(ok.join("secret.txt")).unwrap(), "classified");
    }

    #[test]
    fn cancelling_a_zip_leaves_no_half_file_behind() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a.txt"), b"data").unwrap();
        let out = d.path().join("x.zip");
        let cancel = AtomicBool::new(true);
        let mut n = |_: &Progress| {};
        create_zip(&[d.path().join("a.txt")], &out, None, &mut ctl(&cancel, &mut n));
        assert!(!out.exists(), "a cancelled zip is cleaned up");
    }

    /// zip_modify's whole contract, one scenario per concern: add (with a
    /// directory), delete, rename, replace-on-collision — and the original
    /// survives an error or cancel untouched.
    #[test]
    fn zip_modify_adds_deletes_renames_and_replaces() {
        let d = tempfile::tempdir().unwrap();
        let z = make_zip(d.path());
        let names = |z: &Path| -> Vec<String> {
            let mut v: Vec<String> =
                list(z).unwrap().into_iter().map(|m| m.name).collect();
            v.sort();
            v
        };

        // Add a file and a directory under a prefix.
        fs::write(d.path().join("new.txt"), b"fresh").unwrap();
        fs::create_dir(d.path().join("pack")).unwrap();
        fs::write(d.path().join("pack/one.log"), b"1").unwrap();
        let cancel = AtomicBool::new(false);
        let mut n = |_: &Progress| {};
        let r = zip_modify(
            &z,
            &[],
            &[],
            &[d.path().join("new.txt"), d.path().join("pack")],
            "sub/",
            &mut ctl(&cancel, &mut n),
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(names(&z).contains(&"sub/new.txt".to_string()), "{:?}", names(&z));
        assert!(names(&z).contains(&"sub/pack/one.log".to_string()));

        // Delete one member; rename another.
        let mut n = |_: &Progress| {};
        let r = zip_modify(
            &z,
            &["sub/inner.txt".to_string()],
            &[("readme.txt".to_string(), "README.txt".to_string())],
            &[],
            "",
            &mut ctl(&cancel, &mut n),
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let ns = names(&z);
        assert!(!ns.contains(&"sub/inner.txt".to_string()), "deleted: {ns:?}");
        assert!(ns.contains(&"README.txt".to_string()), "renamed: {ns:?}");
        assert!(!ns.contains(&"readme.txt".to_string()));

        // Adding an existing name replaces it (rewrite path, not a duplicate).
        fs::write(d.path().join("README.txt"), b"replaced body").unwrap();
        let mut n = |_: &Progress| {};
        let r = zip_modify(&z, &[], &[], &[d.path().join("README.txt")], "", &mut ctl(&cancel, &mut n));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let count = names(&z).iter().filter(|s| s.as_str() == "README.txt").count();
        assert_eq!(count, 1, "replaced, not duplicated");
        let out = tempfile::tempdir().unwrap();
        let mut n = |_: &Progress| {};
        extract(&z, &["README.txt".to_string()], out.path(), None, "", &mut ctl(&cancel, &mut n));
        assert_eq!(fs::read(out.path().join("README.txt")).unwrap(), b"replaced body");

        // A cancelled modify leaves the archive exactly as it was.
        let before = fs::read(&z).unwrap();
        let cancelled = AtomicBool::new(true);
        let mut n = |_: &Progress| {};
        let _ = zip_modify(
            &z,
            &["README.txt".to_string()],
            &[],
            &[],
            "",
            &mut ctl(&cancelled, &mut n),
        );
        assert_eq!(fs::read(&z).unwrap(), before, "cancel never half-writes");
        assert!(!z.with_extension("cian-zip-tmp").exists(), "temp cleaned up");
    }

    /// A password-protected zip is refused outright: appending cleartext into
    /// an AES archive would look protected while not being so.
    #[test]
    fn zip_modify_refuses_encrypted_archives() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("s.txt"), b"x").unwrap();
        let z = d.path().join("locked.zip");
        let cancel = AtomicBool::new(false);
        let mut n = |_: &Progress| {};
        create_zip(&[d.path().join("s.txt")], &z, Some("pw"), &mut ctl(&cancel, &mut n));
        let mut n = |_: &Progress| {};
        let r = zip_modify(&z, &[], &[], &[d.path().join("s.txt")], "", &mut ctl(&cancel, &mut n));
        assert!(!r.errors.is_empty());
        assert!(r.errors[0].contains("password"), "{:?}", r.errors);
    }

    /// A tarball can be changed the same four ways a zip can — and could not,
    /// until now: "書き換えられるのは zip だけです（tar はまだ）" was what
    /// every one of these asked for got back.
    #[test]
    fn a_tar_can_be_changed_the_way_a_zip_can() {
        for gz in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let src = d.path().join("tree");
            fs::create_dir_all(src.join("sub")).unwrap();
            fs::write(src.join("keep.txt"), b"keep").unwrap();
            fs::write(src.join("drop.txt"), b"drop").unwrap();
            fs::write(src.join("sub/old.txt"), b"old").unwrap();
            let extra = d.path().join("added.txt");
            fs::write(&extra, b"added").unwrap();

            let name = if gz { "bag.tar.gz" } else { "bag.tar" };
            let at = d.path().join(name);
            let stop = AtomicBool::new(false);
            let mut none = |_: &Progress| {};
            let mut ctl = Ctl { cancel: &stop, on_progress: &mut none };
            let made = create_tar(std::slice::from_ref(&src), &at, gz, &mut ctl);
            assert!(made.errors.is_empty(), "{gz}: {:?}", made.errors);

            let mut ctl = Ctl { cancel: &stop, on_progress: &mut none };
            let r = tar_modify(
                &at,
                &["tree/drop.txt".to_string()],
                &[("tree/sub/old.txt".to_string(), "tree/sub/new.txt".to_string())],
                std::slice::from_ref(&extra),
                "tree/",
                &mut ctl,
            );
            assert!(r.errors.is_empty(), "{gz}: {:?}", r.errors);

            let names: Vec<String> = list(&at).unwrap().into_iter().map(|m| m.name).collect();
            assert!(!names.iter().any(|n| n.ends_with("drop.txt")), "{gz}: dropped {names:?}");
            assert!(names.iter().any(|n| n == "tree/sub/new.txt"), "{gz}: renamed {names:?}");
            assert!(!names.iter().any(|n| n == "tree/sub/old.txt"), "{gz}: old name gone {names:?}");
            assert!(names.iter().any(|n| n == "tree/added.txt"), "{gz}: added {names:?}");
            assert!(names.iter().any(|n| n.ends_with("keep.txt")), "{gz}: kept {names:?}");
        }
    }

    /// **A failed rewrite leaves the archive it was given.** The whole file is
    /// rebuilt, so the moment between "old one deleted" and "new one written"
    /// is the moment somebody's only copy could go — there is no such moment.
    #[test]
    fn a_refused_rewrite_leaves_the_original_alone() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("t");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();
        let at = d.path().join("bag.tar");
        let stop = AtomicBool::new(false);
        let mut none = |_: &Progress| {};
        let mut ctl = Ctl { cancel: &stop, on_progress: &mut none };
        create_tar(&[src], &at, false, &mut ctl);
        let before = fs::read(&at).unwrap();

        // Cancelled partway: the answer is an error and an untouched archive.
        let stopped = AtomicBool::new(true);
        let mut ctl = Ctl { cancel: &stopped, on_progress: &mut none };
        let r = tar_modify(&at, &["t/a.txt".to_string()], &[], &[], "", &mut ctl);
        assert!(!r.errors.is_empty(), "it said no");
        assert_eq!(fs::read(&at).unwrap(), before, "and changed nothing");
        assert!(
            !d.path().join("bag.tar.cian-new").exists(),
            "and left no half-written file behind"
        );
    }
}

#[cfg(test)]
mod japanese_names {
    #[test]
    fn a_cp932_name_reads_as_japanese() {
        let raw = encoding_rs::SHIFT_JIS.encode("日本語.txt").0.into_owned();
        assert_eq!(super::member_name("ô·û{îΩ.txt", &raw), "日本語.txt");
        assert_eq!(super::member_name("日本語.txt", "日本語.txt".as_bytes()), "日本語.txt");
        assert_eq!(super::member_name("a/b.txt", b"a/b.txt"), "a/b.txt");
    }
}
