//! Running another program without a window flashing up.
//!
//! On Windows every process belongs to a console or to none, and a process
//! with none that starts a *console* program gets one made for it — a black
//! window, on screen, in front of whatever the user was doing. A terminal
//! build never sees this: it has a console already, and its children inherit
//! it. A windowed build has none by design, so every `git status`, every
//! availability probe, every `powershell` one-liner flashed a window of its
//! own.
//!
//! That is what was reported as "two windows open at startup, one of them
//! python.exe": the AI probe, running before the first frame.
//!
//! `CREATE_NO_WINDOW` says "run it, but do not make a console for it". It is
//! the right answer for everything cian runs *for itself* — anything whose
//! output cian reads rather than shows. It is the wrong answer for the one
//! thing cian runs *for the user* on a terminal they are looking at: the
//! external editor, which needs the console it was launched from. That one
//! keeps using `Command::new` directly, and says so where it does.
//!
//! Everywhere but Windows this is exactly `Command::new`.

use std::ffi::OsStr;
use std::process::Command;

/// Start building a command that will not open a console window of its own.
pub fn quiet(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    hide(&mut cmd);
    cmd
}

/// Add the "no console, please" flag to a command built elsewhere.
pub fn hide(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW`, from the Windows process-creation flags.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Hand something to whatever the desktop opens it with.
///
/// A path and a URL were two functions, identical to the byte apart from the
/// type of the one argument — and both `&Path` and `&str` are `AsRef<OsStr>`,
/// which is all `Command::arg` ever wanted.
///
/// Down here rather than in a front end because there are two of them now, and
/// the three-way `open` / `xdg-open` / `cmd /C start ""` split is the sort of
/// thing that gets copied with one platform quietly missing.
pub fn open_with_desktop(target: impl AsRef<OsStr>) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = quiet("open");
    #[cfg(target_os = "linux")]
    let mut cmd = quiet("xdg-open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        // The empty argument is the window title `start` insists on, and
        // without it a quoted path is taken as the title and nothing opens.
        let mut c = quiet("cmd");
        c.arg("/C").arg("start").arg("");
        c
    };
    cmd.arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}
