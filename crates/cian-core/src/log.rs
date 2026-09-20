//! Opt-in diagnostic logging.
//!
//! cian is a full-screen TUI, so it cannot print diagnostics to the terminal —
//! anything written to stdout would corrupt the display. Instead the lines go
//! to a file: `CIAN_LOG=/path/to/file` at startup, or `:log on` from inside a
//! running cian. When neither has happened (the normal case) every call is a
//! cheap no-op.
//!
//! **It can be turned on without restarting** (2026-09-20). The path used to
//! be resolved once, from the environment, which meant the answer to "take a
//! log of that" was "quit, set a variable, and make it happen again" — and
//! the faults worth logging are the ones that do not happen again.
//!
//! This exists to make rare, hard-to-reproduce faults reportable: panics, PTY
//! spawn failures, and lock poisoning that would otherwise surface only as the
//! UI mysteriously freezing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Where the lines are going, or `None` when logging is off.
///
/// Seeded from `$CIAN_LOG` the first time anything asks, and writable
/// afterwards by [`start`] and [`stop`].
fn state() -> &'static Mutex<Option<PathBuf>> {
    static STATE: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(
            std::env::var_os("CIAN_LOG")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .and_then(|p| resolve(&p).ok()),
        )
    })
}

/// Make `asked` into a path that can actually be appended to.
///
/// A path that cannot be written to becomes one that can. Diagnostics are
/// asked for at exactly the moment something is wrong, by someone who is
/// already annoyed, and a log that silently goes nowhere costs an evening:
/// `%USERPROFILE%\Desktop` does not exist on a Windows machine whose Desktop
/// is OneDrive's, which is most of them, and the obvious place to ask for the
/// file is the desktop. So the directory is created if it can be, and if it
/// still will not take a file the log lands in the temp directory instead —
/// somewhere, and named, beats nowhere and silent.
fn resolve(asked: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = asked.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    if writable(asked) {
        return Ok(asked.to_path_buf());
    }
    // Named after the file that was asked for, so two sessions logging to
    // different places do not land in the same fallback.
    let name = asked.file_name().unwrap_or_else(|| std::ffi::OsStr::new("cian.log"));
    let fallback = std::env::temp_dir().join(name);
    if writable(&fallback) {
        Ok(fallback)
    } else {
        Err(format!("{} cannot be written to, and neither can {}", asked.display(), fallback.display()))
    }
}

/// Can a line be appended here? Asked once, by creating the file.
fn writable(path: &Path) -> bool {
    std::fs::OpenOptions::new().create(true).append(true).open(path).is_ok()
}

/// The file `:log on` picks when no path is named.
///
/// The temp directory rather than the desktop or the working directory: it
/// exists on every machine, it is writable on the managed Windows this is
/// carried to, and it is not somewhere a stray file will confuse anybody.
pub fn default_path() -> PathBuf {
    std::env::temp_dir().join("cian.log")
}

/// Where the diagnostics are going, for a front end that wants to say so.
///
/// `None` when logging is off. The answer may not be the path that was asked
/// for — see [`resolve`] — which is the whole reason this can be asked.
pub fn destination() -> Option<PathBuf> {
    state().lock().ok().and_then(|g| g.clone())
}

/// Whether logging is enabled, so callers can skip building expensive messages.
pub fn enabled() -> bool {
    destination().is_some()
}

/// Start writing to `path` (or [`default_path`]), and say where the lines will
/// actually land — which may not be where they were asked for.
pub fn start(path: Option<&Path>) -> Result<PathBuf, String> {
    let asked = path.map(Path::to_path_buf).unwrap_or_else(default_path);
    let resolved = resolve(&asked)?;
    // **The guard goes out of scope before the first line is written.**
    // `log` takes this same lock to find out where to write, and a
    // `std::sync::Mutex` is not reentrant: logging while still holding it is
    // a thread waiting for itself. It cost a test run to find, because a
    // deadlock does not fail — it sits there looking like a slow build.
    match state().lock() {
        Ok(mut g) => *g = Some(resolved.clone()),
        Err(_) => return Err("the log is held by a thread that panicked".into()),
    }
    log("--- log started ---");
    Ok(resolved)
}

/// Stop writing. Returns where it had been going, or `None` if it was off.
pub fn stop() -> Option<PathBuf> {
    // Written first, for the same reason as in `start`: `log` needs the lock
    // this is about to take, and it has to say goodbye before the door shuts.
    log("--- log stopped ---");
    state().lock().ok().and_then(|mut g| g.take())
}

/// Append one timestamped line to the log file. Never panics and never fails
/// loudly: a broken log path must not take down the file manager.
pub fn log(msg: &str) {
    let Some(path) = destination() else { return };
    // Serialise writes so lines from the UI thread and PTY reader threads do
    // not interleave mid-line.
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK.get_or_init(|| Mutex::new(())).lock();

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{}.{:03}", d.as_secs(), d.subsec_millis()))
        .unwrap_or_else(|_| "?".to_string());

    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{}] {}", stamp, msg);
    }
}
