//! cian-lua: mlua-based configuration host.
//!
//! Loads `~/.config/cian/init.lua` (overridable with `CIAN_CONFIG_DIR`) and
//! exposes a small WezTerm-flavoured `cian` API to user scripts:
//!
//! ```lua
//! cian.set_theme({ accent = "#00d7d7", mark_fg = "yellow" })
//! cian.set_keymap("x", "delete")          -- bind key `x` to the delete action
//! cian.set_option("clipboard_on_copy", false)
//! cian.set_option("mask", "*.rs")
//! cian.on_open("md", function(path)        -- extension-dispatch execution
//!   cian.spawn({ "open", "-a", "Typora", path })
//! end)
//! ```
//!
//! Loading never fails the program: any syntax/runtime error is captured in
//! [`Config::errors`] and the UI falls back to defaults for whatever could not
//! be applied. This crate stays UI-agnostic — colours are passed through as raw
//! strings and parsed by the UI layer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;

use mlua::{Function, FromLua, Lua, Table, Value};

/// A colour spec exactly as the user wrote it: `"#rrggbb"`, a named colour
/// (`"cyan"`, `"yellow"`), or `"r,g,b"`. Parsed by the UI layer.
pub type ColorSpec = String;

/// User-supplied colour overrides. `None` means "keep the built-in default".
#[derive(Debug, Clone, Default)]
pub struct Theme {
    pub accent: Option<ColorSpec>,
    pub status_bg: Option<ColorSpec>,
    pub selected_bg: Option<ColorSpec>,
    pub visual_bg: Option<ColorSpec>,
    pub mark_fg: Option<ColorSpec>,
}

/// Behavioural switches. `None` means "keep the built-in default".
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub clipboard_on_copy: Option<bool>,
    pub mask: Option<String>,
}

/// Mutable accumulator shared with the Lua callbacks during script execution.
#[derive(Default)]
struct Builder {
    theme: Theme,
    options: Options,
    keymaps: Vec<(char, String)>,
    ext_open: HashMap<String, Function>,
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
    pub keymaps: Vec<(char, String)>,
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

/// Resolve the config file path: `$CIAN_CONFIG_DIR/init.lua` if set, otherwise
/// `~/.config/cian/init.lua`.
pub fn config_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CIAN_CONFIG_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("init.lua"));
        }
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config").join("cian").join("init.lua"))
}

/// Load the configuration. Never panics and never returns an error: anything
/// that goes wrong is recorded in [`Config::errors`] and defaults are used.
pub fn load() -> Config {
    match config_path() {
        Some(p) if p.exists() => load_from(&p),
        _ => Config::default(),
    }
}

fn load_from(path: &Path) -> Config {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return Config {
                errors: vec![format!("cannot read {}: {}", path.display(), e)],
                ..Config::default()
            };
        }
    };

    let lua = Lua::new();
    let builder = Rc::new(RefCell::new(Builder::default()));

    if let Err(e) = install_api(&lua, &builder) {
        return Config {
            errors: vec![format!("failed to set up Lua API: {}", e)],
            ..Config::default()
        };
    }

    let mut errors = Vec::new();
    if let Err(e) = lua.load(&src).set_name("init.lua").exec() {
        errors.push(format!("init.lua: {}", e));
    }

    // Pull the accumulated config out by cloning; the Lua handles stay valid
    // because we move `lua` into the returned Config below.
    let (theme, options, keymaps, ext_open, builder_errors) = {
        let b = builder.borrow();
        (
            b.theme.clone(),
            b.options.clone(),
            b.keymaps.clone(),
            b.ext_open.clone(),
            b.errors.clone(),
        )
    };
    errors.extend(builder_errors);

    Config {
        theme,
        options,
        keymaps,
        ext_open,
        errors,
        _lua: Some(lua),
    }
}

fn install_api(lua: &Lua, builder: &Rc<RefCell<Builder>>) -> mlua::Result<()> {
    let cian = lua.create_table()?;

    // cian.set_theme { accent = "...", status_bg = "...", ... }
    {
        let b = builder.clone();
        cian.set(
            "set_theme",
            lua.create_function(move |_, t: Table| {
                let mut bm = b.borrow_mut();
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
                let mut chars = key.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => bm.keymaps.push((c, action)),
                    _ => bm.errors.push(format!(
                        "set_keymap: key must be a single character, got {:?}",
                        key
                    )),
                }
                Ok(())
            })?,
        )?;
    }

    // cian.set_option("clipboard_on_copy", false) / cian.set_option("mask", "*.rs")
    {
        let b = builder.clone();
        cian.set(
            "set_option",
            lua.create_function(move |lua, (name, val): (String, Value)| {
                let mut bm = b.borrow_mut();
                match name.as_str() {
                    "clipboard_on_copy" => match bool::from_lua(val, lua) {
                        Ok(v) => bm.options.clipboard_on_copy = Some(v),
                        Err(_) => bm
                            .errors
                            .push("set_option: clipboard_on_copy expects a boolean".into()),
                    },
                    "mask" => match String::from_lua(val, lua) {
                        Ok(v) => bm.options.mask = Some(v),
                        Err(_) => {
                            bm.errors.push("set_option: mask expects a string".into())
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
            Command::new(&args[0])
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
    let mut cmd = Command::new("open");
    #[cfg(target_os = "linux")]
    let mut cmd = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
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
