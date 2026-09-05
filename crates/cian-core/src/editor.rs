//! Which external editor to start, and whether it is on `PATH`.
//!
//! **One answer for both front ends.** The terminal build has honoured
//! `cian.set_option("editor", …)` since it had one; the window's `:edit` asked
//! the engine, and the engine read only `$VISUAL` / `$EDITOR` — so the same
//! init.lua opened `code` in the terminal and `vi` in the window, silently.
//! Found on 2026-09-06 by `scripts/configcover.py`, which counts the settings
//! the engine sends that nobody acts on.
//!
//! The decision is here rather than in either front end for the usual reason:
//! a rule written twice is two rules that agree until one of them is edited.

use std::path::PathBuf;

/// The editor command, split into words, with no file argument.
///
/// An explicit editor is trusted as-is and in this order — **init.lua first**,
/// because writing it down is a stronger statement than an environment
/// variable inherited from whatever started cian. Then `$VISUAL`, then
/// `$EDITOR`, then the first of nvim → vim → vi that `found` reports.
///
/// `found` is passed in so the choosing can be tested without a `PATH`.
pub fn pick(
    configured: Option<&str>,
    visual: Option<&str>,
    editor_env: Option<&str>,
    found: impl Fn(&str) -> bool,
) -> Option<Vec<String>> {
    if let Some(cmd) = configured.or(visual).or(editor_env) {
        let words: Vec<String> = cmd.split_whitespace().map(|s| s.to_string()).collect();
        if !words.is_empty() {
            return Some(words);
        }
    }
    ["nvim", "vim", "vi"].into_iter().find(|n| found(n)).map(|n| vec![n.to_string()])
}

/// Is `name` an executable on `PATH`? On Windows the usual executable
/// extensions are tried too.
pub fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    let exts: &[&str] = if cfg!(windows) { &["", ".exe", ".cmd", ".bat"] } else { &[""] };
    std::env::split_paths(&path).any(|dir| {
        exts.iter().any(|ext| {
            let cand: PathBuf = dir.join(format!("{name}{ext}"));
            is_executable(&cand)
        })
    })
}

#[cfg(unix)]
fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &std::path::Path) -> bool {
    p.is_file()
}

/// The editor for these options, reading the environment for the two
/// variables. The half that needs a real process; [`pick`] is the judgement.
pub fn resolve(configured: Option<&str>) -> Option<Vec<String>> {
    let visual = std::env::var("VISUAL").ok().filter(|s| !s.trim().is_empty());
    let editor = std::env::var("EDITOR").ok().filter(|s| !s.trim().is_empty());
    pick(configured, visual.as_deref(), editor.as_deref(), on_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_down_wins_over_the_environment() {
        // init.lua first — and kept whole, arguments and all.
        let cmd = pick(Some("code -w"), Some("hx"), Some("nano"), |_| false);
        assert_eq!(cmd, Some(vec!["code".into(), "-w".into()]));
        assert_eq!(pick(None, Some("hx"), Some("nano"), |_| false), Some(vec!["hx".into()]));
        assert_eq!(pick(None, None, Some("nano"), |_| false), Some(vec!["nano".into()]));
    }

    #[test]
    fn with_nothing_said_it_is_the_first_one_on_the_path() {
        assert_eq!(pick(None, None, None, |n| n == "vim" || n == "vi"), Some(vec!["vim".into()]));
        assert_eq!(pick(None, None, None, |n| n == "nvim" || n == "vim"), Some(vec!["nvim".into()]));
        assert_eq!(pick(None, None, None, |n| n == "vi"), Some(vec!["vi".into()]));
        // Nothing at all, and nothing is claimed.
        assert_eq!(pick(None, None, None, |_| false), None);
    }

    #[test]
    fn an_empty_setting_is_not_a_setting() {
        // `editor = ""` in init.lua must not become an empty command line.
        assert_eq!(pick(Some("   "), None, None, |n| n == "vi"), Some(vec!["vi".into()]));
    }
}
