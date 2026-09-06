//! The colour theme, border style, interface language (i18n via `tr`), and the
//! remappable `Action` enum — resolved from init.lua and installed into
//! process-wide statics at startup. Split out of lib.rs.

use std::sync::{OnceLock, RwLock};

use ratatui::style::Color;
use ratatui::widgets::BorderType;

/// Resolved color palette. Defaults match the original built-in theme; a
/// `~/.config/cian/init.lua` calling `cian.set_theme{...}` overrides any field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ResolvedTheme {
    pub(crate) accent: Color,
    pub(crate) status_bg: Color,
    pub(crate) selected_bg: Color,
    pub(crate) visual_bg: Color,
    pub(crate) mark_fg: Color,
    /// The surface behind panes and the shell. `None` leaves the terminal's own
    /// background showing (the dark default's behaviour); a light theme paints
    /// it so the look holds up on any terminal.
    pub(crate) base_bg: Option<Color>,
    /// Quieter greys for secondary text and borders.
    pub(crate) dim: Color,
    pub(crate) border: Color,
    /// Background of menus and dialogs.
    pub(crate) popup_bg: Color,
    /// File-type accents, indexed by [`FileKind`].
    pub(crate) file: FilePalette,
}

/// The eight file-type accents plus the two neutral tones, kept together so a
/// theme swaps them as a set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FilePalette {
    pub(crate) directory: Color,
    pub(crate) code: Color,
    pub(crate) config: Color,
    pub(crate) document: Color,
    pub(crate) image: Color,
    pub(crate) media: Color,
    pub(crate) archive: Color,
    pub(crate) executable: Color,
    pub(crate) muted: Color,
    pub(crate) plain: Color,
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        Self::DARK
    }
}

/// `0xRRGGBB` → a ratatui truecolor. `const` so whole palettes are compile-time
/// constants.
const fn rgb(v: u32) -> Color {
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

// The palettes themselves live in `cian_core::theme`, because the window
// needs the same eighteen and a palette kept in two places is two palettes the
// first time somebody adjusts one of them. What stays here is the part that is
// this front end's own: turning a spec into ratatui colours.
use cian_core::theme::Spec;

/// Expand a [`Spec`] into the full resolved palette. `const` so every preset is
/// a `const ResolvedTheme`.
const fn from_spec(s: Spec) -> ResolvedTheme {
    ResolvedTheme {
        accent: rgb(s.accent),
        status_bg: rgb(s.status),
        selected_bg: rgb(s.sel),
        visual_bg: rgb(s.visual),
        mark_fg: rgb(s.mark),
        base_bg: Some(rgb(s.bg)),
        dim: rgb(s.dim),
        border: rgb(s.border),
        popup_bg: rgb(s.popup),
        file: FilePalette {
            directory: rgb(s.blue),
            code: rgb(s.yellow),
            config: rgb(s.cyan),
            document: rgb(s.doc),
            image: rgb(s.magenta),
            media: rgb(s.cyan),
            archive: rgb(s.red),
            executable: rgb(s.green),
            muted: rgb(s.dim),
            plain: rgb(s.fg),
        },
    }
}

impl ResolvedTheme {
    /// The original built-in dark theme. Unlike the named presets it leaves
    /// `base_bg` as `None`, so the terminal's own background shows through.
    pub(crate) const DARK: ResolvedTheme = ResolvedTheme {
        accent: Color::Cyan, // cian-blue, kept consistent across the app
        status_bg: rgb(0x282837),
        selected_bg: rgb(0x3c3c5a),
        visual_bg: rgb(0x503c1e),
        mark_fg: Color::Yellow,
        base_bg: None,
        dim: rgb(0x82829b),
        border: Color::DarkGray,
        popup_bg: rgb(0x181822),
        file: FilePalette {
            directory: rgb(0x60a5fa),
            code: rgb(0xfacc15),
            config: rgb(0x94bed2),
            document: rgb(0xe2e2ec),
            image: rgb(0xd882dc),
            media: rgb(0x78c8be),
            archive: rgb(0xf08278),
            executable: rgb(0x7ed982),
            muted: rgb(0x808094),
            plain: rgb(0xcdcdda),
        },
    };

    // Ethan Schoonover's Solarized, light and dark. Popups stay on Solarized's
    // dark base02 so their light body text reads over the light surface.
    pub(crate) const SOLARIZED_LIGHT: ResolvedTheme = from_spec(cian_core::theme::SOLARIZED_LIGHT);
    /// The palette a desktop file manager is drawn in: near-white, one strong
    /// blue for the selection, and greys quiet enough that the eye goes to the
    /// names.
    pub(crate) const FINDER: ResolvedTheme = from_spec(cian_core::theme::FINDER);
    pub(crate) const SOLARIZED_DARK: ResolvedTheme = from_spec(cian_core::theme::SOLARIZED_DARK);
    pub(crate) const DRACULA: ResolvedTheme = from_spec(cian_core::theme::DRACULA);
    pub(crate) const NORD: ResolvedTheme = from_spec(cian_core::theme::NORD);
    pub(crate) const GRUVBOX_DARK: ResolvedTheme = from_spec(cian_core::theme::GRUVBOX_DARK);
    pub(crate) const GRUVBOX_LIGHT: ResolvedTheme = from_spec(cian_core::theme::GRUVBOX_LIGHT);
    pub(crate) const TOKYO_NIGHT: ResolvedTheme = from_spec(cian_core::theme::TOKYO_NIGHT);
    pub(crate) const CATPPUCCIN_MOCHA: ResolvedTheme = from_spec(cian_core::theme::CATPPUCCIN_MOCHA);
    pub(crate) const CATPPUCCIN_LATTE: ResolvedTheme = from_spec(cian_core::theme::CATPPUCCIN_LATTE);
    pub(crate) const MONOKAI: ResolvedTheme = from_spec(cian_core::theme::MONOKAI);
    pub(crate) const ONE_DARK: ResolvedTheme = from_spec(cian_core::theme::ONE_DARK);
    pub(crate) const GITHUB_LIGHT: ResolvedTheme = from_spec(cian_core::theme::GITHUB_LIGHT);
    /// Monokai Pro — the paid Monokai's own palette, not the classic one
    /// above: warmer greys, and the amber that everything is keyed to.
    pub(crate) const MONOKAI_PRO: ResolvedTheme = from_spec(cian_core::theme::MONOKAI_PRO);
    /// Ayu Dark — near-black with one amber accent, which is the whole idea.
    pub(crate) const AYU_DARK: ResolvedTheme = from_spec(cian_core::theme::AYU_DARK);
    /// Ayu Light — the same palette on paper.
    pub(crate) const AYU_LIGHT: ResolvedTheme = from_spec(cian_core::theme::AYU_LIGHT);
    /// Bluloco Light — a light theme with saturated syntax rather than pastel.
    pub(crate) const BLULOCO_LIGHT: ResolvedTheme = from_spec(cian_core::theme::BLULOCO_LIGHT);
    /// Bearded — the family's dark, vivid look: a near-black violet ground
    /// with pink, amethyst and teal on it. Approximated from the family's
    /// signature colours rather than copied from one variant, since Bearded
    /// ships dozens; `cian.set_theme{...}` takes exact values if you have a
    /// particular one in mind.
    pub(crate) const BEARDED: ResolvedTheme = from_spec(cian_core::theme::BEARDED);
}

/// The named presets, in gallery order. `default` is the transparent-background
/// built-in; the rest paint their own surface.
pub(crate) const THEME_NAMES: &[&str] = &[
    "default",
    "solarized-light",
    "solarized-dark",
    "dracula",
    "nord",
    "gruvbox-dark",
    "gruvbox-light",
    "tokyo-night",
    "catppuccin-mocha",
    "catppuccin-latte",
    "monokai",
    "one-dark",
    "github-light",
    "monokai-pro",
    "ayu-dark",
    "ayu-light",
    "bluloco-light",
    "bearded",
];

/// Process-wide active theme. Unlike the old set-once global this is swappable
/// so `:theme` can change the look live; the stateless draw helpers read it
/// through [`theme`] without threading a palette through every call. Reads take
/// a copy — `ResolvedTheme` is `Copy` and small.
static THEME: RwLock<ResolvedTheme> = RwLock::new(ResolvedTheme::DARK);

pub(crate) fn theme() -> ResolvedTheme {
    *THEME.read().unwrap_or_else(|e| e.into_inner())
}

/// Swap the active theme (from `:theme`, the picker preview, or `:reload`).
pub(crate) fn set_theme(t: ResolvedTheme) {
    let mut w = THEME.write().unwrap_or_else(|e| e.into_inner());
    if *w != t {
        THEME_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    *w = t;
}

/// Bumped whenever the theme actually changes.
///
/// Anything that caches *styles* rather than recomputing them each frame — the
/// Markdown preview's grid, the syntax highlighter — has to know when the
/// colours underneath it moved. Without this, a preview opened on a light
/// theme kept its near-black text after `:theme` switched to a dark one, and
/// the page went black on black.
static THEME_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn theme_generation() -> u64 {
    THEME_GEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// A concrete surface colour that follows the theme's light/dark identity —
/// the theme's own `base_bg` when it paints one (light themes go light), else
/// the dark popup background. Surfaces that want to honour a light theme (the
/// right-click menu, the F3 viewer) use this with `readable_on` for their text,
/// instead of the always-dark `popup_bg`.
pub(crate) fn surface() -> Color {
    let t = theme();
    t.base_bg.unwrap_or(t.popup_bg)
}

/// Which corner glyphs the borders use. Set once at startup; see
/// [`resolve_border_type`].
static BORDERS: OnceLock<BorderType> = OnceLock::new();

pub(crate) fn border_type() -> BorderType {
    *BORDERS.get_or_init(|| resolve_border_type(None))
}

/// Whether Nerd Font glyphs may be used (file icons, branch/disk symbols). Set
/// once at startup from `cian.set_option("nerd_fonts", …)`; defaults to true.
static NERD: OnceLock<bool> = OnceLock::new();

pub(crate) fn nerd_fonts() -> bool {
    *NERD.get_or_init(|| true)
}

/// Pick rounded or square corners.
///
/// Rounded corners are `╭╮╯╰` (U+256D–U+2570), which plenty of console fonts —
/// Consolas and Lucida Console among them — simply do not contain, while the
/// straight `─│` (U+2500, U+2502) are in almost all of them. Windows then
/// font-links just the corners to some other face, whose metrics differ, and
/// the frame looks a few pixels out at each corner while its sides stay put.
///
/// So: square corners in the legacy Windows console, rounded where the
/// terminal is known to cope, and an explicit `borders` option to override.
pub(crate) fn resolve_border_type(configured: Option<&str>) -> BorderType {
    match configured.map(|s| s.trim().to_lowercase()).as_deref() {
        Some("plain") | Some("square") => return BorderType::Plain,
        Some("rounded") => return BorderType::Rounded,
        _ => {}
    }
    if cfg!(windows) && !modern_terminal() {
        BorderType::Plain
    } else {
        BorderType::Rounded
    }
}

/// Whether the host can be trusted with the glyphs cian would rather use.
///
/// A window can, always: the font is cian's own and it is a Nerd Font. A
/// terminal can if it says which one it is — the legacy Windows console sets
/// none of these.
pub(crate) fn modern_terminal() -> bool {
    std::env::var_os("WT_SESSION").is_some()
        || std::env::var_os("WEZTERM_PANE").is_some()
        || std::env::var_os("TERM_PROGRAM").is_some()
}

/// Interface language for the key manual / help text. Japanese is the default;
/// `cian.set_option("lang", "en")` switches to English.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Ja,
    En,
}

impl Lang {
    /// From the `lang` option. Japanese unless English is asked for: cian is
    /// written in Japanese first, and an unset `lang` should give the people
    /// it was written for their own language without a config file. (The Lua
    /// layer already rejects values other than "ja"/"en".)
    pub(crate) fn from_opt(opt: Option<&str>) -> Lang {
        match opt {
            Some("en") => Lang::En,
            _ => Lang::Ja,
        }
    }

    /// Toggle to the other language.
    pub(crate) fn toggled(self) -> Lang {
        match self {
            Lang::En => Lang::Ja,
            Lang::Ja => Lang::En,
        }
    }
}

/// Pick the English or Japanese form of a fixed UI string.
///
/// # How cian talks
///
/// Every string that reaches a person goes through here, so the house style
/// belongs here too. It was arrived at by measuring what the six hundred
/// existing messages already did and settling the exceptions, not by decree.
///
/// * **English begins lower-case**, unless the first word is a name: `nothing
///   to operate on`, but `AI returned no command`. A terminal tool's voice,
///   the same as `ls` and `git`.
/// * **Japanese is 敬体** — 「〜ます」「〜ません」. Never 常体.
/// * **No full stop at the end**, in either language. Between two sentences,
///   yes: 「未保存の変更があります。Ctrl+S で保存できます」, `unsaved changes.
///   Ctrl+S saves` — the reader needs the break, but the line does not need a
///   stop it will never be followed past. A test holds both languages to this.
/// * **Two sentences, not a dash.** State what happened, then what can be done
///   about it. `unsaved changes — Ctrl+S saves` reads as one breathless
///   thought; `unsaved changes. Ctrl+S saves` is two clear ones.
/// * **Never "for now", "not yet", "temporarily".** A limit is a fact about
///   the tool, not an apology for it: `archives are read-only. copy extracts.`
///   Saying nothing at all about a limit is worse — silence reads as a bug.
pub(crate) fn tr(lang: Lang, en: &'static str, ja: &'static str) -> &'static str {
    match lang {
        Lang::En => en,
        Lang::Ja => ja,
    }
}

/// Localize the known progress-operation labels (`start_op`'s first argument).
/// Anything unrecognised (e.g. a directory path) is shown unchanged.
pub(crate) fn tr_op_label(lang: Lang, label: &str) -> String {
    if lang == Lang::En {
        return label.to_string();
    }
    match label {
        "copying" => "コピー中",
        "moving" => "移動中",
        "uploading" => "アップロード中",
        "downloading" => "ダウンロード中",
        "hashing" => "チェックサム計算中",
        "elevating" => "管理者権限で実行中",
        "comparing" => "比較中",
        other => return other.to_string(),
    }
    .to_string()
}

/// The "... and N more" overflow line, localized.
pub(crate) fn tr_count(lang: Lang, more: usize) -> String {
    match lang {
        Lang::En => format!("  ... and {} more", more),
        Lang::Ja => format!("  ... 他 {} 件", more),
    }
}

/// Remappable normal-mode actions. Keys the user binds via `cian.set_keymap`
/// resolve to one of these; the default key handling is otherwise untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    CursorDown,
    CursorUp,
    CursorTop,
    CursorBottom,
    PageUp,
    PageDown,
    Parent,
    EnterDir,
    Quit,
    Search,
    SearchNext,
    SearchPrev,
    History,
    Shortcuts,
    Copy,
    Move,
    /// Paste the file clipboard (Windows-style; also on Ctrl+V).
    Paste,
    /// Cut the selection to the file clipboard (also on Ctrl+X).
    Cut,
    Delete,
    Rename,
    NewFile,
    NewDir,
    OpenOther,
    OpenOtherTab,
    /// Make the active pane show the other pane's directory (pull).
    SyncFromOther,
    /// Make the other pane show the active pane's directory (push).
    SyncToOther,
    OpenExternal,
    CopyPath,
    CopyFileRef,
    MarkDown,
    MarkUp,
    InvertMarks,
    /// Mark every file in this listing — or, in the viewer, select the whole
    /// file. Which of the two is simply which is in front of you.
    MarkAll,
    Visual,
    Command,
    Filter,
    FindRecursive,
    GrepRecursive,
    Sort,
    JumpPath,
    View,
    Diff,
    Refresh,
    Menu,
    Ssh,
    NewTab,
    CloseTab,
    Manual,
    /// Bound to a key to disable it — the key does nothing, shadowing whatever
    /// default it would otherwise trigger.
    Nop,
}

/// Map a Lua action name to an [`Action`]. Unknown names are reported as
/// config errors rather than silently ignored.
pub(crate) fn action_from_name(name: &str) -> Option<Action> {
    Some(match name {
        "cursor_down" => Action::CursorDown,
        "cursor_up" => Action::CursorUp,
        "cursor_top" => Action::CursorTop,
        "cursor_bottom" => Action::CursorBottom,
        "page_up" => Action::PageUp,
        "page_down" => Action::PageDown,
        "parent" => Action::Parent,
        "enter" => Action::EnterDir,
        "quit" => Action::Quit,
        "search" => Action::Search,
        "search_next" => Action::SearchNext,
        "search_prev" => Action::SearchPrev,
        "history" => Action::History,
        "shortcuts" => Action::Shortcuts,
        "copy" => Action::Copy,
        "move" => Action::Move,
        "paste" => Action::Paste,
        "cut" => Action::Cut,
        "delete" => Action::Delete,
        "rename" => Action::Rename,
        "new_file" => Action::NewFile,
        "new_dir" => Action::NewDir,
        "open_other" => Action::OpenOther,
        "open_other_tab" => Action::OpenOtherTab,
        "sync_from_other" => Action::SyncFromOther,
        "sync_to_other" => Action::SyncToOther,
        "open_external" => Action::OpenExternal,
        "copy_path" => Action::CopyPath,
        "copy_file_ref" => Action::CopyFileRef,
        "mark_down" => Action::MarkDown,
        "mark_up" => Action::MarkUp,
        "invert_marks" => Action::InvertMarks,
        "mark_all" | "select_all" => Action::MarkAll,
        "visual" => Action::Visual,
        "command" => Action::Command,
        "filter" => Action::Filter,
        "find_recursive" => Action::FindRecursive,
        "grep_recursive" => Action::GrepRecursive,
        "sort" => Action::Sort,
        "jump_path" => Action::JumpPath,
        "view" => Action::View,
        "diff" => Action::Diff,
        "refresh" => Action::Refresh,
        "menu" => Action::Menu,
        "ssh" => Action::Ssh,
        "new_tab" => Action::NewTab,
        "close_tab" => Action::CloseTab,
        "manual" => Action::Manual,
        "none" | "nop" | "unbind" => Action::Nop,
        _ => return None,
    })
}

/// Parse a key spec from `cian.set_keymap` — `"x"`, `"alt+g"`, `"ctrl+f"`,
/// `"shift+s"` — into the character and the modifiers to match on.
///
/// Shift is folded into the character rather than kept as a modifier: a
/// terminal may or may not report Shift alongside an uppercase letter, and the
/// uppercase letter already says everything the binding needs. Only Ctrl and
/// Alt survive as modifiers, which are the two a terminal reports reliably.
pub(crate) fn parse_key_spec(spec: &str) -> Option<(char, crossterm::event::KeyModifiers)> {
    use crossterm::event::KeyModifiers;
    let spec = spec.trim();
    let mut parts: Vec<&str> = spec.split('+').collect();
    let key = parts.pop()?;
    let mut c = key.chars().next()?;
    if key.chars().count() != 1 {
        return None;
    }
    let mut mods = KeyModifiers::NONE;
    for m in parts {
        match m.trim().to_lowercase().as_str() {
            "ctrl" | "control" | "c" => mods |= KeyModifiers::CONTROL,
            "alt" | "opt" | "option" | "meta" | "m" => mods |= KeyModifiers::ALT,
            "shift" | "s" => c = c.to_ascii_uppercase(),
            _ => return None,
        }
    }
    Some((c, mods))
}

/// Parse a user color spec: `#rrggbb`, `r,g,b`, or a named color.
pub(crate) fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    if s.contains(',') {
        let parts: Vec<&str> = s.split(',').map(|x| x.trim()).collect();
        if parts.len() == 3 {
            let r = parts[0].parse::<u8>().ok()?;
            let g = parts[1].parse::<u8>().ok()?;
            let b = parts[2].parse::<u8>().ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    match s.to_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "lightred" => Some(Color::LightRed),
        "lightgreen" => Some(Color::LightGreen),
        "lightyellow" => Some(Color::LightYellow),
        "lightblue" => Some(Color::LightBlue),
        "lightmagenta" => Some(Color::LightMagenta),
        "lightcyan" => Some(Color::LightCyan),
        _ => None,
    }
}

/// Resolve a Lua [`Theme`] into a concrete palette, collecting any invalid
/// color specs as human-readable errors (the default is kept for those).
/// Named palettes selectable with `cian.set_theme "<name>"`.
pub(crate) fn theme_preset(name: &str) -> Option<ResolvedTheme> {
    Some(match name.trim().to_lowercase().replace([' ', '_'], "-").as_str() {
        "default" | "dark" => ResolvedTheme::DARK,
        "solarized-light" | "solarized" => ResolvedTheme::SOLARIZED_LIGHT,
        "solarized-dark" => ResolvedTheme::SOLARIZED_DARK,
        "dracula" => ResolvedTheme::DRACULA,
        "nord" => ResolvedTheme::NORD,
        "gruvbox-dark" | "gruvbox" => ResolvedTheme::GRUVBOX_DARK,
        "gruvbox-light" => ResolvedTheme::GRUVBOX_LIGHT,
        "tokyo-night" | "tokyonight" => ResolvedTheme::TOKYO_NIGHT,
        "catppuccin-mocha" | "catppuccin" | "mocha" => ResolvedTheme::CATPPUCCIN_MOCHA,
        "catppuccin-latte" | "latte" => ResolvedTheme::CATPPUCCIN_LATTE,
        "monokai" => ResolvedTheme::MONOKAI,
        "one-dark" | "onedark" => ResolvedTheme::ONE_DARK,
        "github-light" | "github" => ResolvedTheme::GITHUB_LIGHT,
        "monokai-pro" | "monokaipro" => ResolvedTheme::MONOKAI_PRO,
        "ayu-dark" | "ayu" => ResolvedTheme::AYU_DARK,
        "ayu-light" => ResolvedTheme::AYU_LIGHT,
        "bluloco-light" | "bluloco" => ResolvedTheme::BLULOCO_LIGHT,
        "bearded" | "bearded-theme" => ResolvedTheme::BEARDED,
        "finder" => ResolvedTheme::FINDER,
        _ => return None,
    })
}

/// The preset name whose palette matches `t`, if any (so the picker and status
/// bar can name the active theme). Compares by value since presets are `Copy`.
pub(crate) fn theme_name_of(t: &ResolvedTheme) -> Option<&'static str> {
    THEME_NAMES.iter().copied().find(|n| theme_preset(n).as_ref() == Some(t))
}

pub(crate) fn resolve_theme(t: &cian_lua::Theme) -> (ResolvedTheme, Vec<String>) {
    let mut errors = Vec::new();
    // Start from the named preset if one was chosen, else the dark default.
    let mut c = match &t.preset {
        Some(name) => theme_preset(name).unwrap_or_else(|| {
            errors.push(format!(
                "theme.preset: unknown preset {:?} (try \"solarized-light\")",
                name
            ));
            ResolvedTheme::default()
        }),
        None => ResolvedTheme::default(),
    };
    let mut apply = |spec: &Option<String>, slot: &mut Color, label: &str| {
        if let Some(s) = spec {
            match parse_color(s) {
                Some(col) => *slot = col,
                None => errors.push(format!("theme.{}: invalid color {:?}", label, s)),
            }
        }
    };
    apply(&t.accent, &mut c.accent, "accent");
    apply(&t.status_bg, &mut c.status_bg, "status_bg");
    apply(&t.selected_bg, &mut c.selected_bg, "selected_bg");
    apply(&t.visual_bg, &mut c.visual_bg, "visual_bg");
    apply(&t.mark_fg, &mut c.mark_fg, "mark_fg");
    (c, errors)
}

/// Resolve and install the theme + border style into the process-wide statics
/// (call once, before drawing). Returns the non-fatal theme errors to report.
pub(crate) fn install(theme: &cian_lua::Theme, borders: Option<&str>, nerd: bool) -> Vec<String> {
    // Did anyone actually ask for these colours, or are they the ones cian
    let (resolved, errs) = resolve_theme(theme);
    set_theme(resolved);
    let _ = BORDERS.set(resolve_border_type(borders));
    let _ = NERD.set(nerd);
    errs
}
