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

use anyhow::{Context, Result};

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

/// What a filtered run came back with.
pub struct Filtered {
    /// The shell's exit code, or -1 when it did not produce one (a signal).
    pub code: i32,
    /// Standard output, in whatever bytes the command wrote. **Not decoded**:
    /// the caller knows the encoding the text came from, and is the one that
    /// has to put it back in the same one.
    pub out: Vec<u8>,
    /// Standard error, lossily decoded, for putting on a status line.
    pub err: String,
}

/// Which flag hands one command line to this shell for a single run.
///
/// Looked up by the program's stem, so an absolute path works
/// (`C:\…\WindowsPowerShell\v1.0\powershell.exe`) and so does a bare name.
/// PowerShell also gets `-NoProfile`: a filter is a one-shot, and somebody's
/// profile printing a banner would land that banner in the middle of the file.
fn one_shot_args(program: &str) -> &'static [&'static str] {
    let stem = std::path::Path::new(program)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match stem.as_str() {
        "powershell" | "pwsh" => &["-NoProfile", "-NonInteractive", "-Command"],
        "cmd" => &["/C"],
        _ => &["-c"],
    }
}

/// Run `line` through `shell`, write `input` to it, and collect what it says.
///
/// This is the engine behind `:%!cmd` — vi's filter, where a stretch of the
/// file is handed to a program and replaced by what comes back.
///
/// **The bytes are the caller's business.** A file read as Shift_JIS is fed
/// to the command as Shift_JIS and its answer is decoded the same way, because
/// the tools on a Windows machine (`sort`, `findstr`) expect the code page
/// they were built for, and re-encoding on the way in would hand them mojibake
/// to sort. So this moves bytes and nothing else.
///
/// `shell` is the program and any fixed arguments it was configured with
/// (`cian_pty::split_command` produces exactly this), and the one-shot flag is
/// added here.
pub fn shell_filter(
    shell: &[String],
    line: &str,
    input: &[u8],
    cwd: &std::path::Path,
) -> Result<Filtered> {
    use std::io::Write;
    let Some((program, pre)) = shell.split_first() else {
        anyhow::bail!("no shell configured");
    };
    let mut cmd = quiet(program);
    cmd.args(pre);
    cmd.args(one_shot_args(program));
    cmd.arg(line);
    cmd.current_dir(cwd);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("could not start {program}"))?;
    // Written on a thread: a command that writes a lot before reading it all
    // (`sort` does) fills the pipe and waits, while this side is waiting to
    // finish writing — and neither moves again.
    let mut stdin = child.stdin.take().expect("piped");
    let owned = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&owned);
        // Dropped here, which is the EOF the command is waiting for.
    });
    let out = child.wait_with_output().context("running the filter")?;
    let _ = writer.join();
    Ok(Filtered {
        code: out.status.code().unwrap_or(-1),
        out: out.stdout,
        err: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}
