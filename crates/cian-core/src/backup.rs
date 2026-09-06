//! Reading and writing past an ACL with the administrator's own privileges,
//! **without changing anybody's permissions**.
//!
//! ## What this is for
//!
//! On a shared machine — an AVD session host with other people signed in, a
//! file server reached over UNC — a directory's ACL routinely does not name
//! you, and being in Administrators does not by itself get you in: membership
//! is not access, the ACL is. What an administrator does hold is
//! `SeBackupPrivilege` and `SeRestorePrivilege`, which are carried in the token
//! **present but disabled** and, once enabled, let a handle be opened past the
//! ACL entirely.
//!
//! This is what Explorer's "You'll need to provide administrator permission"
//! dialog is really asking about, and there are two very different ways to
//! answer it:
//!
//! - **backup/restore privileges** — read and write past the ACL, changing
//!   nothing. Asked again next time, because nothing was left behind.
//! - **editing the DACL / taking ownership** — grant Administrators an ACE and
//!   proceed. Permanent, and on a machine other people are signed into it is a
//!   change to *their* access, not only yours.
//!
//! **cian does the first.** Which one Explorer took can be read off its own
//! behaviour: if it asks every time, and the folder's permissions are the same
//! afterwards, nothing was written — that is the first. (Verified this way for
//! the case this module was built for.)
//!
//! ## Why robocopy rather than `CreateFileW`
//!
//! `FILE_FLAG_BACKUP_SEMANTICS` on a handle is the in-process version of this,
//! and it is a poor trade here. It needs `windows-sys` where cian has so far
//! needed no Windows bindings at all; it cannot create a directory (the Win32
//! `CreateDirectoryW` takes no backup flag, so a protected destination tree
//! wants `NtCreateFile`); and none of it can be type-checked on the machine
//! this is written on. `robocopy /B` is the documented, decades-old answer to
//! exactly this question, ships in Windows, handles trees, long paths and UNC,
//! and is already how cian performs its elevated retry.
//!
//! Arguments go through `Command` as an argv array rather than through a
//! PowerShell script, so nothing here has to be quoted or escaped — see
//! [`robocopy_args`] for the one exception that still bites.
//!
//! ## What it cannot do
//!
//! **A privilege is local to the machine that holds it.** Against
//! `\\other-host\share`, the far end decides, from your identity there and its
//! own ACL — so this helps across UNC only when you are an administrator on
//! *that* machine too. The backup intent does travel over SMB (it is a create
//! option, and the server evaluates its own token), which is why `robocopy /B`
//! is the tool administrators reach for against a UNC path; but cian cannot
//! promise the far end will honour it.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::elevate::CopyItem;
use crate::ops::{Conflict, OpReport};

/// robocopy's exit codes are a bitmask: 0–7 are degrees of success (files
/// copied, extras found, mismatches), 8 and above are failures. Anything else
/// would report a perfectly good copy as an error.
#[cfg(windows)]
const ROBOCOPY_FAILURE: i32 = 8;

/// Is this the mode that could get past the wall?
pub fn is_available() -> bool {
    cfg!(windows)
}

/// Strip trailing separators from a path being handed to robocopy.
///
/// **The one escaping problem left.** Rust quotes an argument containing
/// spaces, and a quoted argument ending in a backslash — `"C:\share\"` — has
/// its closing quote escaped by it, so robocopy reads the rest of the command
/// line as part of the path. A directory means the same thing without the
/// trailing separator, so it comes off. A bare root (`C:\`, `\\host\share\`)
/// keeps it, because there it is not a separator but part of the name.
fn trim_for_robocopy(p: &Path) -> String {
    let s = p.display().to_string();
    let trimmed = s.trim_end_matches(['\\', '/']);
    // `C:` alone is not the root of C: — it is "the current directory on C:",
    // which is a different place. A UNC share root has nothing under it to
    // name either.
    let looks_like_a_root = trimmed.is_empty()
        || trimmed.ends_with(':')
        || trimmed.strip_prefix(r"\\").is_some_and(|rest| rest.matches('\\').count() < 2);
    if looks_like_a_root {
        s
    } else {
        trimmed.to_string()
    }
}

/// The robocopy command line for one item.
///
/// A pure function so the shape can be pinned from any machine — the thing it
/// describes only runs on Windows, but getting the flags wrong is not a
/// Windows-only mistake.
///
/// `/B` is the whole point: backup mode, which is the privileges being used.
/// `/COPY:DAT` carries data, attributes and timestamps and **deliberately not
/// `S` or `O`** — copying the source's ACL or owner onto the destination would
/// be the permission change this module exists to avoid.
pub fn robocopy_args(item: &CopyItem, moving: bool, on_conflict: Conflict) -> Result<Vec<String>> {
    let name = item
        .src
        .file_name()
        .ok_or_else(|| anyhow!("{} has no name to copy", item.src.display()))?
        .to_string_lossy()
        .into_owned();

    // robocopy copies *directories*; a single file is named as the third
    // argument, with its own folder as the source. That is the documented way
    // to copy one file with it, and it is why this is not two tools.
    let is_dir = item.src.is_dir();
    let mut args: Vec<String> = if is_dir {
        vec![
            trim_for_robocopy(&item.src),
            trim_for_robocopy(&item.dest_dir.join(&name)),
        ]
    } else {
        let parent = item
            .src
            .parent()
            .ok_or_else(|| anyhow!("{} has no folder", item.src.display()))?;
        vec![
            trim_for_robocopy(parent),
            trim_for_robocopy(&item.dest_dir),
            name,
        ]
    };

    if is_dir {
        args.push("/E".into()); // subdirectories, including empty ones
    }
    args.push("/B".into()); // backup mode — the privileges
    args.push("/COPY:DAT".into());
    // **The answer already given, kept.** The retry has to mean the same thing
    // the refused copy meant: somebody chose "skip what is already there" at a
    // confirmation that now says how many that is, and a retry that quietly
    // overwrote them instead would be the one outcome the confirmation exists
    // to prevent. robocopy's default is to copy anything that differs, so
    // "skip" has to be spelled out — the three exclusions leave only the files
    // with nothing at the destination to collide with.
    if on_conflict == Conflict::Skip {
        for flag in ["/XC", "/XN", "/XO"] {
            args.push(flag.into());
        }
    }
    if moving {
        // `/MOVE` takes directories with it; `/MOV` is files only, which is
        // what a single-file copy is.
        args.push(if is_dir { "/MOVE".into() } else { "/MOV".into() });
    }
    // One retry, one second apart. The default is a million retries thirty
    // seconds apart, which is not a copy any person is waiting for.
    args.push("/R:1".into());
    args.push("/W:1".into());
    // Quiet: no per-file list, no header, no summary, no progress percentages.
    // cian reports the outcome itself, and robocopy's progress writes carriage
    // returns that would arrive as junk.
    for flag in ["/NFL", "/NDL", "/NJH", "/NJS", "/NP"] {
        args.push(flag.into());
    }
    Ok(args)
}

/// Copy (or move) `items` using the administrator's backup/restore privileges.
///
/// Runs in **this** process — there is no prompt, because there is nothing to
/// elevate to: the privileges are already in the token of a cian started as
/// administrator, and robocopy enables them itself. A cian that is *not*
/// running as administrator has no such privileges, and this reports that
/// rather than appearing to work.
pub fn backup_copy(items: &[CopyItem], move_after: bool, on_conflict: Conflict) -> OpReport {
    let mut report = OpReport::default();
    if items.is_empty() {
        return report;
    }
    if !is_available() {
        report.note_error(
            "backup mode is a Windows privilege; there is no equivalent here".to_string(),
        );
        return report;
    }
    for item in items {
        match run_one(item, move_after, on_conflict) {
            Ok(()) => report.ok += 1,
            Err(e) => report.note_error(format!("{}: {}", item.src.display(), e)),
        }
    }
    report.note = Some(if report.errors.is_empty() {
        "administrator's backup mode — no permissions were changed".to_string()
    } else {
        "backup mode could not get past it either".to_string()
    });
    report
}

#[cfg(windows)]
fn run_one(item: &CopyItem, moving: bool, on_conflict: Conflict) -> Result<()> {
    use anyhow::Context;
    let args = robocopy_args(item, moving, on_conflict)?;
    let out = crate::proc::quiet("robocopy")
        .args(&args)
        .output()
        .context("run robocopy in backup mode")?;
    let code = out.status.code().unwrap_or(ROBOCOPY_FAILURE);
    if code < ROBOCOPY_FAILURE {
        return Ok(());
    }
    // robocopy says why on stdout, not stderr, and the last non-empty line is
    // the one naming the file it stopped on. Reported verbatim: a translated
    // guess about somebody else's ACL is worse than the machine's own words.
    let said = String::from_utf8_lossy(&out.stdout);
    let last = said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    Err(anyhow!(
        "robocopy /B exited {code}{}",
        if last.is_empty() { String::new() } else { format!(" — {last}") }
    ))
}

#[cfg(not(windows))]
fn run_one(_item: &CopyItem, _moving: bool, _on_conflict: Conflict) -> Result<()> {
    Err(anyhow!("backup mode is a Windows privilege"))
}

/// The items a failed transfer would retry, from the sources and destination
/// the confirmation already holds.
pub fn items_for(targets: &[PathBuf], dest: &Path) -> Vec<CopyItem> {
    targets
        .iter()
        .map(|src| CopyItem { src: src.clone(), dest_dir: dest.to_path_buf() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(src: &str, dest: &str) -> CopyItem {
        CopyItem { src: PathBuf::from(src), dest_dir: PathBuf::from(dest) }
    }

    /// A trailing backslash on a quoted argument escapes the closing quote, and
    /// robocopy then reads the rest of the line as part of the path. Every
    /// destination cian hands over is a directory, so this is not a rare shape.
    #[test]
    fn a_trailing_separator_comes_off() {
        assert_eq!(trim_for_robocopy(Path::new(r"C:\share\proj\")), r"C:\share\proj");
        assert_eq!(trim_for_robocopy(Path::new(r"C:\share\proj")), r"C:\share\proj");
    }

    /// …but not from a root, where it is part of the name. `C:` on its own
    /// means "wherever I am on C:", which is somewhere else entirely.
    #[test]
    fn but_not_from_a_root() {
        assert_eq!(trim_for_robocopy(Path::new(r"C:\")), r"C:\");
        assert_eq!(trim_for_robocopy(Path::new(r"\\host\share\")), r"\\host\share\");
    }

    /// **Real paths, not Windows-shaped strings.** `parent()` and `file_name()`
    /// read `\` as a separator only on Windows, so `C:\a\f.txt` is one
    /// nameless component on the machine this is written on and every
    /// assertion about its folder passes or fails for the wrong reason. A temp
    /// directory is the same shape on both.
    fn a_file() -> (tempfile::TempDir, CopyItem) {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("report.xlsx");
        std::fs::write(&src, b"x").unwrap();
        let item = CopyItem { src, dest_dir: d.path().join("dest") };
        (d, item)
    }

    /// A single file is named as robocopy's third argument, with its folder as
    /// the source — the documented way to copy one file with a directory tool.
    #[test]
    fn a_file_is_named_beside_its_folder() {
        let (d, it) = a_file();
        let args = robocopy_args(&it, false, Conflict::Overwrite).unwrap();
        assert_eq!(args[0], trim_for_robocopy(d.path()), "the folder it is in");
        assert_eq!(args[1], trim_for_robocopy(&d.path().join("dest")));
        assert_eq!(args[2], "report.xlsx");
        assert!(!args.contains(&"/E".to_string()), "a file has no subtree");
    }

    /// Backup mode, and timestamps — the two promises this makes. And **not**
    /// `S` or `O`: copying the source's ACL or owner onto the destination is
    /// the permission change the whole module exists to avoid.
    #[test]
    fn it_asks_for_backup_mode_and_carries_the_dates() {
        let (_d, it) = a_file();
        let args = robocopy_args(&it, false, Conflict::Overwrite).unwrap();
        assert!(args.contains(&"/B".to_string()), "backup mode is the point");
        // Read the letters *after* the colon. Testing the whole flag catches
        // the `O` in `/COPY:` itself, which is how the first version of this
        // failed a copy spec that was already right.
        let what: Vec<&str> =
            args.iter().filter_map(|a| a.strip_prefix("/COPY:")).collect();
        assert_eq!(what, ["DAT"], "data, attributes, timestamps — and nothing else");
        assert!(
            !what.iter().any(|w| w.contains('S') || w.contains('O')),
            "copying the ACL or the owner would change permissions: {args:?}",
        );
    }

    #[test]
    fn a_directory_takes_its_subtree_and_lands_under_its_own_name() {
        let args = robocopy_args(&item(env!("CARGO_MANIFEST_DIR"), r"D:\b"), false, Conflict::Overwrite).unwrap();
        assert!(args.contains(&"/E".to_string()));
        assert!(args[1].ends_with("cian-core"), "the tree lands under its own name: {}", args[1]);
    }

    /// **The retry means what the confirmation meant.** Somebody chose
    /// "y — skip what is already there" at a dialog that now says how many
    /// that is; robocopy's default is to copy anything that differs, so
    /// without these three the retry would overwrite exactly the files the
    /// answer said to leave alone.
    #[test]
    fn skipping_is_carried_into_the_retry() {
        let (_d, it) = a_file();
        let skip = robocopy_args(&it, false, Conflict::Skip).unwrap();
        for flag in ["/XC", "/XN", "/XO"] {
            assert!(skip.contains(&flag.to_string()), "{flag} missing: {skip:?}");
        }
        let over = robocopy_args(&it, false, Conflict::Overwrite).unwrap();
        assert!(
            !over.iter().any(|a| a.starts_with("/X")),
            "overwrite excludes nothing: {over:?}",
        );
    }

    /// A move is robocopy's own, so the source is not left behind for a
    /// separate delete that might not have the privilege either.
    #[test]
    fn moving_says_so() {
        let (_d, it) = a_file();
        let file = robocopy_args(&it, true, Conflict::Overwrite).unwrap();
        assert!(file.contains(&"/MOV".to_string()), "files only: {file:?}");
        let dir = robocopy_args(&item(env!("CARGO_MANIFEST_DIR"), r"D:\b"), true, Conflict::Overwrite).unwrap();
        assert!(dir.contains(&"/MOVE".to_string()), "directories too: {dir:?}");
    }

    /// Off Windows it says so rather than reporting a copy that never ran.
    #[test]
    #[cfg(not(windows))]
    fn elsewhere_it_refuses_rather_than_pretending() {
        let r = backup_copy(&[item(r"C:\a\f.txt", r"D:\b")], false, Conflict::Skip);
        assert_eq!(r.ok, 0);
        assert!(!r.errors.is_empty(), "a copy that did not happen is not a success");
    }
}
