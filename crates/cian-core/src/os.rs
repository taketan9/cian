//! Handing a file to the desktop: reveal it, open it with something else, show
//! its properties.
//!
//! These lived in `cian-tui` and the windowed engine wrote its own. That is one
//! verb with two implementations, and the copies had already drifted: the
//! engine's `revealos` built Explorer's argument with `Command::arg`, which
//! quotes the whole `/select,PATH` token when the path holds a space —
//! Explorer's own parser rejects that and silently opens Documents instead. A
//! OneDrive-redirected Desktop (`C:\Users\name\OneDrive - Corp\Desktop`) is
//! exactly such a path, so "reveal" quietly showed the wrong folder in the
//! build most likely to be used from one.
//!
//! The terminal build had found and fixed that; the window had not, because it
//! was never the same code. So it is this code now, for both.

use std::path::Path;
#[cfg(feature = "desktop")]
use std::process::Stdio;

use anyhow::Result;

/// Reveal `path` in the OS file manager, selecting it where the platform's
/// manager supports it. Windows Explorer and macOS Finder select the file
/// itself; Linux has no portable "select", so its parent folder is opened.
#[cfg(not(feature = "desktop"))]
pub fn reveal(_path: &Path) -> Result<()> {
    anyhow::bail!("この版に「場所を開く」はありません")
}

#[cfg(feature = "desktop")]
pub fn reveal(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = crate::proc::quiet("open");
        c.arg("-R").arg(path);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        // Explorer wants the PATH quoted, and its own (non-standard) parser
        // rejects the whole `/select,PATH` token being quoted — which is exactly
        // what `Command::arg` does when the path contains a space. Emit the
        // command line verbatim with `raw_arg`, quoting only the path (a Windows
        // path can never contain a `"`, so this is safe). Explorer exits 1 even
        // on success, so we only spawn — never wait on or check its status.
        let mut c = crate::proc::quiet("explorer");
        c.raw_arg(format!("/select,\"{}\"", path.display()));
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let dir = path.parent().unwrap_or(path);
        let mut c = crate::proc::quiet("xdg-open");
        c.arg(dir);
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    Ok(())
}

/// Show the OS "Open with…" application picker for `path`. Only Windows has a
/// portable shell command for this (`OpenAs_RunDLL`); elsewhere it is reported
/// as unsupported so the menu can degrade gracefully.
#[allow(unused_variables)]
pub fn open_with(path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        crate::proc::quiet("rundll32.exe")
            .arg("shell32.dll,OpenAs_RunDLL")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        anyhow::bail!("\u{201c}Open with\u{201d} is only available on Windows");
    }
}

/// Is "Open with…" reachable on this platform at all?
///
/// The menus ask before they draw the row: an entry that can only ever answer
/// "not on this platform" is worse than no entry, and both front ends were
/// deciding that separately.
pub const fn open_with_supported() -> bool {
    cfg!(target_os = "windows")
}

/// Open the OS properties / Get-Info panel for `path`. macOS opens Finder's
/// information window; Windows invokes the shell "Properties" verb via
/// PowerShell (best-effort — the verb name can be localized). Linux has no
/// portable equivalent.
#[allow(unused_variables)]
#[cfg(not(feature = "desktop"))]
pub fn properties(_path: &Path) -> Result<()> {
    anyhow::bail!("この版に「情報を見る」はありません")
}

#[cfg(feature = "desktop")]
pub fn properties(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        // The path is passed as an argv item (not spliced into the script) so a
        // name with quotes or backslashes cannot break the AppleScript.
        crate::proc::quiet("osascript")
            .args([
                "-e", "on run argv",
                "-e", "tell application \"Finder\"",
                "-e", "activate",
                "-e", "open information window of (POSIX file (item 1 of argv))",
                "-e", "end tell",
                "-e", "end run",
            ])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        // Shell.Application's Properties verb. The path arrives as $args[0], not
        // spliced into the script text.
        let script = "$p=$args[0]; \
             (New-Object -ComObject Shell.Application)\
             .Namespace((Split-Path $p))\
             .ParseName((Split-Path $p -Leaf))\
             .InvokeVerb('Properties')";
        crate::proc::quiet("powershell")
            .args(["-NoProfile", "-Command", script])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        // **`path` を1度触る。** Linux では上の2つの塊がまるごと消えるので、
        // 引数はどこからも読まれず `unused_variables` になり、CI の
        // `-D warnings` で落ちる ── **手元（macOS）では絶対に出ない**。
        // `#[allow]` を関数に貼ると他の未使用まで黙るので、ここで捨てる。
        let _ = path;
        anyhow::bail!("Properties is not available on this platform");
    }
}

/// Is the properties panel reachable on this platform?
pub const fn properties_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

/// What the platform calls its file manager, for a menu label.
pub const fn file_manager_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "エクスプローラー"
    } else if cfg!(target_os = "macos") {
        "Finder"
    } else {
        "ファイルマネージャ"
    }
}
