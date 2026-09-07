//! The OS clipboard, when what is on it is *files* rather than text.
//!
//! Three platforms, three unrelated mechanisms: an AppleScript, a
//! `wl-copy`/`xclip` uri-list, and a PowerShell `Set-Clipboard`. Down here
//! because both front ends need it and a three-way split copied by hand is
//! one that ends up with a platform quietly missing — which is exactly how
//! `os_open` came to live here too.

use std::path::PathBuf;

use anyhow::Result;
use std::process::Stdio;

/// Files currently on the OS clipboard, e.g. copied in Explorer or Finder.
///
/// Every candidate is checked against the filesystem before being returned:
/// the platform queries happily hand back plain clipboard *text* interpreted
/// as a path (copying the word "hello" yields `/hello` on macOS), and acting
/// on that would be at best a confusing error.
pub fn files() -> Vec<PathBuf> {
    keep_existing(files_raw())
}

/// Drop anything that is not actually a file or directory on disk.
pub fn keep_existing(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.into_iter().filter(|p| p.exists()).collect()
}

#[cfg(target_os = "macos")]
fn files_raw() -> Vec<PathBuf> {
    // `the clipboard as «class furl»` only ever yields one file; coercing to a
    // list handles both the single- and multi-file cases.
    const SCRIPT: &str = r#"set out to ""
try
  set items_ to the clipboard as list
  repeat with i in items_
    set out to out & POSIX path of i & linefeed
  end repeat
end try
return out"#;
    let out = match crate::proc::quiet("osascript").args(["-e", SCRIPT]).output() {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out)
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(target_os = "macos")]
pub fn put_files(paths: &[PathBuf]) -> Result<()> {
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let parts: Vec<String> = paths
        .iter()
        .map(|p| format!("POSIX file \"{}\"", escape(&p.display().to_string())))
        .collect();
    let script = if parts.len() == 1 {
        format!("set the clipboard to {}", parts[0])
    } else {
        format!("set the clipboard to {{{}}}", parts.join(", "))
    };
    let status = crate::proc::quiet("osascript")
        .args(["-e", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        anyhow::bail!("osascript exited with status {}", status);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn files_raw() -> Vec<PathBuf> {
    let read = |cmd: &str, args: &[&str]| -> Option<String> {
        let o = crate::proc::quiet(cmd).args(args).output().ok()?;
        o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    // Wayland first, then X11, mirroring the write side.
    let text = read("wl-paste", &["--type", "text/uri-list"])
        .or_else(|| read("xclip", &["-selection", "clipboard", "-t", "text/uri-list", "-o"]))
        .unwrap_or_default();
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| PathBuf::from(percent_decode(l.strip_prefix("file://").unwrap_or(l))))
        .collect()
}

/// Turn `%20`-style escapes in a `file://` URI back into bytes.
#[cfg(target_os = "linux")]
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
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

#[cfg(target_os = "linux")]
pub fn put_files(paths: &[PathBuf]) -> Result<()> {
    use std::io::Write;
    let uris = paths
        .iter()
        .map(|p| format!("file://{}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    // try wl-copy first (wayland), then xclip
    if let Ok(mut child) = crate::proc::quiet("wl-copy")
        .args(["--type", "text/uri-list"])
        .stdin(Stdio::piped())
        .spawn()
    {
        if let Some(s) = child.stdin.as_mut() {
            s.write_all(uris.as_bytes())?;
        }
        if child.wait()?.success() {
            return Ok(());
        }
    }
    let mut child = crate::proc::quiet("xclip")
        .args(["-selection", "clipboard", "-t", "text/uri-list"])
        .stdin(Stdio::piped())
        .spawn()?;
    if let Some(s) = child.stdin.as_mut() {
        s.write_all(uris.as_bytes())?;
    }
    if !child.wait()?.success() {
        anyhow::bail!("xclip failed");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn files_raw() -> Vec<PathBuf> {
    let out = crate::proc::quiet("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-Clipboard -Format FileDropList | ForEach-Object { $_.FullName }",
        ])
        .output();
    let out = match out {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out)
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(target_os = "windows")]
pub fn put_files(paths: &[PathBuf]) -> Result<()> {
    // Was a stub that always failed, so Shift+P did nothing on the platform
    // where Explorer interop matters most.
    if paths.is_empty() {
        return Ok(());
    }
    // Single-quoted PowerShell literals: the only escape needed is a doubled
    // quote, which leaves spaces and backslashes alone.
    let list = paths
        .iter()
        .map(|p| format!("'{}'", p.display().to_string().replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    let status = crate::proc::quiet("powershell")
        .args(["-NoProfile", "-Command", &format!("Set-Clipboard -Path {}", list)])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        anyhow::bail!("Set-Clipboard exited with status {}", status);
    }
    Ok(())
}

