use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Result;
use cian_core::ops::{self, Conflict, OpReport};
use cian_core::Pane;
use cian_lua::Config;
use cian_pty::PtySession;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde::{Deserialize, Serialize};
use tui_term::widget::PseudoTerminal;

/// Resolved colour palette. Defaults match the original built-in theme; a
/// `~/.config/cian/init.lua` calling `cian.set_theme{...}` overrides any field.
#[derive(Debug, Clone, Copy)]
struct ResolvedTheme {
    accent: Color,
    status_bg: Color,
    selected_bg: Color,
    visual_bg: Color,
    mark_fg: Color,
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        Self {
            accent: Color::Cyan, // cian-blue, kept consistent across the app
            status_bg: Color::Rgb(40, 40, 55),
            selected_bg: Color::Rgb(60, 60, 90),
            visual_bg: Color::Rgb(80, 60, 30),
            mark_fg: Color::Yellow,
        }
    }
}

/// Process-wide resolved theme. Set once at startup from the Lua config so the
/// stateless draw helpers can read it without threading it through every call.
static THEME: OnceLock<ResolvedTheme> = OnceLock::new();

fn theme() -> &'static ResolvedTheme {
    THEME.get_or_init(ResolvedTheme::default)
}

/// Remappable normal-mode actions. Keys the user binds via `cian.set_keymap`
/// resolve to one of these; the default key handling is otherwise untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    CursorDown,
    CursorUp,
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
    Delete,
    Rename,
    NewFile,
    NewDir,
    OpenOther,
    OpenOtherTab,
    OpenExternal,
    CopyPath,
    CopyFileRef,
    MarkDown,
    MarkUp,
    InvertMarks,
    Visual,
    Command,
}

/// Map a Lua action name to an [`Action`]. Unknown names are reported as
/// config errors rather than silently ignored.
fn action_from_name(name: &str) -> Option<Action> {
    Some(match name {
        "cursor_down" => Action::CursorDown,
        "cursor_up" => Action::CursorUp,
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
        "delete" => Action::Delete,
        "rename" => Action::Rename,
        "new_file" => Action::NewFile,
        "new_dir" => Action::NewDir,
        "open_other" => Action::OpenOther,
        "open_other_tab" => Action::OpenOtherTab,
        "open_external" => Action::OpenExternal,
        "copy_path" => Action::CopyPath,
        "copy_file_ref" => Action::CopyFileRef,
        "mark_down" => Action::MarkDown,
        "mark_up" => Action::MarkUp,
        "invert_marks" => Action::InvertMarks,
        "visual" => Action::Visual,
        "command" => Action::Command,
        _ => return None,
    })
}

/// Parse a user colour spec: `#rrggbb`, `r,g,b`, or a named colour.
fn parse_color(s: &str) -> Option<Color> {
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
/// colour specs as human-readable errors (the default is kept for those).
fn resolve_theme(t: &cian_lua::Theme) -> (ResolvedTheme, Vec<String>) {
    let mut c = ResolvedTheme::default();
    let mut errors = Vec::new();
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPane {
    Left,
    Right,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Visual,
    Search,
    Command,
    Shell,
}


pub struct PaneTabs {
    pub tabs: Vec<Pane>,
    pub active: usize,
}

impl PaneTabs {
    pub fn single(p: Pane) -> Self {
        Self { tabs: vec![p], active: 0 }
    }
    pub fn active_ref(&self) -> &Pane { &self.tabs[self.active] }
    pub fn active_mut(&mut self) -> &mut Pane { &mut self.tabs[self.active] }
    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }
    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
        }
    }
    pub fn select(&mut self, idx: usize) {
        if idx < self.tabs.len() { self.active = idx; }
    }
    pub fn add_clone(&mut self) -> Result<()> {
        let cwd = self.active_ref().cwd.clone();
        self.tabs.push(Pane::new(cwd)?);
        self.active = self.tabs.len() - 1;
        Ok(())
    }
    pub fn close_active(&mut self) {
        if self.tabs.len() > 1 {
            self.tabs.remove(self.active);
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len() - 1;
            }
        }
    }
}

/// How the panes inside one shell tab are arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplitDir {
    /// Panes side by side (vertical dividers).
    LeftRight,
    /// Panes stacked (horizontal dividers).
    TopBottom,
}

/// A node in a shell tab's split tree: a leaf PTY pane, or a binary split of
/// two child nodes (referenced by slab index).
enum Node {
    Leaf(PtySession),
    Split { dir: SplitDir, first: usize, second: usize },
}

/// One shell tab: a binary tree of PTY panes supporting nested splits. Nodes
/// live in a slab indexed by `usize`; `None` slots are free for reuse.
struct ShellTab {
    nodes: Vec<Option<Node>>,
    root: usize,
    /// Index of the active leaf node.
    active: usize,
}

impl ShellTab {
    fn new(session: PtySession) -> Self {
        Self { nodes: vec![Some(Node::Leaf(session))], root: 0, active: 0 }
    }

    fn alloc(&mut self, node: Node) -> usize {
        if let Some(i) = self.nodes.iter().position(|n| n.is_none()) {
            self.nodes[i] = Some(node);
            i
        } else {
            self.nodes.push(Some(node));
            self.nodes.len() - 1
        }
    }

    fn active_pane(&self) -> Option<&PtySession> {
        match self.nodes.get(self.active).and_then(|n| n.as_ref()) {
            Some(Node::Leaf(s)) => Some(s),
            _ => None,
        }
    }
    fn active_pane_mut(&mut self) -> Option<&mut PtySession> {
        match self.nodes.get_mut(self.active).and_then(|n| n.as_mut()) {
            Some(Node::Leaf(s)) => Some(s),
            _ => None,
        }
    }

    fn collect_leaves(&self, i: usize, out: &mut Vec<usize>) {
        match self.nodes.get(i).and_then(|n| n.as_ref()) {
            Some(Node::Leaf(_)) => out.push(i),
            Some(Node::Split { first, second, .. }) => {
                self.collect_leaves(*first, out);
                self.collect_leaves(*second, out);
            }
            None => {}
        }
    }
    fn leaves(&self) -> Vec<usize> {
        let mut v = Vec::new();
        if self.nodes.get(self.root).map(|n| n.is_some()).unwrap_or(false) {
            self.collect_leaves(self.root, &mut v);
        }
        v
    }

    fn first_leaf(&self, i: usize) -> usize {
        match self.nodes.get(i).and_then(|n| n.as_ref()) {
            Some(Node::Split { first, .. }) => self.first_leaf(*first),
            _ => i,
        }
    }

    fn parent_of(&self, child: usize) -> Option<(usize, bool)> {
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(Node::Split { first, second, .. }) = n {
                if *first == child {
                    return Some((i, true));
                }
                if *second == child {
                    return Some((i, false));
                }
            }
        }
        None
    }

    /// Split the active leaf into (old, new) along `dir`; new becomes active.
    fn split(&mut self, dir: SplitDir, new_session: PtySession) {
        let old = self.active;
        if !matches!(self.nodes.get(old).and_then(|n| n.as_ref()), Some(Node::Leaf(_))) {
            return;
        }
        let new_leaf = self.alloc(Node::Leaf(new_session));
        let split_idx = self.alloc(Node::Split { dir, first: old, second: new_leaf });
        if old == self.root {
            self.root = split_idx;
        } else if let Some((p, is_first)) = self.parent_of(old) {
            if let Some(Node::Split { first, second, .. }) = self.nodes[p].as_mut() {
                if is_first {
                    *first = split_idx;
                } else {
                    *second = split_idx;
                }
            }
        }
        self.active = new_leaf;
    }

    fn focus_next(&mut self, forward: bool) {
        let leaves = self.leaves();
        if leaves.is_empty() {
            return;
        }
        let pos = leaves.iter().position(|&l| l == self.active).unwrap_or(0);
        let n = leaves.len();
        let np = if forward { (pos + 1) % n } else { (pos + n - 1) % n };
        self.active = leaves[np];
    }

    /// Close the active leaf; its sibling takes the parent's place. Returns true
    /// if the tab is now empty.
    fn close_active(&mut self) -> bool {
        let leaf = self.active;
        if !matches!(self.nodes.get(leaf).and_then(|n| n.as_ref()), Some(Node::Leaf(_))) {
            return self.leaves().is_empty();
        }
        if leaf == self.root {
            self.nodes[leaf] = None;
            return true;
        }
        let (p, leaf_is_first) = match self.parent_of(leaf) {
            Some(x) => x,
            None => {
                self.nodes[leaf] = None;
                return self.leaves().is_empty();
            }
        };
        let sib = match self.nodes[p].as_ref() {
            Some(Node::Split { first, second, .. }) => {
                if leaf_is_first { *second } else { *first }
            }
            _ => return false,
        };
        if p == self.root {
            self.root = sib;
        } else if let Some((gp, p_is_first)) = self.parent_of(p) {
            if let Some(Node::Split { first, second, .. }) = self.nodes[gp].as_mut() {
                if p_is_first {
                    *first = sib;
                } else {
                    *second = sib;
                }
            }
        }
        self.nodes[leaf] = None;
        self.nodes[p] = None;
        self.active = self.first_leaf(sib);
        false
    }

    fn for_each_leaf_mut(&mut self, f: &mut dyn FnMut(&mut PtySession)) {
        for n in self.nodes.iter_mut() {
            if let Some(Node::Leaf(s)) = n {
                f(s);
            }
        }
    }
}

/// The bottom shell panel: a set of tabs, each holding one or more split panes.
///
/// The first tab is spawned lazily on first focus.
pub struct ShellPane {
    tabs: Vec<ShellTab>,
    active: usize,
    /// Toggle (Shift+F12): show only the active split pane, filling the panel.
    zoom_pane: bool,
    /// Inner size of the whole shell panel, refreshed each frame; used as the
    /// initial size for newly-spawned panes before the next layout pass.
    rows: u16,
    cols: u16,
    shell_cmd: String,
    error: Option<String>,
}

impl ShellPane {
    fn new(shell_cmd: String) -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            zoom_pane: false,
            rows: 24,
            cols: 80,
            shell_cmd,
            error: None,
        }
    }

    fn toggle_pane_zoom(&mut self) {
        self.zoom_pane = !self.zoom_pane;
    }

    fn count(&self) -> usize {
        self.tabs.len()
    }

    fn active_tab(&self) -> Option<&ShellTab> {
        self.tabs.get(self.active)
    }

    fn active_session(&self) -> Option<&PtySession> {
        self.active_tab().and_then(|t| t.active_pane())
    }

    fn active_session_mut(&mut self) -> Option<&mut PtySession> {
        self.tabs.get_mut(self.active).and_then(|t| t.active_pane_mut())
    }

    fn spawn_session_in(&self, cwd: &Path) -> std::result::Result<PtySession, String> {
        PtySession::new(cwd, &self.shell_cmd, self.rows.max(1), self.cols.max(1))
            .map_err(|e| e.to_string())
    }

    /// Spawn the first tab if none exists yet (lazy start on first focus).
    fn ensure(&mut self, cwd: &Path) {
        if self.tabs.is_empty() {
            match self.spawn_session_in(cwd) {
                Ok(s) => {
                    self.tabs.push(ShellTab::new(s));
                    self.active = 0;
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    /// Open an additional shell tab.
    fn new_tab(&mut self, cwd: &Path) {
        match self.spawn_session_in(cwd) {
            Ok(s) => {
                self.tabs.push(ShellTab::new(s));
                self.active = self.tabs.len() - 1;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.zoom_pane = false;
    }

    /// Split the active tab's active pane in `dir`, spawning a new pane.
    fn split_active(&mut self, cwd: &Path, dir: SplitDir) {
        let session = match self.spawn_session_in(cwd) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.split(dir, session);
            self.error = None;
        }
        // A split must be visible, so leave single-pane zoom.
        self.zoom_pane = false;
    }

    fn next_pane(&mut self) {
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.focus_next(true);
        }
        self.zoom_pane = false;
    }

    fn prev_pane(&mut self) {
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.focus_next(false);
        }
        self.zoom_pane = false;
    }

    /// Close the active pane. If its tab becomes empty the tab is removed.
    /// Returns true if no tabs remain (caller should leave the shell).
    fn close_active_pane(&mut self) -> bool {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if tab.close_active() {
                self.tabs.remove(self.active);
                if self.active >= self.tabs.len() && self.active > 0 {
                    self.active -= 1;
                }
            }
        }
        self.zoom_pane = false;
        self.tabs.is_empty()
    }

    /// Switch to shell tab `i` (no-op if out of range).
    fn select(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.active = i;
            self.zoom_pane = false;
        }
    }

    /// Close the whole active tab. Returns true if no tabs remain.
    fn close_active(&mut self) -> bool {
        if self.active < self.tabs.len() {
            self.tabs.remove(self.active);
            if self.active >= self.tabs.len() && self.active > 0 {
                self.active -= 1;
            }
        }
        self.zoom_pane = false;
        self.tabs.is_empty()
    }

    /// Clear and report whether any pane in the active tab produced new output.
    fn take_active_tab_dirty(&mut self) -> bool {
        let mut dirty = false;
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.for_each_leaf_mut(&mut |p| {
                if p.take_dirty() {
                    dirty = true;
                }
            });
        }
        dirty
    }

    /// `(alternate_screen, application_cursor)` for the active pane.
    fn active_modes(&self) -> (bool, bool) {
        if let Some(s) = self.active_session() {
            if let Ok(p) = s.parser().lock() {
                let scr = p.screen();
                return (scr.alternate_screen(), scr.application_cursor());
            }
        }
        (false, false)
    }
}

#[derive(Debug, Clone)]
enum PendingOp {
    Copy,
    Move,
}

#[derive(Debug, Clone)]
enum Popup {
    None,
    ConfirmDelete { targets: Vec<PathBuf> },
    ConfirmTransfer { op: PendingOp, targets: Vec<PathBuf>, dest: PathBuf },
    TextInput { title: String, prompt: String, buffer: String, kind: InputKind },
    Notice { lines: Vec<String> },
    Search { buffer: String },
    History { entries: Vec<PathBuf>, cursor: usize },
    Shortcuts { entries: Vec<Shortcut>, cursor: usize },
    ConfirmQuit,
    ConfirmClose { target: CloseTarget },
}

/// What a close-confirmation popup will close when accepted.
#[derive(Debug, Clone, Copy)]
enum CloseTarget {
    /// The active split pane in the shell.
    ShellPane,
    /// The active tab of a file pane.
    FileTab(FocusedPane),
}

#[derive(Debug, Clone)]
enum InputKind {
    Rename { original: PathBuf },
    NewFile { parent: PathBuf },
    NewDir { parent: PathBuf },
    ShortcutName { editing_index: Option<usize> },
    ShortcutTarget { editing_index: Option<usize>, name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shortcut {
    pub name: String,
    pub target: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct ShortcutsFile {
    #[serde(default)]
    shortcuts: Vec<Shortcut>,
}

pub struct ShortcutStore {
    pub entries: Vec<Shortcut>,
    pub path: PathBuf,
}

impl ShortcutStore {
    fn default_path() -> PathBuf {
        home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("cian")
            .join("shortcuts.toml")
    }

    pub fn load_or_default() -> Self {
        let path = Self::default_path();
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| toml::from_str::<ShortcutsFile>(&s).ok())
            .map(|f| f.shortcuts)
            .unwrap_or_default();
        Self { entries, path }
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = ShortcutsFile { shortcuts: self.entries.clone() };
        let s = toml::to_string_pretty(&file)?;
        std::fs::write(&self.path, s)?;
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct LayoutRects {
    left: Rect,
    right: Rect,
    shell: Rect,
}

pub struct App {
    pub left: PaneTabs,
    pub right: PaneTabs,
    pub shell: ShellPane,
    pub focused: FocusedPane,
    pub mode: Mode,
    pub mask: String,
    pub command_buffer: String,
    pub message: Option<String>,
    pub last_file_pane: FocusedPane,
    pub should_quit: bool,
    pub visual_anchor: Option<usize>,
    pub clipboard_on_copy: bool,
    clipboard: Option<arboard::Clipboard>,
    popup: Popup,
    layout_rects: LayoutRects,
    last_search_query: Option<String>,
    pub shortcuts: ShortcutStore,
    pending_g: bool,
    /// When true, only the focused surface is drawn, filling the window.
    pub zoomed: bool,
    /// When CIAN_DEBUG_KEYS is set, show each shell keypress in the status bar.
    debug_keys: bool,
    config: Config,
    /// User keymap overrides: plain character keys (no Ctrl) the user bound via
    /// `cian.set_keymap`. Only contains entries the user set; everything else
    /// falls through to the built-in defaults.
    keymap: HashMap<char, Action>,
}

impl App {
    pub fn new(left: PathBuf, right: PathBuf, config: Config) -> Result<Self> {
        // Build the keymap from user overrides (invalid action names are
        // validated and reported separately in `run`).
        let mut keymap: HashMap<char, Action> = HashMap::new();
        for (c, name) in &config.keymaps {
            if let Some(a) = action_from_name(name) {
                keymap.insert(*c, a);
            }
        }
        let clipboard_on_copy = config.options.clipboard_on_copy.unwrap_or(true);
        let mask = config
            .options
            .mask
            .clone()
            .unwrap_or_else(|| "*.*".to_string());
        let shell_cmd = config
            .options
            .shell
            .clone()
            .unwrap_or_else(cian_pty::default_shell);
        Ok(Self {
            left: PaneTabs::single(Pane::new(left)?),
            right: PaneTabs::single(Pane::new(right)?),
            shell: ShellPane::new(shell_cmd),
            focused: FocusedPane::Left,
            mode: Mode::Normal,
            mask,
            command_buffer: String::new(),
            message: None,
            last_file_pane: FocusedPane::Left,
            should_quit: false,
            visual_anchor: None,
            clipboard_on_copy,
            clipboard: arboard::Clipboard::new().ok(),
            popup: Popup::None,
            layout_rects: LayoutRects::default(),
            last_search_query: None,
            shortcuts: ShortcutStore::load_or_default(),
            pending_g: false,
            zoomed: false,
            debug_keys: std::env::var("CIAN_DEBUG_KEYS").is_ok(),
            config,
            keymap,
        })
    }

    /// The directory a newly-spawned shell should start in: the cwd of the
    /// file pane we were last on.
    fn shell_cwd(&self) -> PathBuf {
        let tabs = match self.last_file_pane {
            FocusedPane::Right => &self.right,
            _ => &self.left,
        };
        tabs.active_ref().cwd.clone()
    }

    fn active_file_tabs(&self) -> Option<&PaneTabs> {
        match self.focused {
            FocusedPane::Left => Some(&self.left),
            FocusedPane::Right => Some(&self.right),
            FocusedPane::Shell => None,
        }
    }
    fn active_file_tabs_mut(&mut self) -> Option<&mut PaneTabs> {
        match self.focused {
            FocusedPane::Left => Some(&mut self.left),
            FocusedPane::Right => Some(&mut self.right),
            FocusedPane::Shell => None,
        }
    }
    fn active_pane(&self) -> Option<&Pane> { self.active_file_tabs().map(|t| t.active_ref()) }
    fn active_pane_mut(&mut self) -> Option<&mut Pane> {
        self.active_file_tabs_mut().map(|t| t.active_mut())
    }

    fn opposite_pane_cwd(&self) -> Option<PathBuf> {
        let other = match self.focused {
            FocusedPane::Left => &self.right,
            FocusedPane::Right => &self.left,
            FocusedPane::Shell => return None,
        };
        Some(other.active_ref().cwd.clone())
    }

    fn focus(&mut self, target: FocusedPane) {
        if matches!(self.focused, FocusedPane::Left | FocusedPane::Right) {
            self.last_file_pane = self.focused;
        }
        if target == FocusedPane::Shell {
            // Lazily start a shell in the directory we're coming from.
            let cwd = self
                .active_pane()
                .map(|p| p.cwd.clone())
                .unwrap_or_else(|| self.left.active_ref().cwd.clone());
            self.shell.ensure(&cwd);
        }
        self.focused = target;
        self.mode = match target {
            FocusedPane::Shell => Mode::Shell,
            _ => Mode::Normal,
        };
        self.visual_anchor = None;
    }

    fn focus_direction(&mut self, dir: char) {
        let next = match (self.focused, dir) {
            (FocusedPane::Left, 'l') => FocusedPane::Right,
            (FocusedPane::Right, 'h') => FocusedPane::Left,
            (FocusedPane::Left | FocusedPane::Right, 'j') => FocusedPane::Shell,
            // From shell: H and K both go left, L goes right.
            (FocusedPane::Shell, 'h') | (FocusedPane::Shell, 'k') => FocusedPane::Left,
            (FocusedPane::Shell, 'l') => FocusedPane::Right,
            _ => self.focused,
        };
        if next != self.focused {
            self.focus(next);
        }
    }

    fn run_command(&mut self) {
        let raw = self.command_buffer.trim().to_string();
        self.command_buffer.clear();
        self.mode = Mode::Normal;
        match raw.as_str() {
            "" => {}
            "q" | "quit" => self.should_quit = true,
            "shell" => self.focus(FocusedPane::Shell),
            other => self.message = Some(format!("unknown command: :{}", other)),
        }
    }

    fn open_in_other_pane(&mut self, new_tab: bool) -> Result<()> {
        let target = match self.active_pane().and_then(|p| p.selected()) {
            Some(e) if e.is_dir => e.path.clone(),
            _ => { self.message = Some("not a directory".into()); return Ok(()); }
        };
        let other = match self.focused {
            FocusedPane::Left => &mut self.right,
            FocusedPane::Right => &mut self.left,
            FocusedPane::Shell => return Ok(()),
        };
        if new_tab {
            let pane = Pane::new(target.clone())?;
            other.tabs.push(pane);
            other.active = other.tabs.len() - 1;
        } else {
            other.active_mut().jump_to(target.clone())?;
        }
        // focus stays on the active pane
        self.message = Some(format!(
            "{} other pane → {}",
            if new_tab { "new tab in" } else { "opened in" },
            target.display()
        ));
        Ok(())
    }

    fn open_externally(&mut self) {
        let Some(pane) = self.active_pane() else { return };
        let Some(entry) = pane.selected() else { return };
        let path = entry.path.clone();
        // Extension-dispatch execution: if the user registered an `on_open`
        // handler for this extension in init.lua, run it instead of the OS open.
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !ext.is_empty() && self.config.has_ext_open(&ext) {
            match self.config.run_ext_open(&ext, &path) {
                Some(Ok(())) => {
                    self.message = Some(format!("opened via lua: {}", path.display()));
                    return;
                }
                Some(Err(e)) => {
                    self.message = Some(format!("on_open({}) error: {}", ext, e));
                    return;
                }
                None => {}
            }
        }
        match os_open(&path) {
            Ok(()) => self.message = Some(format!("opened: {}", path.display())),
            Err(e) => self.message = Some(format!("open failed: {}", e)),
        }
    }

    fn push_clipboard(&mut self, paths: &[PathBuf]) {
        if !self.clipboard_on_copy { return; }
        let Some(cb) = self.clipboard.as_mut() else { return; };
        let text = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n");
        let _ = cb.set_text(text);
    }

    // ------- Visual mode -------
    fn visual_start(&mut self) {
        if let Some(p) = self.active_pane() {
            self.visual_anchor = Some(p.cursor);
            self.mode = Mode::Visual;
        }
    }
    fn visual_commit(&mut self) {
        let anchor = match self.visual_anchor.take() {
            Some(a) => a,
            None => { self.mode = Mode::Normal; return; }
        };
        if let Some(p) = self.active_pane_mut() {
            let cur = p.cursor;
            let (a, b) = if anchor <= cur { (anchor, cur) } else { (cur, anchor) };
            for i in a..=b { p.set_mark_at(i); }
        }
        self.mode = Mode::Normal;
    }
    fn visual_cancel_and_clear_all(&mut self) {
        self.visual_anchor = None;
        if let Some(p) = self.active_pane_mut() { p.clear_marks(); }
        self.mode = Mode::Normal;
    }

    // ------- Confirmation flows -------
    fn start_transfer(&mut self, op: PendingOp) {
        let Some(dest) = self.opposite_pane_cwd() else { return };
        let targets = match self.active_pane() {
            Some(p) => p.target_paths(),
            None => return,
        };
        if targets.is_empty() { self.message = Some("nothing to operate on".into()); return; }
        self.popup = Popup::ConfirmTransfer { op, targets, dest };
    }
    fn start_delete(&mut self) {
        let targets = match self.active_pane() {
            Some(p) => p.target_paths(),
            None => return,
        };
        if targets.is_empty() { self.message = Some("nothing to delete".into()); return; }
        self.popup = Popup::ConfirmDelete { targets };
    }
    fn start_rename(&mut self) {
        let Some(p) = self.active_pane() else { return };
        let Some(e) = p.selected() else { return };
        self.popup = Popup::TextInput {
            title: "rename".into(),
            prompt: "new name:".into(),
            buffer: e.name.clone(),
            kind: InputKind::Rename { original: e.path.clone() },
        };
    }
    fn start_new_file(&mut self) {
        let Some(p) = self.active_pane() else { return };
        self.popup = Popup::TextInput {
            title: "new file".into(),
            prompt: "name:".into(),
            buffer: String::new(),
            kind: InputKind::NewFile { parent: p.cwd.clone() },
        };
    }
    fn start_new_dir(&mut self) {
        let Some(p) = self.active_pane() else { return };
        self.popup = Popup::TextInput {
            title: "new directory".into(),
            prompt: "name:".into(),
            buffer: String::new(),
            kind: InputKind::NewDir { parent: p.cwd.clone() },
        };
    }

    // ------- Search -------
    fn start_search(&mut self) {
        self.popup = Popup::Search { buffer: String::new() };
        self.mode = Mode::Search;
    }

    fn finish_search(&mut self) {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let buffer = if let Popup::Search { buffer } = popup { buffer } else { return };
        self.mode = Mode::Normal;
        let q = buffer.trim().to_string();
        if q.is_empty() { return; }
        self.last_search_query = Some(q.clone());
        let ql = q.to_lowercase();
        if let Some(p) = self.active_pane_mut() {
            if let Some(i) = p.entries.iter().position(|e| e.name.to_lowercase().contains(&ql)) {
                p.cursor = i;
            } else {
                self.message = Some(format!("pattern not found: {}", q));
            }
        }
    }

    // ------- Shortcuts -------
    fn start_shortcuts(&mut self) {
        self.popup = Popup::Shortcuts {
            entries: self.shortcuts.entries.clone(),
            cursor: 0,
        };
    }

    fn start_shortcut_add(&mut self) {
        self.popup = Popup::TextInput {
            title: "new shortcut — name".into(),
            prompt: "name:".into(),
            buffer: String::new(),
            kind: InputKind::ShortcutName { editing_index: None },
        };
    }

    fn start_shortcut_edit(&mut self, idx: usize) {
        let Some(s) = self.shortcuts.entries.get(idx).cloned() else { return };
        self.popup = Popup::TextInput {
            title: "edit shortcut — name".into(),
            prompt: "name:".into(),
            buffer: s.name,
            kind: InputKind::ShortcutName { editing_index: Some(idx) },
        };
    }

    fn copy_paths_to_clipboard(&mut self) {
        let paths = match self.active_pane() {
            Some(p) => p.target_paths(),
            None => return,
        };
        if paths.is_empty() {
            self.message = Some("nothing to copy".into());
            return;
        }
        let Some(cb) = self.clipboard.as_mut() else {
            self.message = Some("clipboard unavailable".into());
            return;
        };
        let text = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n");
        match cb.set_text(text) {
            Ok(()) => self.message = Some(format!("◂ copied {} path(s) to clipboard", paths.len())),
            Err(e) => self.message = Some(format!("clipboard error: {}", e)),
        }
    }

    fn copy_file_refs_to_clipboard(&mut self) {
        let paths = match self.active_pane() {
            Some(p) => p.target_paths(),
            None => return,
        };
        if paths.is_empty() {
            self.message = Some("nothing to copy".into());
            return;
        }
        match os_clipboard_file_refs(&paths) {
            Ok(()) => self.message = Some(format!("◂ copied {} file ref(s) to clipboard", paths.len())),
            Err(e) => self.message = Some(format!("file-ref clipboard failed: {}", e)),
        }
    }

    fn copy_shortcut_target_to_clipboard(&mut self, idx: usize) {
        let Some(entry) = self.shortcuts.entries.get(idx).cloned() else { return };
        let Some(cb) = self.clipboard.as_mut() else {
            self.message = Some("clipboard unavailable".into());
            return;
        };
        match cb.set_text(entry.target.clone()) {
            Ok(()) => self.message = Some(format!("◂ copied: {}", truncate(&entry.target, 50))),
            Err(e) => self.message = Some(format!("clipboard error: {}", e)),
        }
    }

    fn execute_shortcut(&mut self, idx: usize) -> Result<()> {
        let Some(entry) = self.shortcuts.entries.get(idx).cloned() else { return Ok(()) };
        let target = entry.target.clone();

        // URL?
        if target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("file://")
        {
            let _ = os_open_string(&target);
            self.message = Some(format!("◂ {}", entry.name));
            return Ok(());
        }

        let path = expand_tilde(Path::new(&target));

        // macOS .app bundles are technically directories. Always hand them to
        // `open` so the app launches instead of cd-ing into the package.
        let is_app_bundle = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("app"))
            .unwrap_or(false);
        if is_app_bundle && path.exists() {
            match os_open(&path) {
                Ok(()) => self.message = Some(format!("◂ {}", entry.name)),
                Err(e) => self.message = Some(format!("shortcut failed: {}", e)),
            }
            return Ok(());
        }

        // Plain directory → navigate.
        if path.is_dir() {
            if let Some(p) = self.active_pane_mut() {
                p.jump_to(path)?;
            }
            self.message = Some(format!("◂ {}", entry.name));
            return Ok(());
        }

        // File or other existing entity → OS default.
        if path.exists() {
            let _ = os_open(&path);
            self.message = Some(format!("◂ {}", entry.name));
            return Ok(());
        }

        // Fallback: hand off the raw string to the OS opener (e.g. unknown protocols).
        match os_open_string(&target) {
            Ok(()) => self.message = Some(format!("◂ {}", entry.name)),
            Err(e) => self.message = Some(format!("shortcut failed: {}", e)),
        }
        Ok(())
    }

    // ------- History -------
    fn start_history(&mut self) {
        let entries = self.active_pane().map(|p| p.history.clone()).unwrap_or_default();
        if entries.is_empty() {
            self.message = Some("no history yet".into());
            return;
        }
        self.popup = Popup::History { entries, cursor: 0 };
    }

    fn finish_history(&mut self) -> Result<()> {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let (entries, cursor) = if let Popup::History { entries, cursor } = popup {
            (entries, cursor)
        } else { return Ok(()) };
        let Some(target) = entries.get(cursor).cloned() else { return Ok(()) };
        if let Some(p) = self.active_pane_mut() {
            p.jump_to(target)?;
        }
        Ok(())
    }

    // ------- Quit confirmation -------
    fn start_quit_confirm(&mut self) {
        self.popup = Popup::ConfirmQuit;
    }

    /// Perform a confirmed close (shell split pane or file tab).
    fn execute_close(&mut self, target: CloseTarget) {
        match target {
            CloseTarget::ShellPane => {
                if self.shell.close_active_pane() {
                    self.focus(self.last_file_pane);
                }
            }
            CloseTarget::FileTab(pane) => {
                let tabs = match pane {
                    FocusedPane::Left => &mut self.left,
                    FocusedPane::Right => &mut self.right,
                    FocusedPane::Shell => return,
                };
                tabs.close_active();
            }
        }
    }

    fn jump_to_next_match(&mut self, forward: bool) {
        let Some(query) = self.last_search_query.clone() else {
            self.message = Some("no previous search".into());
            return;
        };
        let ql = query.to_lowercase();
        let Some(p) = self.active_pane_mut() else { return };
        let n = p.entries.len();
        if n == 0 { return; }
        let start = p.cursor;
        let mut i = if forward { (start + 1) % n } else { (start + n - 1) % n };
        for _ in 0..n {
            if p.entries[i].name.to_lowercase().contains(&ql) {
                p.cursor = i;
                return;
            }
            i = if forward { (i + 1) % n } else { (i + n - 1) % n };
        }
        self.message = Some(format!("pattern not found: {}", query));
    }

    fn finish_transfer(&mut self, conflict: Conflict) -> Result<()> {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let Popup::ConfirmTransfer { op, targets, dest } = popup else { return Ok(()) };
        let report = match op {
            PendingOp::Copy => {
                self.push_clipboard(&targets);
                ops::copy_many(&targets, &dest, conflict)
            }
            PendingOp::Move => {
                self.push_clipboard(&targets);
                ops::move_many(&targets, &dest, conflict)
            }
        };
        if let Some(t) = self.active_file_tabs_mut() { let _ = t.active_mut().reload(); }
        let other_focus = match self.focused {
            FocusedPane::Left => FocusedPane::Right,
            FocusedPane::Right => FocusedPane::Left,
            FocusedPane::Shell => FocusedPane::Left,
        };
        let other = match other_focus {
            FocusedPane::Left => &mut self.left,
            FocusedPane::Right => &mut self.right,
            FocusedPane::Shell => &mut self.left,
        };
        let _ = other.active_mut().reload();
        self.show_op_report(&report);
        Ok(())
    }

    fn finish_delete(&mut self) -> Result<()> {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let Popup::ConfirmDelete { targets } = popup else { return Ok(()) };
        let report = ops::delete_many(&targets);
        if let Some(t) = self.active_file_tabs_mut() { let _ = t.active_mut().reload(); }
        if let Some(p) = self.active_pane_mut() { p.clear_marks(); }
        self.show_op_report(&report);
        Ok(())
    }

    fn show_op_report(&mut self, report: &OpReport) {
        if !report.errors.is_empty() {
            let mut lines = vec![format!(
                "{} ok · {} skipped · {} errors", report.ok, report.skipped, report.errors.len()
            )];
            lines.extend(report.errors.iter().take(8).cloned());
            if report.errors.len() > 8 {
                lines.push(format!("... and {} more", report.errors.len() - 8));
            }
            self.popup = Popup::Notice { lines };
        } else {
            self.message = Some(format!("done — {} ok · {} skipped", report.ok, report.skipped));
        }
    }

    fn finish_text_input(&mut self) -> Result<()> {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        let Popup::TextInput { buffer, kind, .. } = popup else { return Ok(()) };
        let name = buffer.trim().to_string();
        if name.is_empty() {
            self.message = Some("cancelled (empty name)".into());
            return Ok(());
        }
        let result = match &kind {
            InputKind::Rename { original } => {
                ops::rename_in_place(original, &name).map(|p| format!("renamed: {}", p.display()))
            }
            InputKind::NewFile { parent } => {
                ops::create_file(parent, &name).map(|p| format!("created: {}", p.display()))
            }
            InputKind::NewDir { parent } => {
                ops::create_dir(parent, &name).map(|p| format!("mkdir: {}", p.display()))
            }
            InputKind::ShortcutName { editing_index } => {
                // chain into the next step: target input
                let prev_target = editing_index
                    .and_then(|i| self.shortcuts.entries.get(i).map(|s| s.target.clone()))
                    .unwrap_or_default();
                self.popup = Popup::TextInput {
                    title: "shortcut — target".into(),
                    prompt: "URL / path (~ ok) / app:".into(),
                    buffer: prev_target,
                    kind: InputKind::ShortcutTarget {
                        editing_index: *editing_index,
                        name,
                    },
                };
                return Ok(());
            }
            InputKind::ShortcutTarget { editing_index, name: stored_name } => {
                let target = name; // `name` here is actually the trimmed buffer
                if target.is_empty() {
                    self.message = Some("cancelled (empty target)".into());
                    return Ok(());
                }
                let entry = Shortcut { name: stored_name.clone(), target };
                match editing_index {
                    Some(i) => {
                        if let Some(s) = self.shortcuts.entries.get_mut(*i) { *s = entry; }
                    }
                    None => self.shortcuts.entries.push(entry),
                }
                match self.shortcuts.save() {
                    Ok(()) => self.message = Some("shortcut saved".into()),
                    Err(e) => self.popup = Popup::Notice { lines: vec![format!("save failed: {}", e)] },
                }
                return Ok(());
            }
        };
        if let Some(t) = self.active_file_tabs_mut() { let _ = t.active_mut().reload(); }
        match result {
            Ok(msg) => self.message = Some(msg),
            Err(e) => self.popup = Popup::Notice { lines: vec![e.to_string()] },
        }
        Ok(())
    }

    // ------- Mouse -------
    fn handle_mouse(&mut self, ev: MouseEvent) {
        if !matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }
        // ignore clicks while a popup is open
        if !matches!(self.popup, Popup::None) {
            return;
        }
        let (col, row) = (ev.column, ev.row);
        let in_rect = |r: Rect| {
            r.width > 0 && r.height > 0
                && col >= r.x && col < r.x + r.width
                && row >= r.y && row < r.y + r.height
        };
        if in_rect(self.layout_rects.left) {
            self.focus(FocusedPane::Left);
        } else if in_rect(self.layout_rects.right) {
            self.focus(FocusedPane::Right);
        } else if in_rect(self.layout_rects.shell) {
            self.focus(FocusedPane::Shell);
        }
    }

    // ------- Key dispatch -------
    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if !matches!(self.popup, Popup::None) {
            return self.handle_popup_key(key);
        }
        // F12 toggles full-window zoom of the focused surface; Shift+F12 zooms
        // only the active split pane within the shell. While a full-screen app
        // runs in the shell, both are passed through to it.
        if key.code == KeyCode::F(12) {
            let shell_fullscreen =
                self.focused == FocusedPane::Shell && self.shell.active_modes().0;
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                if self.focused == FocusedPane::Shell && !shell_fullscreen {
                    self.shell.toggle_pane_zoom();
                    return Ok(());
                }
            } else if !shell_fullscreen {
                self.zoomed = !self.zoomed;
                return Ok(());
            }
        }
        if self.mode == Mode::Command {
            return self.handle_command_key(key);
        }
        if self.focused == FocusedPane::Shell {
            return self.handle_shell_key(key);
        }
        if self.mode == Mode::Visual {
            return self.handle_visual_key(key);
        }
        self.handle_normal_key(key)
    }

    fn handle_popup_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Popup::TextInput { buffer, .. } = &mut self.popup {
            match key.code {
                KeyCode::Esc => { self.popup = Popup::None; return Ok(()); }
                KeyCode::Enter => { return self.finish_text_input(); }
                KeyCode::Backspace => { buffer.pop(); return Ok(()); }
                KeyCode::Char(c) => { buffer.push(c); return Ok(()); }
                _ => return Ok(()),
            }
        }
        if let Popup::Search { buffer } = &mut self.popup {
            match key.code {
                KeyCode::Esc => {
                    self.popup = Popup::None;
                    self.mode = Mode::Normal;
                    return Ok(());
                }
                KeyCode::Enter => { self.finish_search(); return Ok(()); }
                KeyCode::Backspace => { buffer.pop(); return Ok(()); }
                KeyCode::Char(c) => { buffer.push(c); return Ok(()); }
                _ => return Ok(()),
            }
        }
        if let Popup::History { cursor, entries } = &mut self.popup {
            match key.code {
                KeyCode::Esc => { self.popup = Popup::None; return Ok(()); }
                KeyCode::Enter => { return self.finish_history(); }
                KeyCode::Char('j') | KeyCode::Down => {
                    if *cursor + 1 < entries.len() { *cursor += 1; }
                    return Ok(());
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if *cursor > 0 { *cursor -= 1; }
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }
        if let Popup::Shortcuts { cursor, entries } = &mut self.popup {
            match key.code {
                KeyCode::Esc => { self.popup = Popup::None; return Ok(()); }
                KeyCode::Enter => {
                    let idx = *cursor;
                    self.popup = Popup::None;
                    return self.execute_shortcut(idx);
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if !entries.is_empty() && *cursor + 1 < entries.len() { *cursor += 1; }
                    return Ok(());
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if *cursor > 0 { *cursor -= 1; }
                    return Ok(());
                }
                KeyCode::Char('a') => {
                    self.popup = Popup::None;
                    self.start_shortcut_add();
                    return Ok(());
                }
                KeyCode::Char('d') => {
                    if !entries.is_empty() {
                        let idx = *cursor;
                        entries.remove(idx);
                        self.shortcuts.entries = entries.clone();
                        let _ = self.shortcuts.save();
                        if *cursor >= entries.len() && *cursor > 0 { *cursor -= 1; }
                        if entries.is_empty() { self.popup = Popup::None; }
                    }
                    return Ok(());
                }
                KeyCode::Char('r') => {
                    if !entries.is_empty() {
                        let idx = *cursor;
                        self.popup = Popup::None;
                        self.start_shortcut_edit(idx);
                    }
                    return Ok(());
                }
                KeyCode::Char('p') => {
                    if !entries.is_empty() {
                        let idx = *cursor;
                        self.copy_shortcut_target_to_clipboard(idx);
                    }
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }
        if matches!(self.popup, Popup::ConfirmQuit) {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.popup = Popup::None;
                    self.should_quit = true;
                }
                KeyCode::Char('n') | KeyCode::Esc => { self.popup = Popup::None; }
                _ => {}
            }
            return Ok(());
        }
        if let Popup::ConfirmClose { target } = &self.popup {
            let target = *target;
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.popup = Popup::None;
                    self.execute_close(target);
                }
                KeyCode::Char('n') | KeyCode::Esc => { self.popup = Popup::None; }
                _ => {}
            }
            return Ok(());
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => { self.popup = Popup::None; Ok(()) }
            KeyCode::Char('y') => match &self.popup {
                Popup::ConfirmDelete { .. } => self.finish_delete(),
                Popup::ConfirmTransfer { .. } => self.finish_transfer(Conflict::Skip),
                Popup::Notice { .. } => { self.popup = Popup::None; Ok(()) }
                _ => Ok(()),
            },
            KeyCode::Char('a') => match &self.popup {
                Popup::ConfirmDelete { .. } => self.finish_delete(),
                Popup::ConfirmTransfer { .. } => self.finish_transfer(Conflict::Overwrite),
                _ => Ok(()),
            },
            KeyCode::Enter => {
                if matches!(self.popup, Popup::Notice { .. }) { self.popup = Popup::None; }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn handle_command_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => { self.command_buffer.clear(); self.mode = Mode::Normal; }
            KeyCode::Enter => self.run_command(),
            KeyCode::Backspace => { self.command_buffer.pop(); }
            KeyCode::Char(c) => self.command_buffer.push(c),
            _ => {}
        }
        Ok(())
    }

    fn handle_shell_key(&mut self, key: KeyEvent) -> Result<()> {
        let (alt_screen, app_cursor) = self.shell.active_modes();
        if self.debug_keys {
            self.message = Some(format!(
                "key={:?} mods={:?} alt_screen={}",
                key.code, key.modifiers, alt_screen
            ));
        }
        // Esc returns to the file pane — unless a full-screen app (alternate
        // screen) is running, in which case Esc belongs to that app (e.g. vim).
        if key.code == KeyCode::Esc && !alt_screen {
            self.focus(self.last_file_pane);
            return Ok(());
        }
        // Tab and split controls via F-keys — reserved only at a normal prompt.
        // The Ctrl modifier is swallowed before reaching the app on some setups
        // (IME/OS), so F-keys (independent escape sequences) are used instead.
        // Shift+F drives splits. While a full-screen app runs (alternate screen)
        // they pass through, like F12.
        if !alt_screen {
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            match key.code {
                // Pane navigation (parallels file-pane tab nav): Shift+F1/F2.
                KeyCode::F(1) if shift => {
                    self.shell.next_pane();
                    return Ok(());
                }
                KeyCode::F(2) if shift => {
                    self.shell.prev_pane();
                    return Ok(());
                }
                // Splits within the active tab: Shift+F8 left/right, F9 top/bottom.
                KeyCode::F(8) if shift => {
                    let cwd = self.shell_cwd();
                    self.shell.split_active(&cwd, SplitDir::LeftRight);
                    return Ok(());
                }
                KeyCode::F(9) if shift => {
                    let cwd = self.shell_cwd();
                    self.shell.split_active(&cwd, SplitDir::TopBottom);
                    return Ok(());
                }
                // Close the active split pane, with confirmation.
                KeyCode::F(10) if shift => {
                    self.popup = Popup::ConfirmClose { target: CloseTarget::ShellPane };
                    return Ok(());
                }
                // Tab controls: plain F-keys.
                KeyCode::F(n @ 1..=8) if !shift => {
                    self.shell.select((n - 1) as usize);
                    return Ok(());
                }
                KeyCode::F(9) if !shift => {
                    let cwd = self.shell_cwd();
                    self.shell.new_tab(&cwd);
                    return Ok(());
                }
                KeyCode::F(10) if !shift => {
                    if self.shell.close_active() {
                        self.focus(self.last_file_pane);
                    }
                    return Ok(());
                }
                _ => {}
            }
        }
        // Everything else is forwarded to the shell.
        if let Some(bytes) = encode_key(key, app_cursor) {
            if let Some(s) = self.shell.active_session_mut() {
                s.write_input(&bytes);
            }
        }
        Ok(())
    }

    fn handle_visual_key(&mut self, mut key: KeyEvent) -> Result<()> {
        normalize_jp_key(&mut key);
        match key.code {
            KeyCode::Esc => self.visual_cancel_and_clear_all(),
            KeyCode::Enter | KeyCode::Char('v') => self.visual_commit(),
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(1); }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(-1); }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_normal_key(&mut self, mut key: KeyEvent) -> Result<()> {
        // Full-width input (全角英数) → ASCII so commands work without leaving
        // the Japanese IME. Kana can be bound per-key via init.lua.
        normalize_jp_key(&mut key);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        // `gg` chord → jump to top
        if self.pending_g {
            self.pending_g = false;
            if matches!(key.code, KeyCode::Char('g')) && !ctrl {
                if let Some(p) = self.active_pane_mut() { p.cursor = 0; }
                return Ok(());
            }
            // anything else: fall through to normal handling
        }

        // User keymap overrides: plain character keys (no Ctrl). Only keys the
        // user explicitly bound appear here, so default behaviour is untouched
        // for everything else.
        if !ctrl {
            if let KeyCode::Char(c) = key.code {
                if let Some(action) = self.keymap.get(&c).copied() {
                    return self.execute_action(action);
                }
            }
        }

        match (ctrl, shift, key.code) {
            (false, _, KeyCode::Char('q')) => self.start_quit_confirm(),
            (false, false, KeyCode::Char(':')) => {
                self.mode = Mode::Command;
                self.command_buffer.clear();
            }
            (false, _, KeyCode::Esc) => {
                if let Some(p) = self.active_pane_mut() { p.clear_marks(); }
            }
            // Pane navigation: Shift + H/J/K/L (universally works, no terminal config needed).
            // Ctrl+H/J/K/L is the alternative — needs `enable_kitty_keyboard = true` in WezTerm.
            (false, _, KeyCode::Char('H')) => self.focus_direction('h'),
            (false, _, KeyCode::Char('J')) => self.focus_direction('j'),
            (false, _, KeyCode::Char('K')) => self.focus_direction('k'),
            (false, _, KeyCode::Char('L')) => self.focus_direction('l'),
            (true, _, KeyCode::Char('h')) => self.focus_direction('h'),
            (true, _, KeyCode::Char('j')) => self.focus_direction('j'),
            (true, _, KeyCode::Char('k')) => self.focus_direction('k'),
            (true, _, KeyCode::Char('l')) => self.focus_direction('l'),
            (false, false, KeyCode::Tab) => {
                if let Some(t) = self.active_file_tabs_mut() { t.next_tab(); }
            }
            (_, _, KeyCode::BackTab) => {
                if let Some(t) = self.active_file_tabs_mut() { t.prev_tab(); }
            }
            (true, _, KeyCode::Char(c)) if c.is_ascii_digit() => {
                if let Some(d) = c.to_digit(10) {
                    if d >= 1 {
                        if let Some(t) = self.active_file_tabs_mut() { t.select(d as usize - 1); }
                    }
                }
            }
            // Tab management: t = new tab, w = close active tab (Ctrl variants intentionally absent).
            (false, false, KeyCode::Char('t')) => {
                if let Some(t) = self.active_file_tabs_mut() { t.add_clone()?; }
            }
            (false, false, KeyCode::Char('w')) => {
                if let Some(t) = self.active_file_tabs_mut() { t.close_active(); }
            }
            // Consistent F-key tab controls (parallel the shell's pane controls):
            // Shift+F1/F2 = next/prev tab, Shift+F10 = close tab (with confirm).
            (false, true, KeyCode::F(1)) => {
                if let Some(t) = self.active_file_tabs_mut() { t.next_tab(); }
            }
            (false, true, KeyCode::F(2)) => {
                if let Some(t) = self.active_file_tabs_mut() { t.prev_tab(); }
            }
            (false, true, KeyCode::F(10)) => {
                self.popup = Popup::ConfirmClose { target: CloseTarget::FileTab(self.focused) };
            }
            // search, history, shortcuts
            (false, false, KeyCode::Char('f')) => self.start_search(),
            (false, _, KeyCode::Char('n')) => self.jump_to_next_match(true),
            (false, _, KeyCode::Char('N')) => self.jump_to_next_match(false),
            (false, false, KeyCode::Char('h')) => self.start_history(),
            (false, false, KeyCode::Char('s')) => self.start_shortcuts(),
            // navigation: gg/G + Shift+U/D for fast cursor moves
            (false, false, KeyCode::Char('g')) => { self.pending_g = true; }
            (false, _, KeyCode::Char('G')) => {
                if let Some(p) = self.active_pane_mut() {
                    if !p.entries.is_empty() { p.cursor = p.entries.len() - 1; }
                }
            }
            (false, _, KeyCode::Char('U')) => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(-10); }
            }
            (false, _, KeyCode::Char('D')) => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(10); }
            }
            // p = copy path strings; P = copy file references (Finder/Explorer-style)
            (false, false, KeyCode::Char('p')) => self.copy_paths_to_clipboard(),
            (false, true, KeyCode::Char('P')) => self.copy_file_refs_to_clipboard(),
            (false, false, KeyCode::Char(' ')) => {
                if let Some(p) = self.active_pane_mut() {
                    let i = p.cursor; p.toggle_mark_at(i); p.move_cursor(1);
                }
            }
            (false, true, KeyCode::Char(' ')) => {
                if let Some(p) = self.active_pane_mut() {
                    let i = p.cursor; p.toggle_mark_at(i); p.move_cursor(-1);
                }
            }
            (false, false, KeyCode::Char('v')) => self.visual_start(),
            (false, true, KeyCode::Char('V')) => {
                if let Some(p) = self.active_pane_mut() {
                    for i in 0..p.entries.len() { p.toggle_mark_at(i); }
                }
            }
            (false, false, KeyCode::Char('y')) | (false, false, KeyCode::Char('c')) => {
                self.start_transfer(PendingOp::Copy);
            }
            (false, false, KeyCode::Char('m')) => self.start_transfer(PendingOp::Move),
            (false, false, KeyCode::Char('d')) => self.start_delete(),
            (false, false, KeyCode::Char('r')) => self.start_rename(),
            (false, false, KeyCode::Char('a')) => self.start_new_file(),
            (false, true, KeyCode::Char('A')) => self.start_new_dir(),
            (false, false, KeyCode::Char('j')) | (_, _, KeyCode::Down) => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(1); }
            }
            (false, false, KeyCode::Char('k')) | (_, _, KeyCode::Up) => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(-1); }
            }
            // Parent: h was reassigned to history; use -, Backspace, or Left arrow instead.
            (false, false, KeyCode::Char('-'))
            | (_, _, KeyCode::Left)
            | (_, _, KeyCode::Backspace) => {
                if let Some(p) = self.active_pane_mut() { p.go_parent()?; }
            }
            // FIX: l / Right only enters directories; never opens files.
            (false, false, KeyCode::Char('l')) | (_, _, KeyCode::Right) => {
                if let Some(p) = self.active_pane_mut() {
                    let is_dir = p.selected().map(|e| e.is_dir).unwrap_or(false);
                    if is_dir { p.enter_selected()?; }
                }
            }
            // Ctrl+Enter / Ctrl+Shift+Enter need kitty keyboard protocol to be distinguished.
            (true, false, KeyCode::Enter) => { self.open_in_other_pane(false)?; }
            (true, true, KeyCode::Enter) => { self.open_in_other_pane(true)?; }
            // Universal aliases (always work, no terminal config needed).
            (false, false, KeyCode::Char('o')) => { self.open_in_other_pane(false)?; }
            (false, true, KeyCode::Char('O')) => { self.open_in_other_pane(true)?; }
            // Enter alone keeps the OS-open behavior until viewer ships in sprint 5.
            (false, _, KeyCode::Enter) => {
                let is_dir = self.active_pane()
                    .and_then(|p| p.selected())
                    .map(|e| e.is_dir)
                    .unwrap_or(false);
                if is_dir {
                    if let Some(p) = self.active_pane_mut() { p.enter_selected()?; }
                } else {
                    self.open_externally();
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Run a remappable action (dispatched from a user keymap override). The
    /// bodies mirror the default key handlers exactly so behaviour is identical
    /// whether triggered by a default key or a user-bound one.
    fn execute_action(&mut self, action: Action) -> Result<()> {
        match action {
            Action::CursorDown => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(1); }
            }
            Action::CursorUp => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(-1); }
            }
            Action::CursorBottom => {
                if let Some(p) = self.active_pane_mut() {
                    if !p.entries.is_empty() { p.cursor = p.entries.len() - 1; }
                }
            }
            Action::PageUp => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(-10); }
            }
            Action::PageDown => {
                if let Some(p) = self.active_pane_mut() { p.move_cursor(10); }
            }
            Action::Parent => {
                if let Some(p) = self.active_pane_mut() { p.go_parent()?; }
            }
            Action::EnterDir => {
                if let Some(p) = self.active_pane_mut() {
                    let is_dir = p.selected().map(|e| e.is_dir).unwrap_or(false);
                    if is_dir { p.enter_selected()?; }
                }
            }
            Action::Quit => self.start_quit_confirm(),
            Action::Search => self.start_search(),
            Action::SearchNext => self.jump_to_next_match(true),
            Action::SearchPrev => self.jump_to_next_match(false),
            Action::History => self.start_history(),
            Action::Shortcuts => self.start_shortcuts(),
            Action::Copy => self.start_transfer(PendingOp::Copy),
            Action::Move => self.start_transfer(PendingOp::Move),
            Action::Delete => self.start_delete(),
            Action::Rename => self.start_rename(),
            Action::NewFile => self.start_new_file(),
            Action::NewDir => self.start_new_dir(),
            Action::OpenOther => self.open_in_other_pane(false)?,
            Action::OpenOtherTab => self.open_in_other_pane(true)?,
            Action::OpenExternal => self.open_externally(),
            Action::CopyPath => self.copy_paths_to_clipboard(),
            Action::CopyFileRef => self.copy_file_refs_to_clipboard(),
            Action::MarkDown => {
                if let Some(p) = self.active_pane_mut() {
                    let i = p.cursor; p.toggle_mark_at(i); p.move_cursor(1);
                }
            }
            Action::MarkUp => {
                if let Some(p) = self.active_pane_mut() {
                    let i = p.cursor; p.toggle_mark_at(i); p.move_cursor(-1);
                }
            }
            Action::InvertMarks => {
                if let Some(p) = self.active_pane_mut() {
                    for i in 0..p.entries.len() { p.toggle_mark_at(i); }
                }
            }
            Action::Visual => self.visual_start(),
            Action::Command => {
                self.mode = Mode::Command;
                self.command_buffer.clear();
            }
        }
        Ok(())
    }
}

/// Map a full-width character to its ASCII equivalent, if it has one.
///
/// Covers the full-width ASCII block (U+FF01–U+FF5E → U+0021–U+007E) and the
/// ideographic space, so commands work while a Japanese IME is in full-width
/// alphanumeric (全角英数) mode without switching back to ASCII input.
fn jp_to_ascii(c: char) -> Option<char> {
    let u = c as u32;
    if (0xFF01..=0xFF5E).contains(&u) {
        char::from_u32(u - 0xFEE0)
    } else if c == '\u{3000}' {
        Some(' ')
    } else {
        None
    }
}

/// Normalise a key in place: full-width characters become their ASCII command
/// key, with SHIFT synthesised for upper-case letters so the existing
/// shift-gated bindings (A, V, P, O, …) still match.
fn normalize_jp_key(key: &mut KeyEvent) {
    if let KeyCode::Char(c) = key.code {
        if let Some(a) = jp_to_ascii(c) {
            key.code = KeyCode::Char(a);
            if a.is_ascii_uppercase() {
                key.modifiers.insert(KeyModifiers::SHIFT);
            }
        }
    }
}

/// Translate a key event into the byte sequence a terminal would send to the
/// shell. `app_cursor` selects between normal (`ESC [`) and application
/// (`ESC O`) cursor-key encodings, mirroring the active DECCKM mode.
fn encode_key(key: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let cursor = |c: u8| -> Vec<u8> {
        let intro = if app_cursor { b"\x1bO" } else { b"\x1b[" };
        let mut v = intro.to_vec();
        v.push(c);
        v
    };

    let mut out: Vec<u8> = Vec::new();
    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                let ctl = match c {
                    ' ' | '@' => Some(0u8),
                    'a'..='z' => Some(c as u8 - b'a' + 1),
                    'A'..='Z' => Some(c as u8 - b'A' + 1),
                    '[' => Some(27),
                    '\\' => Some(28),
                    ']' => Some(29),
                    '^' => Some(30),
                    '_' => Some(31),
                    '?' => Some(127),
                    _ => None,
                };
                if alt {
                    out.push(0x1b);
                }
                match ctl {
                    Some(b) => out.push(b),
                    None => {
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                }
            } else {
                if alt {
                    out.push(0x1b);
                }
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => out = cursor(b'A'),
        KeyCode::Down => out = cursor(b'B'),
        KeyCode::Right => out = cursor(b'C'),
        KeyCode::Left => out = cursor(b'D'),
        KeyCode::Home => out = cursor(b'H'),
        KeyCode::End => out = cursor(b'F'),
        KeyCode::PageUp => out.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => out.extend_from_slice(b"\x1b[6~"),
        KeyCode::Delete => out.extend_from_slice(b"\x1b[3~"),
        KeyCode::Insert => out.extend_from_slice(b"\x1b[2~"),
        _ => return None,
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn os_open(path: &Path) -> Result<()> {
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
    cmd.arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

fn os_open_string(target: &str) -> Result<()> {
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

/// The user's home directory: `$HOME`, or `$USERPROFILE` on Windows.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn expand_tilde(p: &Path) -> PathBuf {
    if let Some(s) = p.to_str() {
        if let Some(rest) = s.strip_prefix("~/") {
            if let Some(home) = home_dir() {
                return home.join(rest);
            }
        }
        if s == "~" {
            if let Some(home) = home_dir() {
                return home;
            }
        }
    }
    p.to_path_buf()
}

/// Put native file references on the clipboard so Finder/Explorer can paste
/// the actual files (not just the path string).
#[cfg(target_os = "macos")]
fn os_clipboard_file_refs(paths: &[PathBuf]) -> Result<()> {
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
    let status = Command::new("osascript")
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
fn os_clipboard_file_refs(paths: &[PathBuf]) -> Result<()> {
    use std::io::Write;
    let uris = paths
        .iter()
        .map(|p| format!("file://{}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    // try wl-copy first (wayland), then xclip
    if let Ok(mut child) = Command::new("wl-copy")
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
    let mut child = Command::new("xclip")
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
fn os_clipboard_file_refs(_paths: &[PathBuf]) -> Result<()> {
    anyhow::bail!("file-reference clipboard not yet implemented on Windows");
}

fn shortcut_icon(target: &str) -> &'static str {
    if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("file://")
    {
        return "\u{f0ac}"; // globe
    }
    let lower = target.to_lowercase();
    if lower.ends_with(".app") {
        return "\u{f179}"; // apple
    }
    let path = expand_tilde(Path::new(target));
    if path.is_dir() {
        return "\u{f07b}"; // folder
    }
    if path.exists() {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let entry = cian_core::Entry { name, path: path.clone(), is_dir: false };
        return icon_for(&entry);
    }
    "\u{f15b}" // default file
}

pub fn run(left: PathBuf, right: PathBuf) -> Result<()> {
    // Load user config (never fails; problems are reported below).
    let config = cian_lua::load();

    // Resolve and install the colour theme before any drawing happens.
    let (resolved, theme_errors) = resolve_theme(&config.theme);
    let _ = THEME.set(resolved);

    // Collect all non-fatal config issues for a single startup notice.
    let mut startup_errors = config.errors.clone();
    startup_errors.extend(theme_errors);
    for (c, name) in &config.keymaps {
        if action_from_name(name).is_none() {
            startup_errors.push(format!("keymap: unknown action {:?} (key '{}')", name, c));
        }
    }

    let mut app = App::new(left, right, config)?;
    if !startup_errors.is_empty() {
        let mut lines = vec!["config loaded with issues:".to_string(), String::new()];
        let total = startup_errors.len();
        lines.extend(startup_errors.into_iter().take(10));
        if total > 10 {
            lines.push(format!("... and {} more", total - 10));
        }
        app.popup = Popup::Notice { lines };
    }

    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    // Ask the terminal to disambiguate Ctrl-h / Ctrl-i / Ctrl-m from Backspace/Tab/Enter.
    // Supported by WezTerm, kitty, foot, etc. Silently ignored elsewhere.
    let kbd_enhanced = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app);

    if kbd_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(terminal.backend_mut(), DisableMouseCapture);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            terminal.draw(|f| draw(f, app))?;
            needs_redraw = false;
        }
        // Short timeout so live shell output is picked up promptly; we only
        // actually repaint when something changed (input, resize, or new
        // shell output), so the loop stays cheap when idle.
        if event::poll(Duration::from_millis(33))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    app.handle_key(key)?;
                    needs_redraw = true;
                }
                Event::Mouse(m) => {
                    app.handle_mouse(m);
                    needs_redraw = true;
                }
                Event::Resize(_, _) => needs_redraw = true,
                _ => {}
            }
        }
        // Repaint when any pane in the active shell tab produced new output.
        if app.shell.take_active_tab_dirty() {
            needs_redraw = true;
        }
        // If the focused pane's shell has exited (e.g. the user typed `exit`),
        // close that pane; if its tab (and the whole panel) empties, return to
        // the files so we never strand the user typing into a dead shell.
        if app.focused == FocusedPane::Shell {
            let exited = app
                .shell
                .active_session_mut()
                .map(|s| !s.is_alive())
                .unwrap_or(false);
            if exited {
                let empty = app.shell.close_active_pane();
                if empty {
                    let back = app.last_file_pane;
                    app.focus(back);
                }
                app.message = Some("shell exited".into());
                needs_redraw = true;
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

/// Normal three-surface layout: left/right file panes on top, shell below.
fn draw_split(f: &mut Frame, main_area: Rect, app: &mut App) {
    let main_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(main_area);
    let panes_area = main_split[0];
    let shell_area = main_split[1];

    let panes_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(panes_area);

    app.layout_rects = LayoutRects {
        left: panes_split[0],
        right: panes_split[1],
        shell: shell_area,
    };

    let visual_for_left = if app.focused == FocusedPane::Left { app.visual_anchor } else { None };
    let visual_for_right = if app.focused == FocusedPane::Right { app.visual_anchor } else { None };

    draw_file_pane(f, panes_split[0], &app.left, app.focused == FocusedPane::Left, visual_for_left, app.mode);
    draw_file_pane(f, panes_split[1], &app.right, app.focused == FocusedPane::Right, visual_for_right, app.mode);
    // draw_shell sizes each pane's PTY to its computed sub-rect.
    draw_shell(f, shell_area, &mut app.shell, app.focused == FocusedPane::Shell);
}

/// Zoomed layout: only the focused surface, filling the available area.
fn draw_zoomed(f: &mut Frame, area: Rect, app: &mut App) {
    let mut rects = LayoutRects::default();
    match app.focused {
        FocusedPane::Left => {
            rects.left = area;
            app.layout_rects = rects;
            let va = app.visual_anchor;
            draw_file_pane(f, area, &app.left, true, va, app.mode);
        }
        FocusedPane::Right => {
            rects.right = area;
            app.layout_rects = rects;
            let va = app.visual_anchor;
            draw_file_pane(f, area, &app.right, true, va, app.mode);
        }
        FocusedPane::Shell => {
            rects.shell = area;
            app.layout_rects = rects;
            draw_shell(f, area, &mut app.shell, true);
        }
    }
}

fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let bottom_lines = if app.mode == Mode::Command { 2 } else { 1 };
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(bottom_lines)])
        .split(area);
    let main_area = vertical[0];
    let bottom_area = vertical[1];

    if app.zoomed {
        draw_zoomed(f, main_area, app);
    } else {
        draw_split(f, main_area, app);
    }

    if app.mode == Mode::Command {
        let cmd_area = Rect::new(bottom_area.x, bottom_area.y, bottom_area.width, 1);
        let status_area = Rect::new(bottom_area.x, bottom_area.y + 1, bottom_area.width, 1);
        draw_command_line(f, cmd_area, &app.command_buffer);
        draw_status(f, status_area, app);
    } else {
        draw_status(f, bottom_area, app);
    }

    if !matches!(app.popup, Popup::None) {
        draw_popup(f, area, &app.popup);
    }
}

/// Build a tab strip. Active tab uses full path; inactive tabs use just the
/// directory name. If the labels overflow `max_width`, the rest collapse into
/// a `+N` marker so the active tab stays visible.
fn tabs_title<'a>(tabs: &'a PaneTabs, focused: bool, focus_bg: Color, max_width: u16) -> Line<'a> {
    fn label_for(i: usize, tab: &Pane, is_active: bool) -> String {
        let main = if is_active {
            tab.cwd.display().to_string()
        } else {
            tab.cwd
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| tab.cwd.display().to_string())
        };
        format!(" {} {} ", i + 1, main)
    }
    let width_of = |s: &str| s.chars().count() as u16;

    // First, lay out tabs starting from the active one outward so it never gets cut.
    let active = tabs.active.min(tabs.tabs.len().saturating_sub(1));
    let total = tabs.tabs.len();
    let mut shown: Vec<usize> = vec![active];
    let mut used: u16 = width_of(&label_for(active, &tabs.tabs[active], true));
    let sep_w: u16 = 1;
    let reserve: u16 = 5; // for " +N "

    let (mut left, mut right) = (active, active);
    loop {
        let try_right = right + 1 < total;
        let try_left = left > 0;
        if !try_right && !try_left { break; }
        // prefer expanding right first (chronological order)
        if try_right {
            let i = right + 1;
            let w = width_of(&label_for(i, &tabs.tabs[i], false)) + sep_w;
            let need_reserve = if i + 1 < total || left > 0 { reserve } else { 0 };
            if used + w + need_reserve <= max_width {
                shown.push(i);
                used += w;
                right = i;
                continue;
            }
        }
        if try_left {
            let i = left - 1;
            let w = width_of(&label_for(i, &tabs.tabs[i], false)) + sep_w;
            let need_reserve = if i > 0 || right + 1 < total { reserve } else { 0 };
            if used + w + need_reserve <= max_width {
                shown.insert(0, i);
                used += w;
                left = i;
                continue;
            }
        }
        break;
    }
    let hidden_left = left;
    let hidden_right = total.saturating_sub(right + 1);

    let mut spans: Vec<Span<'a>> = Vec::new();
    spans.push(Span::raw(" "));
    if hidden_left > 0 {
        spans.push(Span::styled(
            format!("+{} ", hidden_left),
            Style::default().fg(Color::DarkGray),
        ));
    }
    for (pos, &i) in shown.iter().enumerate() {
        let is_active = i == active;
        let style = if is_active {
            if focused {
                Style::default().fg(Color::Black).bg(focus_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White).bg(Color::DarkGray)
            }
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label = label_for(i, &tabs.tabs[i], is_active);
        spans.push(Span::styled(label, style));
        if pos + 1 < shown.len() {
            spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
        }
    }
    if hidden_right > 0 {
        spans.push(Span::styled(
            format!(" +{}", hidden_right),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// Pick a Nerd Font glyph based on the entry name/extension.
fn icon_for(entry: &cian_core::Entry) -> &'static str {
    if entry.is_dir {
        return match entry.name.as_str() {
            ".git" => "\u{e702}",
            ".github" => "\u{f408}",
            "node_modules" => "\u{e5fa}",
            "src" => "\u{f121}",
            "tests" | "test" => "\u{f0c3}",
            "docs" | "doc" => "\u{f02d}",
            "target" | "build" | "dist" | "out" => "\u{f1c6}",
            ".vscode" | ".idea" => "\u{e7c5}",
            _ => "\u{f07b}",
        };
    }
    let lower = entry.name.to_lowercase();
    match lower.as_str() {
        "cargo.toml" | "cargo.lock" => return "\u{e7a8}",
        "dockerfile" | ".dockerignore" => return "\u{f308}",
        "makefile" => return "\u{e779}",
        "readme.md" | "readme" => return "\u{f48a}",
        "license" | "license.md" => return "\u{f02d}",
        ".gitignore" | ".gitattributes" | ".gitmodules" => return "\u{f1d3}",
        ".env" | ".env.local" => return "\u{f462}",
        "package.json" | "package-lock.json" | "yarn.lock" => return "\u{e60b}",
        _ => {}
    }
    let ext = std::path::Path::new(&entry.name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "rs" => "\u{e7a8}",
        "py" => "\u{e73c}",
        "js" | "mjs" | "cjs" => "\u{f2ee}",
        "ts" | "tsx" | "jsx" => "\u{e628}",
        "go" => "\u{e627}",
        "c" | "h" => "\u{e61e}",
        "cpp" | "cc" | "cxx" | "hpp" => "\u{e61d}",
        "java" => "\u{e738}",
        "rb" => "\u{e21e}",
        "php" => "\u{e608}",
        "lua" => "\u{e620}",
        "swift" => "\u{e755}",
        "kt" | "kts" => "\u{e634}",
        "md" | "markdown" => "\u{f48a}",
        "json" | "jsonc" => "\u{e60b}",
        "yaml" | "yml" => "\u{f481}",
        "toml" | "ini" | "conf" | "cfg" => "\u{f013}",
        "xml" => "\u{f72d}",
        "html" | "htm" => "\u{f13b}",
        "css" | "scss" | "sass" | "less" => "\u{f13c}",
        "vue" => "\u{fd42}",
        "svelte" => "\u{e697}",
        "sh" | "bash" | "zsh" | "fish" => "\u{f489}",
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" | "ico" | "tif" | "tiff" => "\u{f1c5}",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => "\u{f001}",
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "wmv" => "\u{f03d}",
        "pdf" => "\u{f1c1}",
        "zip" | "tar" | "gz" | "7z" | "rar" | "bz2" | "xz" => "\u{f1c6}",
        "txt" | "log" => "\u{f0f6}",
        "exe" | "dll" | "so" | "dylib" => "\u{f013}",
        _ => "\u{f15c}",
    }
}

fn shell_tabs_title<'a>(tabs: &'a ShellPane, focused: bool) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    spans.push(Span::raw(" "));
    for i in 0..tabs.count().max(1) {
        let label = format!(" shell {} ", i + 1);
        let style = if i == tabs.active {
            if focused {
                Style::default().fg(Color::Black).bg(theme().accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White).bg(Color::DarkGray)
            }
        } else {
            // Readable medium grey for inactive tabs (DarkGray was too dim).
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
        if i + 1 < tabs.count() {
            spans.push(Span::styled("│", Style::default().fg(Color::Gray)));
        }
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn draw_file_pane(
    f: &mut Frame,
    area: Rect,
    tabs: &PaneTabs,
    focused: bool,
    visual_anchor: Option<usize>,
    mode: Mode,
) {
    let focus_bg = focus_badge_color(mode);
    let border_style = if focused {
        Style::default().fg(focus_bg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let max_title_w = area.width.saturating_sub(2);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(tabs_title(tabs, focused, focus_bg, max_title_w));

    let pane = tabs.active_ref();
    let visual_range = visual_anchor.map(|a| {
        if a <= pane.cursor { (a, pane.cursor) } else { (pane.cursor, a) }
    });

    let items: Vec<ListItem> = pane.entries.iter().enumerate().map(|(i, e)| {
        let marked = pane.is_marked(i);
        let in_visual = visual_range.map(|(a, b)| i >= a && i <= b).unwrap_or(false);
        let mark_symbol = if marked { "● " } else { "  " };
        let mark_style = Style::default().fg(theme().mark_fg).add_modifier(Modifier::BOLD);
        let name_style = if e.is_dir {
            Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
        } else { Style::default() };
        let icon_style = if e.is_dir {
            Style::default().fg(theme().accent)
        } else {
            Style::default().fg(Color::Rgb(180, 180, 200))
        };
        let mut item = ListItem::new(Line::from(vec![
            Span::styled(mark_symbol, mark_style),
            Span::styled(format!("{}  ", icon_for(e)), icon_style),
            Span::styled(e.name.clone(), name_style),
        ]));
        if in_visual { item = item.style(Style::default().bg(theme().visual_bg)); }
        item
    }).collect();

    let list = List::new(items).block(block).highlight_style(
        Style::default().bg(theme().selected_bg).add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    if !pane.entries.is_empty() { state.select(Some(pane.cursor)); }
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_shell(f: &mut Frame, area: Rect, shell: &mut ShellPane, focused: bool) {
    let border_style = if focused {
        Style::default().fg(theme().accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(shell_tabs_title(shell, focused));
    let inner = area.inner(Margin { vertical: 1, horizontal: 1 });
    f.render_widget(block, area);

    // Remember the inner size for sizing newly-spawned panes.
    shell.rows = inner.height.max(1);
    shell.cols = inner.width.max(1);

    let active = shell.active;
    if shell.tabs.get(active).is_none() {
        let body = if let Some(err) = &shell.error {
            format!("shell failed to start: {}", err)
        } else {
            "shell pane — focus here (Shift+J / click / :shell) to start a shell. \
             Esc returns to the files."
                .to_string()
        };
        f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inner);
        return;
    }

    // Shift+F12: show only the active leaf, filling the panel.
    if shell.zoom_pane {
        let leaf = shell.tabs[active].active;
        if let Some(tab) = shell.tabs.get_mut(active) {
            if let Some(Node::Leaf(s)) = tab.nodes.get_mut(leaf).and_then(|n| n.as_mut()) {
                s.resize(inner.height.max(1), inner.width.max(1));
            }
        }
        if let Some(Node::Leaf(s)) = shell.tabs[active].nodes.get(leaf).and_then(|n| n.as_ref()) {
            if let Ok(parser) = s.parser().lock() {
                f.render_widget(PseudoTerminal::new(parser.screen()), inner);
            }
        }
        return;
    }

    let root = shell.tabs[active].root;
    if let Some(tab) = shell.tabs.get_mut(active) {
        resize_node(tab, root, inner, false);
    }
    let tab = &shell.tabs[active];
    render_node(f, tab, root, inner, tab.active, focused, false);
}

/// Recursively size each leaf's PTY to its rect. `bordered` is true for leaves
/// inside a split (which draw a 1-cell border), false for a lone root leaf.
fn resize_node(tab: &mut ShellTab, i: usize, area: Rect, bordered: bool) {
    let split = match tab.nodes.get(i).and_then(|n| n.as_ref()) {
        Some(Node::Split { dir, first, second }) => Some((*dir, *first, *second)),
        Some(Node::Leaf(_)) => None,
        None => return,
    };
    match split {
        None => {
            let (h, w) = if bordered {
                (area.height.saturating_sub(2).max(1), area.width.saturating_sub(2).max(1))
            } else {
                (area.height.max(1), area.width.max(1))
            };
            if let Some(Node::Leaf(s)) = tab.nodes[i].as_mut() {
                s.resize(h, w);
            }
        }
        Some((dir, first, second)) => {
            let rects = split_rects(dir, area);
            resize_node(tab, first, rects.0, true);
            resize_node(tab, second, rects.1, true);
        }
    }
}

/// Recursively render the split tree. Leaves inside a split get a border (the
/// active one highlighted); a lone root leaf fills its area without one.
fn render_node(
    f: &mut Frame,
    tab: &ShellTab,
    i: usize,
    area: Rect,
    active_leaf: usize,
    focused: bool,
    bordered: bool,
) {
    match tab.nodes.get(i).and_then(|n| n.as_ref()) {
        Some(Node::Leaf(session)) => {
            let target = if bordered {
                let is_active = focused && i == active_leaf;
                let bs = if is_active {
                    Style::default().fg(theme().accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let blk = Block::default().borders(Borders::ALL).border_style(bs);
                let pinner = area.inner(Margin { vertical: 1, horizontal: 1 });
                f.render_widget(blk, area);
                pinner
            } else {
                area
            };
            if let Ok(parser) = session.parser().lock() {
                f.render_widget(PseudoTerminal::new(parser.screen()), target);
            }
        }
        Some(Node::Split { dir, first, second }) => {
            let rects = split_rects(*dir, area);
            render_node(f, tab, *first, rects.0, active_leaf, focused, true);
            render_node(f, tab, *second, rects.1, active_leaf, focused, true);
        }
        None => {}
    }
}

/// Split a rect 50/50 along the given direction.
fn split_rects(dir: SplitDir, area: Rect) -> (Rect, Rect) {
    let direction = match dir {
        SplitDir::LeftRight => Direction::Horizontal,
        SplitDir::TopBottom => Direction::Vertical,
    };
    let rects = Layout::default()
        .direction(direction)
        .constraints([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(area);
    (rects[0], rects[1])
}

fn draw_command_line(f: &mut Frame, area: Rect, buf: &str) {
    let text = format!(":{}", buf);
    let p = Paragraph::new(text).style(
        Style::default().bg(Color::Rgb(20, 20, 30)).fg(Color::White).add_modifier(Modifier::BOLD),
    );
    f.render_widget(p, area);
}

fn focus_badge_color(mode: Mode) -> Color {
    match mode {
        Mode::Normal => theme().accent,
        Mode::Visual => Color::Rgb(255, 140, 0),
        Mode::Search => Color::Rgb(80, 200, 120),
        Mode::Command => Color::Rgb(200, 100, 200),
        Mode::Shell => Color::Rgb(200, 160, 60),
    }
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let focus_label = match app.focused {
        FocusedPane::Left => "L",
        FocusedPane::Right => "R",
        FocusedPane::Shell => "S",
    };
    let badge_bg = focus_badge_color(app.mode);
    let (item_count, mark_count) = match app.active_pane() {
        Some(p) => (p.entries.len(), p.mark_count()),
        None => (0, 0),
    };
    let dim_sep = Span::styled(
        "  ▏  ",
        Style::default().fg(Color::Rgb(90, 90, 110)).bg(theme().status_bg),
    );
    let pad = Span::styled(" ", Style::default().bg(theme().status_bg));
    let chip = |label: String, fg: Color| {
        Span::styled(
            label,
            Style::default().fg(fg).bg(theme().status_bg).add_modifier(Modifier::BOLD),
        )
    };

    let mut spans: Vec<Span> = vec![
        Span::styled(
            format!(" {} ", focus_label),
            Style::default().fg(Color::Black).bg(badge_bg).add_modifier(Modifier::BOLD),
        ),
        pad.clone(),
        chip(format!("{} items", item_count), Color::White),
        dim_sep.clone(),
        chip(
            format!("marks {}", mark_count),
            if mark_count > 0 { theme().mark_fg } else { Color::Rgb(140, 140, 160) },
        ),
        dim_sep.clone(),
        chip(format!("mask {}", app.mask), Color::Rgb(180, 180, 220)),
    ];

    if app.zoomed {
        spans.push(dim_sep.clone());
        spans.push(chip("[zoom]".to_string(), theme().accent));
    }

    if let Some(msg) = app.message.as_ref() {
        if !msg.is_empty() {
            spans.push(dim_sep.clone());
            spans.push(Span::styled(
                format!("◂ {}", msg),
                Style::default()
                    .fg(theme().accent)
                    .bg(theme().status_bg)
                    .add_modifier(Modifier::ITALIC | Modifier::BOLD),
            ));
        }
    }

    let line = Line::from(spans);
    let p = Paragraph::new(line).style(Style::default().bg(theme().status_bg));
    f.render_widget(p, area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut)
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn draw_popup(f: &mut Frame, area: Rect, popup: &Popup) {
    let (title, body, footer) = match popup {
        Popup::ConfirmDelete { targets } => {
            let title = " delete ".to_string();
            let head = format!("{} item(s) will be deleted:", targets.len());
            let mut lines = vec![head, String::new()];
            for p in targets.iter().take(8) { lines.push(format!("  {}", p.display())); }
            if targets.len() > 8 { lines.push(format!("  ... and {} more", targets.len() - 8)); }
            (title, lines, " y=Yes  n=No  a=Yes(force)  Esc=cancel ".to_string())
        }
        Popup::ConfirmTransfer { op, targets, dest } => {
            let verb = match op { PendingOp::Copy => "copy", PendingOp::Move => "move" };
            let title = format!(" {} ", verb);
            let head = format!("{} item(s) → {}", targets.len(), dest.display());
            let mut lines = vec![head, String::new()];
            for p in targets.iter().take(8) { lines.push(format!("  {}", p.display())); }
            if targets.len() > 8 { lines.push(format!("  ... and {} more", targets.len() - 8)); }
            (title, lines, " y=Yes(skip on conflict)  a=Yes(overwrite)  n/Esc=cancel ".to_string())
        }
        Popup::TextInput { title, prompt, buffer, .. } => {
            let body = vec![prompt.clone(), format!(">{}_", buffer)];
            (format!(" {} ", title), body, " Enter=ok  Esc=cancel ".to_string())
        }
        Popup::Notice { lines } => {
            (" notice ".to_string(), lines.clone(), " Enter / Esc = close ".to_string())
        }
        Popup::Search { buffer } => {
            (
                " search ".to_string(),
                vec!["find (substring, case-insensitive):".into(), format!("/{}_", buffer)],
                " Enter=jump  Esc=cancel  (then n/N for next/prev) ".to_string(),
            )
        }
        Popup::History { entries, cursor } => {
            let mut lines: Vec<String> =
                vec![format!("recent paths ({} entries):", entries.len()), String::new()];
            for (i, p) in entries.iter().enumerate() {
                let marker = if i == *cursor { "▸ " } else { "  " };
                lines.push(format!("{}{}", marker, p.display()));
            }
            (" history ".to_string(), lines, " ↑↓/jk select  Enter jump  Esc cancel ".to_string())
        }
        Popup::ConfirmQuit => {
            (
                " quit cian? ".to_string(),
                vec!["Are you sure you want to quit?".into()],
                " y / Enter = yes   n / Esc = no ".to_string(),
            )
        }
        Popup::ConfirmClose { target } => {
            let what = match target {
                CloseTarget::ShellPane => "this shell pane",
                CloseTarget::FileTab(_) => "this tab",
            };
            (
                " close? ".to_string(),
                vec![format!("Close {}?", what)],
                " y / Enter = yes   n / Esc = no ".to_string(),
            )
        }
        Popup::Shortcuts { entries, cursor } => {
            let title = " shortcuts ".to_string();
            let mut lines: Vec<String> = if entries.is_empty() {
                vec![
                    "(no shortcuts yet)".to_string(),
                    String::new(),
                    "Press `a` to add your first one.".to_string(),
                    String::new(),
                    "Targets can be URLs (https://...), paths (~/foo),".to_string(),
                    "or apps (e.g. /Applications/Safari.app).".to_string(),
                ]
            } else {
                let mut lines = vec![format!("{} entries:", entries.len()), String::new()];
                for (i, s) in entries.iter().enumerate() {
                    let marker = if i == *cursor { "▸ " } else { "  " };
                    let icon = shortcut_icon(&s.target);
                    lines.push(format!(
                        "{}{}  {:<20} {}",
                        marker,
                        icon,
                        truncate(&s.name, 20),
                        s.target
                    ));
                }
                lines
            };
            lines.push(String::new());
            lines.push(format!("(file: {})", ShortcutStore::default_path().display()));
            (
                title,
                lines,
                " Enter=open  a=add  d=delete  r=edit  p=copy target  Esc=close ".to_string(),
            )
        }
        Popup::None => return,
    };

    let height = (body.len() as u16 + 4).max(6).min(area.height.saturating_sub(2));
    let width: u16 = 70u16.min(area.width.saturating_sub(2));
    let rect = centered_rect(width, height, area);

    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().accent).add_modifier(Modifier::BOLD))
        .title(title);
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);

    let body_text: Vec<Line> = body.into_iter().map(Line::from).collect();
    let body_area = Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1));
    let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);

    let p = Paragraph::new(body_text).wrap(Wrap { trim: false });
    f.render_widget(p, body_area);

    let footer_p = Paragraph::new(footer).style(
        Style::default().fg(Color::Black).bg(theme().accent).add_modifier(Modifier::BOLD),
    );
    f.render_widget(footer_p, footer_area);
}
