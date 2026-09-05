//! cian-lua: mlua-based configuration host.
//!
//! Loads `~/.config/cian/init.lua` (overridable with `CIAN_CONFIG_DIR`) and
//! exposes a small WezTerm-flavoured `cian` API to user scripts:
//!
//! ```lua
//! cian.set_theme "solarized-light"   -- a named preset, or
//! cian.set_theme({ accent = "#00d7d7", mark_fg = "yellow" })
//! cian.set_keymap("x", "delete")          -- bind key `x` to the delete action
//! cian.set_keymap("alt+g", "grep_recursive") -- …with a modifier, for the
//!                                            -- Ctrl combinations a terminal
//!                                            -- may keep to itself
//! cian.on_open("md", function(path)        -- extension-dispatch execution
//!   cian.spawn({ "open", "-a", "Typora", path })
//! end)
//! ```
//!
//! Loading never fails the program: any syntax/runtime error is captured in
//! [`Config::errors`] and the UI falls back to defaults for whatever could not
//! be applied. This crate stays UI-agnostic — colors are passed through as raw
//! strings and parsed by the UI layer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;

use mlua::{Function, FromLua, Lua, Table, Value};

pub mod count;
pub mod macro_script;
pub mod macros;
pub mod shortcuts;

/// A color spec exactly as the user wrote it: `"#rrggbb"`, a named color
/// (`"cyan"`, `"yellow"`), or `"r,g,b"`. Parsed by the UI layer.
pub type ColorSpec = String;

/// User-supplied color overrides. `None` means "keep the built-in default".
#[derive(Debug, Clone, Default)]
pub struct Theme {
    /// A named palette to start from — e.g. "solarized-light". Unset uses the
    /// built-in dark theme. Per-key overrides below still apply on top.
    pub preset: Option<String>,
    pub accent: Option<ColorSpec>,
    pub status_bg: Option<ColorSpec>,
    pub selected_bg: Option<ColorSpec>,
    pub visual_bg: Option<ColorSpec>,
    pub mark_fg: Option<ColorSpec>,
}

/// Behavioural switches. `None` means "keep the built-in default".
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Ceiling on transfer speed to and from a server: `"2M"`, `"500k"`,
    /// `"1.5MB/s"`. Unset is as fast as the link allows.
    pub transfer_limit: Option<String>,
    /// After an SFTP upload/download, re-read the remote file and compare its
    /// checksum with the local one, warning on a mismatch. Off by default: it is
    /// worth the second read of the data only when integrity matters more than
    /// bandwidth. Verification needs SFTP (the SCP fallback cannot be re-read).
    pub verify_transfers: Option<bool>,
    /// Program to run in the embedded shell panel (e.g. "powershell.exe").
    pub shell: Option<String>,
    /// Duration of split/zoom/close transitions in milliseconds. `0` disables
    /// animation entirely.
    pub animation_ms: Option<u64>,
    /// Which view the window opens in: `"classic"`, `"details"` or `"icons"`.
    ///
    /// Only the windowed build has views to choose between; the terminal build
    /// ignores it. Defaults to `"classic"` — the two panes, because that is
    /// what cian is; the other two are one pane at a time and give up the
    /// arrangement the whole program is built around. Anyone who wants
    /// Explorer's list or a wall of tiles says so here, or on `T`.
    pub view: Option<String>,
    /// Show the contextual key-hint bar above the status line.
    pub key_hints: Option<bool>,
    /// Which grammar the built-in editor answers to: `"vim"` or `"notepad"`.
    ///
    /// Defaults to vim — cian's editor is vi-shaped and so, usually, is the
    /// person who installed it. `"notepad"` is for handing the same binary to
    /// someone who has never used vi: arrows move, Shift and an arrow select,
    /// and every letter is a letter. `T` flips it live either way; this is only
    /// what a machine starts with.
    pub edit_style: Option<String>,
    /// Border corners: "rounded", "plain", or unset for per-terminal defaults.
    pub borders: Option<String>,
    /// Start with dotfiles visible. Defaults to true.
    pub show_hidden: Option<bool>,
    /// Use Nerd Font glyphs (file-type icons, the branch/disk symbols). Default
    /// true; set false on a terminal without a Nerd Font so nothing mojibakes.
    pub nerd_fonts: Option<bool>,
    /// How many columns a tab reaches. Defaults to 4. Worth raising to 8 for
    /// tab-separated data, which lines up only when every field is narrower
    /// than the stop.
    pub tab_width: Option<usize>,
    /// Directory both panes open in when cian is started with no path
    /// argument. Unset falls back to the Desktop, then the working directory.
    pub home: Option<String>,
    /// Interface language for the key manual and help text: "ja" (default) or
    /// "en".
    pub lang: Option<String>,
    /// Language for the key manual (`?`) and the right-click context menu only,
    /// overriding `lang` for those two surfaces. Unset = follow `lang`.
    pub menu_lang: Option<String>,
    /// External editor for `E` in the viewer / `:edit`. A command line, e.g.
    /// "nvim" or "code -w". Unset falls back to $VISUAL/$EDITOR, then nvim →
    /// vim → vi on PATH.
    pub editor: Option<String>,
    /// Ring the terminal bell and post a desktop notification (OSC 9) when a
    /// long-running job — a copy/move/delete, an archive, a transfer — finishes
    /// while cian isn't in the foreground's attention. Defaults to true. The
    /// job has to have run at least `notify_min_secs` for it to fire.
    pub notify: Option<bool>,
    /// Start with the cursor-follow preview on (the shell panel shows the
    /// file under the cursor). Defaults to true; :preview / T toggles live.
    pub preview: Option<bool>,
    /// Let sweeps (grep, :count, :hash, :dupes, :preview) read cloud
    /// placeholder files, downloading them. Off by default — see
    /// `cian_core::cloud`.
    pub read_cloud_files: Option<bool>,
    /// How many seconds a job must run before a finish notification fires.
    /// Defaults to 5, so quick operations stay silent.
    pub notify_min_secs: Option<u64>,
    /// Extensions the cursor-follow preview leaves alone, without the dot and
    /// case-insensitively: `{ "vsix", "iso" }`.
    ///
    /// For the kinds where showing it costs more than it is worth — a `.vsix`
    /// is a zip of an editor extension, and unpacking one to list it stalls
    /// the panel for a file nobody wanted to look inside. `F3` still opens
    /// them: this is about what happens *without* being asked.
    pub preview_skip: Vec<String>,
}

/// A login on a host.
///
/// `password` is stored exactly as written in `init.lua`. That is a plaintext
/// secret in a file, with everything that implies — it is opt-in, per user, and
/// `password_cmd` exists so the value can come from a credential store instead
/// without changing anything else. cian never logs or displays either.
#[derive(Clone)]
pub struct SshUser {
    pub name: String,
    pub password: Option<String>,
    /// Shell command whose stdout is the password (trailing newline trimmed).
    pub password_cmd: Option<String>,
    /// A private key to log in with — `key = "~/.ssh/id_ed25519"`.
    ///
    /// **cian could only ever offer a password.** That is not how most people
    /// reach most servers, and on a key-only host there was no way in at all.
    /// `~` is expanded here rather than at the call site, because a path in a
    /// config file is written the way a person writes one.
    pub key: Option<String>,
    /// The passphrase on that key, when it has one.
    pub key_pass: Option<String>,
}

impl SshUser {
    pub fn plain(name: impl Into<String>) -> Self {
        Self { name: name.into(), password: None, password_cmd: None, key: None, key_pass: None }
    }

    pub fn has_secret(&self) -> bool {
        self.password.is_some() || self.password_cmd.is_some() || self.key.is_some()
    }

    /// The key file, with `~` expanded. `None` when none is configured.
    pub fn key_path(&self) -> Option<std::path::PathBuf> {
        let raw = self.key.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        Some(match raw.strip_prefix("~/") {
            Some(rest) => match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
                Some(home) => std::path::PathBuf::from(home).join(rest),
                None => std::path::PathBuf::from(raw),
            },
            None => std::path::PathBuf::from(raw),
        })
    }

    /// Resolve the secret to send, running `password_cmd` if that is the source.
    pub fn secret(&self) -> Option<String> {
        if let Some(p) = &self.password {
            return Some(p.clone());
        }
        let cmd = self.password_cmd.as_ref()?;
        let out = if cfg!(windows) {
            cian_core::proc::quiet("cmd").args(["/C", cmd]).output().ok()?
        } else {
            cian_core::proc::quiet("sh").arg("-c").arg(cmd).output().ok()?
        };
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim_end_matches(['\r', '\n']).to_string())
    }
}

/// Never let a secret reach a log line or a panic message.
impl std::fmt::Debug for SshUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshUser")
            .field("name", &self.name)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("password_cmd", &self.password_cmd.as_ref().map(|_| "<redacted>"))
            // The *path* is safe to print and is the thing you want to see
            // when a key is refused; the passphrase never is.
            .field("key", &self.key)
            .field("key_pass", &self.key_pass.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// One SSH target: a host plus the users worth offering for it.
#[derive(Debug, Clone)]
pub struct SshHost {
    /// Label shown in the picker.
    pub name: String,
    /// Hostname or address passed to `ssh`.
    pub host: String,
    pub users: Vec<SshUser>,
    pub port: Option<u16>,
    /// Free-text facts about this server (OS, installed middleware, versions),
    /// fed to the AI as context when a shell is connected here. `None` = none.
    pub notes: Option<String>,
}

/// One command snippet from `cian.snippets{...}`: a labelled line of shell
/// text sent to the active shell pane on demand.
#[derive(Debug, Clone)]
pub struct Snippet {
    /// Label shown in the picker.
    pub name: String,
    /// The shell text to send.
    pub cmd: String,
    /// Append a newline so it runs immediately. Defaults to `true` — a snippet
    /// is a one-action launcher; set `enter = false` to type it for review.
    pub enter: bool,
    /// Ask before sending (for destructive commands). Defaults to `false`.
    pub confirm: bool,
}

/// AI settings from `cian.ai{...}`. Presence enables the (optional) AI
/// features; the TUI still verifies the helper actually works before showing
/// them. Fields mirror crmaine's backend so the same Azure endpoint is reached.
#[derive(Debug, Clone)]
pub struct AiOptions {
    pub python: String,
    pub endpoint: String,
    pub model: String,
    pub api_version: String,
    pub auth_mode: String,
    pub api_key: String,
    pub api_base_url: String,
}

impl Default for AiOptions {
    fn default() -> Self {
        Self {
            python: "python".into(),
            // No endpoint by default: see `cian_ai::AiConfig`. A site's own
            // gateway belongs in that site's init.lua, not in a public
            // repository.
            endpoint: String::new(),
            model: "gpt-5-mini".into(),
            api_version: "2025-04-01-preview".into(),
            auth_mode: "broker".into(),
            api_key: String::new(),
            api_base_url: String::new(),
        }
    }
}

/// Settings from `cian.ime{...}`: the commands cian runs to turn the system's
/// input method off and on again.
///
/// cian cannot see a key an IME is still composing — the terminal holds it —
/// so the only way single-key commands can work with Japanese input on is for
/// the input method to be off while cian is being *driven*, and on again the
/// moment it is being *typed into*. Doing that needs a helper that can switch
/// the input source: `macism` or `im-select` on macOS, `zenhan` or
/// `im-select` on Windows. cian runs whatever command is given here.
#[derive(Debug, Clone, Default)]
pub struct ImeOptions {
    /// A program that prints the current input source when run with no
    /// argument, and switches to the one named when given it. `macism`,
    /// `im-select` and cian's own `cian-ime` all have this shape, so this one
    /// field is usually the whole configuration.
    pub helper: Option<String>,
    /// Read the current input source (prints its id). Defaults to `helper`.
    pub query: Option<String>,
    /// Switch to an input source; `{}` is replaced by its id. Defaults to
    /// `helper {}`.
    pub set: Option<String>,
    /// The id of the input source that means "no IME" — the one cian switches
    /// to whenever a keystroke is a command rather than text.
    pub off: Option<String>,
    /// Put back the input source the user was last typing with when cian
    /// exits. Defaults to true.
    pub restore: bool,
}

impl ImeOptions {
    /// The command that reads the current input source.
    pub fn query_cmd(&self) -> Option<String> {
        self.query.clone().or_else(|| self.helper.clone())
    }

    /// The command that switches to `id`.
    pub fn set_cmd(&self, id: &str) -> Option<String> {
        match (&self.set, &self.helper) {
            (Some(t), _) if t.contains("{}") => Some(t.replace("{}", id)),
            (Some(t), _) => Some(format!("{t} {id}")),
            (None, Some(h)) => Some(format!("{h} {id}")),
            (None, None) => None,
        }
    }
}

/// Settings from `cian.font{...}`: how to make the terminal's font bigger and
/// smaller.
///
/// A program inside a terminal cannot resize that terminal's font — the font
/// belongs to the emulator, and there is no portable escape sequence for it.
/// What cian can do is run the command *your* terminal offers and remember
/// the level, so `Ctrl+-` / `Ctrl++` mean the same thing in cian as they do
/// everywhere else and the size survives a restart.
#[derive(Debug, Clone, Default)]
pub struct FontOptions {
    /// Set an absolute size; `{}` is replaced by the level. Preferred, because
    /// it is the only form that can be restored on the next launch.
    pub set: Option<String>,
    /// Or a step either way, when the terminal only offers relative changes.
    pub bigger: Option<String>,
    pub smaller: Option<String>,
    /// The level to start from, and the range to stay inside.
    pub start: i64,
    pub min: i64,
    pub max: i64,
}

/// Mutable accumulator shared with the Lua callbacks during script execution.
#[derive(Default)]
struct Builder {
    theme: Theme,
    options: Options,
    keymaps: Vec<(String, String)>,
    sharepoint: Vec<(String, String)>,
    ext_open: HashMap<String, Function>,
    ssh_hosts: Vec<SshHost>,
    ai: Option<AiOptions>,
    /// Input-method switching, if `cian.ime{...}` was called.
    ime: Option<ImeOptions>,
    /// Font-size stepping, if `cian.font{...}` was called.
    font: Option<FontOptions>,
    /// Precondition facts about the environment, fed to every AI prompt.
    ai_context: Vec<String>,
    /// Command snippets declared with `cian.snippets{...}`.
    pub snippets: Vec<Snippet>,
    errors: Vec<String>,
}

/// Fully-parsed configuration.
///
/// Owns the Lua runtime so `ext_open` callbacks — and the helpers they call,
/// like `cian.spawn` — stay valid for the whole life of the app.
#[derive(Default)]
pub struct Config {
    pub theme: Theme,
    pub options: Options,
    /// `(key, action-name)` pairs the user explicitly bound. The UI validates
    /// the action names and reports any it does not recognise.
    pub keymaps: Vec<(String, String)>,
    /// Synced SharePoint / OneDrive libraries: which local folder is which
    /// address in the cloud. Stated rather than guessed — see
    /// `cian_core::office::SyncMap`.
    pub sharepoint: Vec<(String, String)>,
    /// SSH targets declared with `cian.ssh{...}`.
    pub ssh_hosts: Vec<SshHost>,
    /// AI settings declared with `cian.ai{...}`, if any.
    pub ai: Option<AiOptions>,
    /// Input-method switching declared with `cian.ime{...}`, if any.
    pub ime: Option<ImeOptions>,
    /// Font-size stepping declared with `cian.font{...}`, if any.
    pub font: Option<FontOptions>,
    /// Precondition facts declared with `cian.ai_context{...}`, prepended to
    /// every AI prompt so answers assume the user's actual environment.
    pub ai_context: Vec<String>,
    /// Command snippets declared with `cian.snippets{...}`, sent to the active
    /// shell pane from the `:snip` picker.
    pub snippets: Vec<Snippet>,
    /// Non-fatal problems collected while loading (surfaced in a notice popup).
    pub errors: Vec<String>,
    ext_open: HashMap<String, Function>,
    /// Held purely to keep the Lua runtime (and thus every `ext_open` handle and
    /// helper) alive for the app's lifetime. Never read directly.
    #[allow(dead_code)]
    _lua: Option<Lua>,
}

impl Config {
    /// Does the user have a handler registered for this (lower-cased) extension?
    pub fn has_ext_open(&self, ext: &str) -> bool {
        self.ext_open.contains_key(&ext.to_lowercase())
    }

    /// Invoke the user's handler for `ext`, passing the file path as a string.
    /// Returns `None` if no handler is registered.
    pub fn run_ext_open(&self, ext: &str, path: &Path) -> Option<Result<(), String>> {
        let f = self.ext_open.get(&ext.to_lowercase())?;
        let arg = path.to_string_lossy().into_owned();
        Some(f.call::<()>(arg).map_err(|e| e.to_string()))
    }
}

/// The user's home directory: `$HOME`, or `$USERPROFILE` on Windows.
/// Case-insensitive `*`/`?` glob of a single filename component.
///
/// Case-insensitive because both callers want it that way: macro scripts match
/// `*.log` against `app.LOG`, and `:mark` is typed in a hurry.
pub fn glob_match(pat: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) =
        (pat.to_lowercase().chars().collect(), name.to_lowercase().chars().collect());
    // Classic iterative wildcard match with backtracking on `*`.
    let (mut pi, mut ni, mut star, mut mark) = (0usize, 0usize, None, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// The user's home directory. `HOME` everywhere but Windows, which sets
/// `USERPROFILE` instead.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// The cian config files that mark a directory as a cian config directory. Used
/// for portable-mode detection: if any of these sits next to the executable,
/// that directory is treated as a portable config set.
const CONFIG_MARKERS: [&str; 3] = ["init.lua", "shortcuts.lua", "macro.lua"];

/// The directory the running executable lives in, resolved through symlinks.
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

/// The user config directory: `$CIAN_CONFIG_DIR` if set, else `~/.config/cian`.
pub fn user_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CIAN_CONFIG_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    Some(home_dir()?.join(".config").join("cian"))
}

/// True when the executable's directory holds a cian config file — i.e. cian is
/// being run "portable", carried around with its config next to it.
pub fn is_portable() -> bool {
    exe_dir()
        .map(|d| CONFIG_MARKERS.iter().any(|m| d.join(m).exists()))
        .unwrap_or(false)
}

/// Where to **read** config file `name` from. A copy sitting next to the
/// executable wins (portable: carry cian + its `*.lua` on a stick and they take
/// precedence); otherwise the user config directory.
pub fn config_read_path(name: &str) -> Option<PathBuf> {
    read_path_in(exe_dir().as_deref(), user_config_dir().as_deref(), name)
}

/// Where to **write** config file `name`. Next to the executable when a copy is
/// already there, or when the executable directory is a portable config set
/// (so bookmarks/macros created in portable mode stay with the binary);
/// otherwise the user config directory.
pub fn config_write_path(name: &str) -> Option<PathBuf> {
    write_path_in(exe_dir().as_deref(), user_config_dir().as_deref(), name)
}

/// The read-resolution logic, split out from the executable lookup so it can be
/// tested against real directories: the portable copy next to the executable
/// wins only if it actually exists, else the user directory.
fn read_path_in(exe_dir: Option<&Path>, user_dir: Option<&Path>, name: &str) -> Option<PathBuf> {
    if let Some(dir) = exe_dir {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    user_dir.map(|d| d.join(name))
}

/// The write-resolution logic (see [`read_path_in`]): next to the executable if
/// that file is already there or the directory is a portable config set, else
/// the user directory.
fn write_path_in(exe_dir: Option<&Path>, user_dir: Option<&Path>, name: &str) -> Option<PathBuf> {
    if let Some(dir) = exe_dir {
        let here = dir.join(name);
        if here.exists() || CONFIG_MARKERS.iter().any(|m| dir.join(m).exists()) {
            return Some(here);
        }
    }
    user_dir.map(|d| d.join(name))
}

/// Resolve `init.lua`: the portable copy next to the executable if present,
/// otherwise `$CIAN_CONFIG_DIR`/`~/.config/cian`.
pub fn config_path() -> Option<PathBuf> {
    config_read_path("init.lua")
}

/// Load the configuration. Never panics and never returns an error: anything
/// that goes wrong is recorded in [`Config::errors`] and defaults are used.
/// Turn Lua's terse "invalid escape sequence" into advice.
///
/// Pasting a Windows path straight into a quoted string — the natural thing to
/// do for `shell` — makes `\W`, `\S`, `\v` … look like escape sequences, and
/// the resulting syntax error takes the *whole* config down with it. The raw
/// message gives no hint that backslashes are the problem, so spell it out.
fn escape_hint(err: &str) -> Vec<String> {
    if !err.contains("invalid escape sequence") {
        return Vec::new();
    }
    vec![
        "  hint: a backslash starts an escape sequence in Lua, so a Windows".into(),
        "  path cannot be pasted into \"...\" as-is. Use [[...]] instead:".into(),
        "    cian.set_option(\"shell\", [[C:\\path\\to\\shell.exe]])".into(),
    ]
}

pub fn load() -> Config {
    match config_path() {
        // init.lua itself may be absent while a split file is not: somebody
        // who only wanted key bindings writes keymap.lua and nothing else, and
        // somebody tidying up deletes an init.lua that had become empty. Both
        // used to lose their config in silence. `load_from` filters the split
        // files for existence and reports a missing init.lua as nothing at
        // all, so the path can be handed over either way.
        Some(p) if p.exists() || split_config_exists(&p) => {
            let mut c = load_from(&p);
            // Only worth saying if a config actually holds a secret — and the
            // secret may live in ssh.lua now rather than init.lua, so check both.
            if c.ssh_hosts.iter().any(|h| h.users.iter().any(|u| u.password.is_some())) {
                let dir = p.parent();
                // Only files that can carry a password (init.lua, ssh.lua).
                for name in ["init.lua", "ssh.lua"] {
                    if let Some(f) = dir.map(|d| d.join(name)).filter(|f| f.exists()) {
                        c.errors.extend(permission_warning(&f));
                    }
                }
            }
            c
        }
        _ => Config::default(),
    }
}

/// Warn when a config holding plaintext passwords is readable by anyone else
/// on the machine. Storing them is the user's call; leaving them world-readable
/// is almost never intended.
#[cfg(unix)]
fn permission_warning(path: &Path) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(path) else { return Vec::new() };
    let mode = meta.permissions().mode() & 0o077;
    if mode == 0 {
        return Vec::new();
    }
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("the config");
    vec![
        format!("{} holds SSH passwords but is readable by others (mode {:o}).", name, meta.permissions().mode() & 0o777),
        format!("  fix: chmod 600 {}", path.display()),
    ]
}

#[cfg(not(unix))]
fn permission_warning(_path: &Path) -> Vec<String> {
    // Windows ACLs are not a mode bitmask; a meaningful check would need the
    // security API, and a wrong warning is worse than none.
    Vec::new()
}

/// The optional config files that may sit next to `init.lua` to keep it tidy:
/// SSH hosts and key bindings can each move to their own file. They are plain
/// Lua too, sharing init.lua's `cian.*` API, so `cian.ssh{…}` in `ssh.lua` and
/// `cian.set_keymap(…)` in `keymap.lua` accumulate into the same config.
pub const SPLIT_CONFIG_FILES: [&str; 2] = ["ssh.lua", "keymap.lua"];

/// Is there a `ssh.lua` or `keymap.lua` beside this (possibly absent)
/// `init.lua`?
fn split_config_exists(init: &Path) -> bool {
    init.parent().is_some_and(|dir| SPLIT_CONFIG_FILES.iter().any(|n| dir.join(n).exists()))
}

fn load_from(path: &Path) -> Config {
    let lua = Lua::new();
    let builder = Rc::new(RefCell::new(Builder::default()));

    if let Err(e) = install_api(&lua, &builder) {
        return Config {
            errors: vec![format!("failed to set up Lua API: {}", e)],
            ..Config::default()
        };
    }

    let mut errors = Vec::new();
    // init.lua first, then any split-out files in the same directory, all sharing
    // one Lua context so their cian.* calls build one config.
    let mut files = Vec::new();
    if path.exists() {
        files.push(path.to_path_buf());
    }
    if let Some(dir) = path.parent() {
        files.extend(SPLIT_CONFIG_FILES.iter().map(|n| dir.join(n)).filter(|p| p.exists()));
    }
    for f in &files {
        let name = f.file_name().and_then(|s| s.to_str()).unwrap_or("config.lua").to_string();
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                // A missing init.lua is the caller's concern; a listed extra was
                // filtered for existence, so a read error here is worth flagging.
                errors.push(format!("cannot read {}: {}", f.display(), e));
                continue;
            }
        };
        if let Err(e) = lua.load(&src).set_name(&name).exec() {
            errors.push(format!("{}: {}", name, e));
            errors.extend(escape_hint(&e.to_string()));
        }
    }

    // Pull the accumulated config out by cloning; the Lua handles stay valid
    // because we move `lua` into the returned Config below.
    #[allow(clippy::type_complexity)]
    let (theme, options, keymaps, sharepoint, ext_open, ssh_hosts, ai, ime, font, ai_context, snippets, builder_errors) = {
        let b = builder.borrow();
        (
            b.theme.clone(),
            b.options.clone(),
            b.keymaps.clone(),
            b.sharepoint.clone(),
            b.ext_open.clone(),
            b.ssh_hosts.clone(),
            b.ai.clone(),
            b.ime.clone(),
            b.font.clone(),
            b.ai_context.clone(),
            b.snippets.clone(),
            b.errors.clone(),
        )
    };
    errors.extend(builder_errors);

    Config {
        theme,
        options,
        keymaps,
        sharepoint,
        ext_open,
        ssh_hosts,
        ai,
        ime,
        font,
        ai_context,
        snippets,
        errors,
        _lua: Some(lua),
    }
}

/// Parse a `users` list, where each entry is either a bare name or a table
/// carrying credentials:
///
/// ```lua
/// users = { "root", { name = "deploy", password = "..." } }
/// ```
///
/// Mixing the two matters: it lets a host move to key auth one login at a
/// time, dropping the stored secret as each one is migrated.
fn parse_users(t: Option<Table>) -> mlua::Result<Vec<SshUser>> {
    let Some(t) = t else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for v in t.sequence_values::<Value>() {
        match v? {
            Value::String(s) => out.push(SshUser::plain(s.to_str()?.to_owned())),
            Value::Table(u) => {
                let Some(name) = u.get::<Option<String>>("name")? else { continue };
                out.push(SshUser {
                    name,
                    password: u.get::<Option<String>>("password")?,
                    password_cmd: u.get::<Option<String>>("password_cmd")?,
                    key: u.get::<Option<String>>("key")?,
                    key_pass: u.get::<Option<String>>("key_pass")?,
                });
            }
            _ => {}
        }
    }
    Ok(out)
}

fn install_api(lua: &Lua, builder: &Rc<RefCell<Builder>>) -> mlua::Result<()> {
    let cian = lua.create_table()?;

    // cian.set_theme "solarized-light"   -- pick a preset, or
    // cian.set_theme { preset = "solarized-light", accent = "#...", ... }
    {
        let b = builder.clone();
        cian.set(
            "set_theme",
            lua.create_function(move |_, v: mlua::Value| {
                let mut bm = b.borrow_mut();
                // A bare string is shorthand for choosing a preset.
                let t = match v {
                    mlua::Value::String(s) => {
                        bm.theme.preset = Some(s.to_str()?.to_string());
                        return Ok(());
                    }
                    mlua::Value::Table(t) => t,
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "set_theme expects a table or a preset name, got {}",
                            other.type_name()
                        )))
                    }
                };
                if let Some(v) = t.get::<Option<String>>("preset")? {
                    bm.theme.preset = Some(v);
                }
                if let Some(v) = t.get::<Option<String>>("accent")? {
                    bm.theme.accent = Some(v);
                }
                if let Some(v) = t.get::<Option<String>>("status_bg")? {
                    bm.theme.status_bg = Some(v);
                }
                if let Some(v) = t.get::<Option<String>>("selected_bg")? {
                    bm.theme.selected_bg = Some(v);
                }
                if let Some(v) = t.get::<Option<String>>("visual_bg")? {
                    bm.theme.visual_bg = Some(v);
                }
                if let Some(v) = t.get::<Option<String>>("mark_fg")? {
                    bm.theme.mark_fg = Some(v);
                }
                Ok(())
            })?,
        )?;
    }

    // cian.set_keymap("x", "delete")
    {
        let b = builder.clone();
        cian.set(
            "set_keymap",
            lua.create_function(move |_, (key, action): (String, String)| {
                let mut bm = b.borrow_mut();
                // A bare character, or one with modifiers in front of it:
                // "x", "alt+g", "ctrl+f", "shift+s". Validated here only far
                // enough to catch a typo; cian-tui turns it into a real key.
                let spec = key.trim().to_string();
                let last = spec.rsplit('+').next().unwrap_or("");
                if last.chars().count() == 1 && !spec.ends_with('+') {
                    bm.keymaps.push((spec, action));
                } else {
                    bm.errors.push(format!(
                        "set_keymap: expected a key like \"x\" or \"alt+g\", got {:?}",
                        key
                    ));
                }
                Ok(())
            })?,
        )?;
    }

    // cian.sharepoint { { local = "/path", url = "https://…" }, … }
    {
        let b = builder.clone();
        cian.set(
            "sharepoint",
            lua.create_function(move |_, list: Table| {
                let mut bm = b.borrow_mut();
                for pair in list.sequence_values::<Table>() {
                    let Ok(t) = pair else {
                        bm.errors.push("sharepoint: each entry must be a table".into());
                        continue;
                    };
                    let local: Option<String> = t.get("local").ok();
                    let url: Option<String> = t.get("url").ok();
                    match (local, url) {
                        (Some(l), Some(u)) if !l.is_empty() && !u.is_empty() => {
                            bm.sharepoint.push((l, u))
                        }
                        _ => bm.errors.push(
                            "sharepoint: each entry needs local = \"…\" and url = \"…\"".into(),
                        ),
                    }
                }
                Ok(())
            })?,
        )?;
    }

    {
        let b = builder.clone();
        cian.set(
            "set_option",
            lua.create_function(move |lua, (name, val): (String, Value)| {
                let mut bm = b.borrow_mut();
                match name.as_str() {
                    "verify_transfers" => match bool::from_lua(val, lua) {
                        Ok(v) => bm.options.verify_transfers = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: verify_transfers expects a boolean".into()),
                    },
                    // Removed: `mask` was only ever displayed, never applied.
                    // Name it explicitly so an old config gets told what to do
                    // instead of a bare "unknown option".
                    "mask" => bm.errors.push(
                        "set_option: `mask` was removed — it never filtered anything. \
                         Press `/` in the app to narrow the listing instead."
                            .into(),
                    ),
                    "shell" => match String::from_lua(val, lua) {
                        Ok(v) => bm.options.shell = Some(v),
                        Err(_) => {
                            bm.errors.push("set_option: shell expects a string".into())
                        }
                    },
                    "borders" => match String::from_lua(val, lua) {
                        Ok(v) => bm.options.borders = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: borders expects \"rounded\" or \"plain\"".into()),
                    },
                    "show_hidden" => match bool::from_lua(val, lua) {
                        Ok(v) => bm.options.show_hidden = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: show_hidden expects a boolean".into()),
                    },
                    "tab_width" => match usize::from_lua(val, lua) {
                        Ok(v) => bm.options.tab_width = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: tab_width expects a number".into()),
                    },
                    "nerd_fonts" => match bool::from_lua(val, lua) {
                        Ok(v) => bm.options.nerd_fonts = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: nerd_fonts expects a boolean".into()),
                    },
                    "home" => match String::from_lua(val, lua) {
                        Ok(v) => bm.options.home = Some(v),
                        Err(_) => {
                            bm.errors.push("set_option: home expects a directory path".into())
                        }
                    },
                    "editor" => match String::from_lua(val, lua) {
                        Ok(v) => bm.options.editor = Some(v),
                        Err(_) => {
                            bm.errors.push("set_option: editor expects a command string".into())
                        }
                    },
                    "key_hints" => match bool::from_lua(val, lua) {
                        Ok(v) => bm.options.key_hints = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: key_hints expects a boolean".into()),
                    },
                    // Not `editor`, which is above and means something else
                    // entirely — the *external* one `E` shells out to.
                    "edit_style" => match String::from_lua(val, lua) {
                        Ok(v) if matches!(v.as_str(), "vim" | "notepad") => {
                            bm.options.edit_style = Some(v)
                        }
                        Ok(v) => bm.errors.push(format!(
                            "set_option: edit_style expects \"vim\" or \"notepad\" (got {v:?})"
                        )),
                        Err(_) => bm
                            .errors
                            .push("set_option: edit_style expects a string".into()),
                    },
                    "view" => match String::from_lua(val, lua) {
                        Ok(v) if matches!(v.as_str(), "classic" | "details" | "icons") => {
                            bm.options.view = Some(v)
                        }
                        Ok(v) => bm.errors.push(format!(
                            "set_option: view expects \"classic\", \"details\" or \"icons\" (got {v:?})"
                        )),
                        Err(_) => bm
                            .errors
                            .push("set_option: view expects a string".into()),
                    },
                    "animation_ms" => match u64::from_lua(val, lua) {
                        Ok(v) => bm.options.animation_ms = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: animation_ms expects a number".into()),
                    },
                    "preview" => match bool::from_lua(val, lua) {
                        Ok(v) => bm.options.preview = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: preview expects a boolean".into()),
                    },
                    "preview_skip" => match &val {
                        Value::String(one) => match one.to_str() {
                            Ok(v) => bm.options.preview_skip.push(v.trim().to_string()),
                            Err(_) => bm
                                .errors
                                .push("set_option: preview_skip expects text".into()),
                        },
                        Value::Table(t) => {
                            for item in t.clone().sequence_values::<String>() {
                                match item {
                                    Ok(v) => bm.options.preview_skip.push(v.trim().to_string()),
                                    Err(_) => bm.errors.push(
                                        "set_option: preview_skip expects a list of extensions"
                                            .into(),
                                    ),
                                }
                            }
                        }
                        _ => bm.errors.push(
                            "set_option: preview_skip expects an extension or a list of them"
                                .into(),
                        ),
                    },
                    "read_cloud_files" => match bool::from_lua(val, lua) {
                        Ok(v) => bm.options.read_cloud_files = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: read_cloud_files expects a boolean".into()),
                    },
                    "notify" => match bool::from_lua(val, lua) {
                        Ok(v) => bm.options.notify = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: notify expects a boolean".into()),
                    },
                    "notify_min_secs" => match u64::from_lua(val, lua) {
                        Ok(v) => bm.options.notify_min_secs = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: notify_min_secs expects a number".into()),
                    },
                    // A speed, written the way a speed is written: "2M",
                    // "500k", "1.5MB/s". cian parses it; anything unreadable is
                    // reported rather than silently ignored.
                    "transfer_limit" | "speed_limit" => match String::from_lua(val, lua) {
                        Ok(v) => bm.options.transfer_limit = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: transfer_limit expects a string like \"2M\"".into()),
                    },
                    "lang" => match String::from_lua(val, lua) {
                        Ok(v) if v == "ja" || v == "en" => bm.options.lang = Some(v),
                        Ok(_) => bm
                            .errors
                            .push("set_option: lang expects \"ja\" or \"en\"".into()),
                        Err(_) => {
                            bm.errors.push("set_option: lang expects \"ja\" or \"en\"".into())
                        }
                    },
                    "menu_lang" => match String::from_lua(val, lua) {
                        Ok(v) if v == "ja" || v == "en" => bm.options.menu_lang = Some(v),
                        Ok(_) => bm
                            .errors
                            .push("set_option: menu_lang expects \"ja\" or \"en\"".into()),
                        Err(_) => {
                            bm.errors.push("set_option: menu_lang expects \"ja\" or \"en\"".into())
                        }
                    },
                    other => bm
                        .errors
                        .push(format!("set_option: unknown option {:?}", other)),
                }
                Ok(())
            })?,
        )?;
    }

    // cian.ssh { users = {...}, hosts = { { name=, host=, users= }, ... } }
    {
        let b = builder.clone();
        cian.set(
            "ssh",
            lua.create_function(move |_, t: Table| {
                let mut bm = b.borrow_mut();
                // Fleet-wide default, overridable per host.
                let default_users = parse_users(t.get::<Option<Table>>("users")?)?;
                let hosts: Vec<Table> = match t.get::<Option<Vec<Table>>>("hosts")? {
                    Some(v) => v,
                    None => {
                        bm.errors.push("cian.ssh: expected a `hosts` list".into());
                        return Ok(());
                    }
                };
                for h in hosts {
                    let host: String = match h.get::<Option<String>>("host")? {
                        Some(v) => v,
                        None => {
                            bm.errors.push("cian.ssh: a host entry is missing `host`".into());
                            continue;
                        }
                    };
                    // `name` is what the picker shows; default to the address.
                    let name = h.get::<Option<String>>("name")?.unwrap_or_else(|| host.clone());
                    let users = match h.get::<Option<Table>>("users")? {
                        Some(t) => parse_users(Some(t))?,
                        None => default_users.clone(),
                    };
                    if users.is_empty() {
                        bm.errors.push(format!(
                            "cian.ssh: host {:?} has no users (set `users` here or at the top level)",
                            name
                        ));
                        continue;
                    }
                    let port = h.get::<Option<u16>>("port")?;
                    let notes = h.get::<Option<String>>("notes")?.filter(|s| !s.trim().is_empty());
                    bm.ssh_hosts.push(SshHost { name, host, users, port, notes });
                }
                Ok(())
            })?,
        )?;
    }

    // cian.ai { endpoint=, model=, auth_mode=, python=, api_version=, api_key=, api_base_url= }
    {
        let b = builder.clone();
        cian.set(
            "ai",
            lua.create_function(move |_, t: Table| {
                let mut ai = AiOptions::default();
                let get = |k: &str| -> Option<String> { t.get::<Option<String>>(k).ok().flatten() };
                if let Some(v) = get("python") { ai.python = v; }
                if let Some(v) = get("endpoint") { ai.endpoint = v; }
                if let Some(v) = get("model") { ai.model = v; }
                if let Some(v) = get("api_version") { ai.api_version = v; }
                if let Some(v) = get("auth_mode") { ai.auth_mode = v; }
                if let Some(v) = get("api_key") { ai.api_key = v; }
                if let Some(v) = get("api_base_url") { ai.api_base_url = v; }
                b.borrow_mut().ai = Some(ai);
                Ok(())
            })?,
        )?;
    }


    // cian.ime{ off = "...", on = "..." }  — switch the system input method
    // with cian's mode, so single-key commands work with Japanese input on.
    {
        let b = builder.clone();
        cian.set(
            "ime",
            lua.create_function(move |_, t: Option<Table>| {
                let mut c = ImeOptions { restore: true, ..Default::default() };
                if let Some(t) = t {
                    let s = |k: &str| -> Option<String> { t.get::<Option<String>>(k).ok().flatten() };
                    c.helper = s("helper");
                    c.query = s("query");
                    c.set = s("set");
                    c.off = s("off");
                    if let Ok(Some(v)) = t.get::<Option<bool>>("restore") {
                        c.restore = v;
                    }
                }
                b.borrow_mut().ime = Some(c);
                Ok(())
            })?,
        )?;
    }

    // cian.font{ set = "…{}…" } or { bigger = "…", smaller = "…" }
    {
        let b = builder.clone();
        cian.set(
            "font",
            lua.create_function(move |_, t: Option<Table>| {
                let mut c = FontOptions { start: 13, min: 6, max: 40, ..Default::default() };
                if let Some(t) = t {
                    let s = |k: &str| -> Option<String> { t.get::<Option<String>>(k).ok().flatten() };
                    c.set = s("set");
                    c.bigger = s("bigger");
                    c.smaller = s("smaller");
                    for (k, slot) in [("start", 0), ("min", 1), ("max", 2)] {
                        if let Ok(Some(v)) = t.get::<Option<i64>>(k) {
                            match slot {
                                0 => c.start = v,
                                1 => c.min = v,
                                _ => c.max = v,
                            }
                        }
                    }
                }
                b.borrow_mut().font = Some(c);
                Ok(())
            })?,
        )?;
    }

    // cian.ai_context("fact")  or  cian.ai_context{ "fact one", "fact two" }
    //
    // Precondition facts the AI should assume — e.g. the OS the file panes
    // browse, the deployment target, house conventions. Prepended to every AI
    // prompt. Additive across calls.
    {
        let b = builder.clone();
        cian.set(
            "ai_context",
            lua.create_function(move |_, v: Value| {
                let mut bm = b.borrow_mut();
                match v {
                    Value::String(s) => {
                        let f = s.to_str()?.trim().to_string();
                        if !f.is_empty() {
                            bm.ai_context.push(f);
                        }
                    }
                    Value::Table(t) => {
                        for item in t.sequence_values::<String>() {
                            let f = item?.trim().to_string();
                            if !f.is_empty() {
                                bm.ai_context.push(f);
                            }
                        }
                    }
                    other => {
                        bm.errors.push(format!(
                            "cian.ai_context: expected a string or a list of strings, got {}",
                            other.type_name()
                        ));
                    }
                }
                Ok(())
            })?,
        )?;
    }

    // cian.snippets{ { name=, cmd=, enter=, confirm= }, ... }
    //
    // Command snippets sent to the active shell pane from the `:snip` picker.
    // `enter` (default true) runs it immediately; `confirm` (default false)
    // asks first. Additive across calls.
    {
        let b = builder.clone();
        cian.set(
            "snippets",
            lua.create_function(move |_, t: Table| {
                let mut bm = b.borrow_mut();
                let entries: Vec<Table> = match t.sequence_values::<Table>().collect::<mlua::Result<_>>() {
                    Ok(v) => v,
                    Err(e) => {
                        bm.errors.push(format!("cian.snippets: expected a list of tables ({})", e));
                        return Ok(());
                    }
                };
                for s in entries {
                    let cmd = match s.get::<Option<String>>("cmd")? {
                        Some(c) if !c.is_empty() => c,
                        _ => {
                            bm.errors.push("cian.snippets: an entry is missing `cmd`".into());
                            continue;
                        }
                    };
                    let name = s.get::<Option<String>>("name")?.unwrap_or_else(|| cmd.clone());
                    let enter = s.get::<Option<bool>>("enter")?.unwrap_or(true);
                    let confirm = s.get::<Option<bool>>("confirm")?.unwrap_or(false);
                    bm.snippets.push(Snippet { name, cmd, enter, confirm });
                }
                Ok(())
            })?,
        )?;
    }

    // cian.on_open("md", function(path) ... end)
    {
        let b = builder.clone();
        cian.set(
            "on_open",
            lua.create_function(move |_, (ext, f): (String, Function)| {
                let key = ext.trim_start_matches('.').to_lowercase();
                b.borrow_mut().ext_open.insert(key, f);
                Ok(())
            })?,
        )?;
    }

    // cian.spawn({ "nvim", path }) — launch a detached process.
    cian.set(
        "spawn",
        lua.create_function(|_, args: Vec<String>| {
            if args.is_empty() {
                return Err(mlua::Error::RuntimeError("cian.spawn: empty command".into()));
            }
            cian_core::proc::quiet(&args[0])
                .args(&args[1..])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| mlua::Error::RuntimeError(format!("cian.spawn: {}", e)))?;
            Ok(())
        })?,
    )?;

    // cian.open(path) — hand a path/URL to the OS default opener.
    cian.set(
        "open",
        lua.create_function(|_, target: String| {
            os_open(&target)
                .map_err(|e| mlua::Error::RuntimeError(format!("cian.open: {}", e)))?;
            Ok(())
        })?,
    )?;

    lua.globals().set("cian", cian)?;
    Ok(())
}

fn os_open(target: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = cian_core::proc::quiet("open");
    #[cfg(target_os = "linux")]
    let mut cmd = cian_core::proc::quiet("xdg-open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = cian_core::proc::quiet("cmd");
        c.arg("/C").arg("start").arg("");
        c
    };
    cmd.arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

// ---- Remembered UI state ----
//
// A plain `key = value` file beside the config, for the things a person
// changes by using the program rather than by editing it: which look, which
// editor grammar, how big the font. Deliberately not `init.lua` — that file is
// written by hand and read as code, and a program that rewrites it would be
// reformatting somebody's comments.
//
// Here rather than in a front end because there are two of them now, and a
// look chosen in one and not the other would be two programs.
const STATE_FILE: &str = "state.toml";

/// The theme name saved from a previous session's `:theme`, if any. Applied at
/// startup on top of init.lua so a chosen theme survives a restart.
pub fn load_saved_theme() -> Option<String> {
    state_get("theme")
}

/// One value out of the state file. Hand-parsed `key = value` — no TOML
/// dependency for a handful of scalars.
pub fn state_get(key: &str) -> Option<String> {
    let path = config_read_path(STATE_FILE)?;
    let text = std::fs::read_to_string(path).ok()?;
    state_get_in(&text, key)
}

pub fn state_get_in(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix(key) else { continue };
        let Some(val) = rest.trim_start().strip_prefix('=') else { continue };
        let val = val.trim().trim_matches('"').trim();
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }
    None
}

/// Set one value, keeping the others. Best-effort: a read-only config dir
/// just means the choice does not stick, which is not worth interrupting
/// anyone over.
pub fn state_set(key: &str, value: &str) {
    let Some(path) = config_write_path(STATE_FILE) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::write(path, state_with(&old, key, value));
}

/// The state file's text with `key` set to `value` — replacing the line it
/// was on, or added at the end.
pub fn state_with(text: &str, key: &str, value: &str) -> String {
    const HEAD: &str = "# cian runtime state — managed by cian (see :where)";
    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in text.lines() {
        let is_key = line
            .trim_start()
            .strip_prefix(key)
            .map(|r| r.trim_start().starts_with('='))
            .unwrap_or(false);
        if is_key {
            if !replaced {
                out.push(format!("{key} = \"{value}\""));
                replaced = true;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if out.first().map(|l| l.trim() != HEAD).unwrap_or(true) {
        out.insert(0, HEAD.to_string());
    }
    if !replaced {
        out.push(format!("{key} = \"{value}\""));
    }
    out.join("\n") + "\n"
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_and_keymap_can_live_in_their_own_files() {
        let dir = tempfile::tempdir().unwrap();
        // init.lua keeps only display / AI, per the intended split.
        std::fs::write(
            dir.path().join("init.lua"),
            "cian.set_theme \"dracula\"\ncian.ai_context(\"deploy to RHEL 8\")\n",
        )
        .unwrap();
        // SSH hosts move to ssh.lua …
        std::fs::write(
            dir.path().join("ssh.lua"),
            "cian.ssh{ users = { \"root\" }, hosts = { { name = \"db\", host = \"10.0.0.9\" } } }\n",
        )
        .unwrap();
        // … and key bindings to keymap.lua, all sharing one config.
        std::fs::write(
            dir.path().join("keymap.lua"),
            "cian.set_keymap(\"x\", \"delete\")\n",
        )
        .unwrap();

        let cfg = load_from(&dir.path().join("init.lua"));
        assert!(cfg.errors.is_empty(), "{:?}", cfg.errors);
        assert_eq!(cfg.theme.preset.as_deref(), Some("dracula"), "init.lua still applies");
        assert_eq!(cfg.ai_context, vec!["deploy to RHEL 8"]);
        assert_eq!(cfg.ssh_hosts.len(), 1, "ssh.lua contributed a host");
        assert_eq!(cfg.ssh_hosts[0].name, "db");
        assert!(
            cfg.keymaps.iter().any(|(k, a)| k == "x" && a == "delete"),
            "keymap.lua bound x → delete: {:?}",
            cfg.keymaps
        );
    }

    #[test]
    fn ai_context_and_host_notes_parse() {
        let dir = tempfile::tempdir().unwrap();
        let init = dir.path().join("init.lua");
        std::fs::write(
            &init,
            r#"
            cian.ai_context("The file panes browse a RHEL 8 NFS mount.")
            cian.ai_context{ "Prefer POSIX sh.", "Deploy target is Oracle 19c." }
            cian.ssh{
              users = { "root" },
              hosts = {
                { name = "db", host = "10.0.0.9", notes = "RHEL 8, Oracle 19c, nginx 1.24" },
                { name = "web", host = "10.0.0.10" },
              },
            }
            "#,
        )
        .unwrap();
        let cfg = load_from(&init);
        assert!(cfg.errors.is_empty(), "{:?}", cfg.errors);
        assert_eq!(
            cfg.ai_context,
            vec![
                "The file panes browse a RHEL 8 NFS mount.",
                "Prefer POSIX sh.",
                "Deploy target is Oracle 19c.",
            ]
        );
        assert_eq!(cfg.ssh_hosts[0].notes.as_deref(), Some("RHEL 8, Oracle 19c, nginx 1.24"));
        assert_eq!(cfg.ssh_hosts[1].notes, None);
    }

    #[test]
    fn shipped_example_init_parses_cleanly() {
        // The template users copy must always load without errors — the AI
        // context block and every other snippet in it stay valid Lua.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/init.lua");
        if !path.exists() {
            eprintln!("example init.lua not found at {}; skipping", path.display());
            return;
        }
        let cfg = load_from(&path);
        assert!(cfg.errors.is_empty(), "{:?}", cfg.errors);
    }

    #[test]
    fn snippets_parse_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let init = dir.path().join("init.lua");
        std::fs::write(
            &init,
            r#"
            cian.snippets{
              { name = "sqlplus dev", cmd = "sqlplus u@DEV" },
              { cmd = "tail -f /var/log/app.log", enter = false },
              { name = "danger", cmd = "rm -rf tmp", confirm = true },
              { name = "seq", cmd = [[
cd /x
pwd
ls]] },
              { name = "no cmd" },
            }
            "#,
        )
        .unwrap();
        let cfg = load_from(&init);
        assert_eq!(cfg.snippets.len(), 4, "the entry without cmd is dropped");
        // A multi-line cmd keeps its newlines so it runs as a sequence.
        assert_eq!(cfg.snippets[3].cmd, "cd /x\npwd\nls");
        assert_eq!(cfg.snippets[0].name, "sqlplus dev");
        assert!(cfg.snippets[0].enter, "enter defaults to true");
        assert!(!cfg.snippets[0].confirm, "confirm defaults to false");
        // A missing name falls back to the command text.
        assert_eq!(cfg.snippets[1].name, "tail -f /var/log/app.log");
        assert!(!cfg.snippets[1].enter, "enter=false honoured");
        assert!(cfg.snippets[2].confirm, "confirm=true honoured");
        assert!(cfg.errors.iter().any(|e| e.contains("missing `cmd`")), "{:?}", cfg.errors);
    }

    #[test]
    fn ai_context_rejects_a_number() {
        let dir = tempfile::tempdir().unwrap();
        let init = dir.path().join("init.lua");
        std::fs::write(&init, "cian.ai_context(42)").unwrap();
        let cfg = load_from(&init);
        assert!(cfg.ai_context.is_empty());
        assert!(cfg.errors.iter().any(|e| e.contains("ai_context")), "{:?}", cfg.errors);
    }

    #[test]
    fn portable_copy_next_to_exe_wins_for_reading() {
        let exe = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        // No file next to the exe yet → read resolves to the user dir.
        assert_eq!(
            read_path_in(Some(exe.path()), Some(user.path()), "init.lua"),
            Some(user.path().join("init.lua"))
        );
        // Drop a copy next to the exe → it now wins.
        std::fs::write(exe.path().join("init.lua"), "-- portable").unwrap();
        assert_eq!(
            read_path_in(Some(exe.path()), Some(user.path()), "init.lua"),
            Some(exe.path().join("init.lua"))
        );
    }

    #[test]
    fn writes_go_beside_the_exe_only_in_portable_mode() {
        let exe = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        // Nothing portable next to the exe → a new file is written to the user dir.
        assert_eq!(
            write_path_in(Some(exe.path()), Some(user.path()), "shortcuts.lua"),
            Some(user.path().join("shortcuts.lua"))
        );
        // Once init.lua sits beside the exe, the directory is a portable set, so
        // a freshly-created shortcuts.lua stays with the binary.
        std::fs::write(exe.path().join("init.lua"), "-- portable").unwrap();
        assert_eq!(
            write_path_in(Some(exe.path()), Some(user.path()), "shortcuts.lua"),
            Some(exe.path().join("shortcuts.lua"))
        );
    }

    #[test]
    fn an_existing_file_beside_the_exe_is_written_in_place() {
        let exe = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::write(exe.path().join("shortcuts.lua"), "return {}").unwrap();
        // Even without any other marker, an existing copy keeps being written there.
        assert_eq!(
            write_path_in(Some(exe.path()), Some(user.path()), "shortcuts.lua"),
            Some(exe.path().join("shortcuts.lua"))
        );
    }

    /// A Windows path pasted into a quoted string is the likeliest way for a
    /// config to fail, and Lua's own message never mentions backslashes.
    #[test]
    fn an_invalid_escape_gets_a_windows_path_hint() {
        let msg = r#"syntax error: [string "init.lua"]:1: invalid escape sequence near '"C:\W'"#;
        let hint = escape_hint(msg);
        assert!(!hint.is_empty(), "should explain the backslash problem");
        let joined = hint.join("\n");
        assert!(joined.contains("[[...]]"), "should show the fix: {}", joined);
        assert!(joined.contains("escape sequence"), "should name the cause");
    }

    #[test]
    fn unrelated_errors_get_no_hint() {
        assert!(escape_hint("attempt to call a nil value").is_empty());
        assert!(escape_hint(r#"set_option: unknown option "nope""#).is_empty());
    }
}

#[cfg(test)]
mod split_config_alone_tests {
    use super::*;

    #[test]
    fn keymap_lua_is_read_without_an_init_lua() {
        // Somebody who only wants key bindings writes keymap.lua and nothing
        // else. It used to be read only when init.lua existed beside it, so
        // that person's bindings vanished without a word.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("keymap.lua"),
            "cian.set_keymap(\"x\", \"delete\")\n",
        )
        .unwrap();
        let cfg = load_from(&dir.path().join("init.lua"));
        assert!(
            cfg.keymaps.iter().any(|(k, a)| k == "x" && a == "delete"),
            "keymap.lua alone bound x → delete: {:?}",
            cfg.keymaps
        );
        assert!(cfg.errors.is_empty(), "a missing init.lua is not an error: {:?}", cfg.errors);
    }
}
