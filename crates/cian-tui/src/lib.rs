use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use cian_core::ops::{self, Conflict, DeleteMode, OpReport};
use cian_core::{Pane, Sort, SortKey};
use cian_lua::Config;
use cian_pty::PtySession;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton,
    MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{SetTitle, 
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Direction, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::{Frame, Terminal};
use serde::{Deserialize, Serialize};

mod panes;
mod theme;
use theme::*;

mod util;
use cian_lua::glob_match;
use util::{
    back_one_char, centered_rect, fit, hit_rect, order_pos, pad_left, pad_to, truncate, truncate_middle,
    union_rect,
    viewer_charwise, viewer_find, viewer_match_bracket, viewer_paragraph, vlen, width, wrap_str,
};

mod ai;
mod arcview;
mod drop;
mod font;
mod ime;
mod preview;
mod markdown;
mod viewer;
mod vim;
mod ssh;
mod gitui;
mod commands;
mod actions;
mod count;
mod du;
mod palette;
mod toggles;
mod edit;
mod macro_run;
mod session;

/// Re-exported so a front end speaks the *same* crossterm cian was built
/// against. Two copies of it in one binary would be two unrelated `KeyCode`
/// types that happen to share a name.
pub use crossterm;
/// Likewise: the `Frame` handed to [`Session::draw`] must be ratatui's own.
pub use ratatui;
mod mouse;
mod menu;
mod keys;
mod render;
use render::{draw, icon_for};
// Exercised only by the test module.
#[cfg(test)]
use render::{key_hints, tint_default_cells};

mod ai_parse;
use ai_parse::{
    clean_ai_command, clean_ai_commit_message, parse_junk_reply, parse_rename_reply,
    parse_sem_search_reply, parse_structure_reply, truncate_diff_for_ai, truncate_text_for_ai,
};
// `clean_dest_folder` / `clean_filename` are only exercised directly by tests;
// the library reaches them through the parse_* functions above.
#[cfg(test)]
use ai_parse::{clean_dest_folder, clean_filename};

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
    /// Incremental filter: the listing narrows as the user types.
    Filter,
    Shell,
}


pub struct PaneTabs {
    pub tabs: Vec<Pane>,
    pub active: usize,
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
    /// A live pane. `bg` tints only this pane, so split panes can be told
    /// apart at a glance — the whole point of colouring them.
    Leaf { session: PtySession, bg: Option<Color> },
    /// `ratio` is the percentage of the split's area given to `first`; it is
    /// what dragging the border between the two children adjusts.
    Split { dir: SplitDir, first: usize, second: usize, ratio: u16 },
}

/// One shell tab: a binary tree of PTY panes supporting nested splits. Nodes
/// live in a slab indexed by `usize`; `None` slots are free for reuse.
struct ShellTab {
    nodes: Vec<Option<Node>>,
    root: usize,
    /// Index of the active leaf node.
    active: usize,
    /// What this tab is *for*, when it has been said. Empty means the strip
    /// shows its number.
    ///
    /// Four tabs called `shell 1`..`shell 4` are four tabs you have to open
    /// to tell apart, and the reason for the second one is always that the
    /// first is busy with something in particular. `Aserver` answers that at
    /// a glance; `shell 2` never can.
    name: String,
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
    /// Set when a shell started, but not the one that was asked for. Shown once
    /// on the status line and then taken.
    note: Option<String>,
    /// Spawns currently in flight on background threads; polled each tick by
    /// [`ShellPane::poll_pending`]. See [`ShellPane::spawn_async`].
    pending: Vec<PendingSpawn>,
    /// `(tab, split node)` for a split that was just created, so the UI can
    /// animate the new pane growing in. Consumed by whoever reads it.
    just_split: Option<(usize, usize)>,
    /// A command to type into the next tab the moment it lands (used to open a
    /// file in an editor in a fresh tab; the tab spawn is async, so the command
    /// cannot be sent until the PTY exists).
    pending_tab_cmd: Option<String>,
    /// Synchronize/broadcast input: keystrokes go to every pane in the active
    /// tab at once. Only meaningful with more than one pane.
    broadcast: bool,
    /// When non-empty, sync goes only to these leaf node ids (a subset of the
    /// active tab's panes) instead of every pane. Empty = all panes.
    sync_members: std::collections::BTreeSet<usize>,
}

/// A PTY spawn running on a background thread, plus what to do with the
/// session once it arrives.
/// The result channel for an async remote directory listing: `(cwd, entries)`
/// on success, an error string otherwise.
type RemoteLsRx = std::sync::mpsc::Receiver<Result<(String, Vec<cian_scp::RemoteEntry>), String>>;

struct PendingSpawn {
    /// The session, and a note when it is not the shell that was asked for —
    /// see [`cian_pty::PtySession::start`].
    rx: std::sync::mpsc::Receiver<std::result::Result<(PtySession, Option<String>), String>>,
    kind: PendingKind,
}

/// Where a pending session should be installed once it is ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    /// The lazily-started first tab (see [`ShellPane::ensure`]).
    FirstTab,
    /// An additional tab (F9).
    NewTab,
    /// A split of tab `tab`. `leaf` is the specific leaf node to split (so a
    /// macro's `from = N` targets the intended pane regardless of what is active
    /// when the spawn lands); `None` splits whatever is active at install time.
    /// `ratio` is the percentage the source pane keeps (for even grid thirds).
    Split { tab: usize, dir: SplitDir, leaf: Option<usize>, ratio: u16 },
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingOp {
    Copy,
    Move,
}

/// A drag-selection inside a shell pane, in that pane's grid coordinates
/// (row/col relative to the PTY area), used to copy terminal text.
#[derive(Debug, Clone, Copy)]
struct ShellSel {
    tab: usize,
    leaf: usize,
    /// The PTY area on screen, to map cells for highlighting.
    inner: Rect,
    /// Anchor and moving end, as `(grid_row, grid_col)`.
    anchor: (u16, u16),
    end: (u16, u16),
    /// True once the pointer moved — a bare click just focuses.
    dragged: bool,
}

/// Which way an SFTP transfer goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScpDir {
    /// Local files → remote directory.
    Upload,
    /// A remote file → local directory.
    Download,
    /// Open the server in the active file pane (a persistent remote pane).
    BrowsePane,
}

/// A transfer waiting on the remote path being typed. Held on `App` rather than
/// in the popup so the resolved password never reaches a `Debug`-formatted
/// `Popup`.
struct ScpPending {
    target: cian_scp::Target,
    label: String,
    /// The local files to send (upload only; download uses `scp_target` + the
    /// remote browser instead).
    locals: Vec<PathBuf>,
}

/// A remote file being fetched to a temp path so the F3 viewer can open it.
struct RemoteView {
    rx: std::sync::mpsc::Receiver<Result<(), String>>,
    temp: PathBuf,
    name: String,
}

/// A queued host-crossing move: copy each source file to the destination, then
/// delete the source. A `None` target means that end is the local machine; the
/// source paths are absolute (local paths or remote absolute paths as strings).
#[derive(Clone)]
struct RemoteMovePlan {
    files: Vec<String>,
    src_target: Option<cian_scp::Target>,
    dst_target: Option<cian_scp::Target>,
    dst_dir: String,
}

// Manual Debug so the connection targets (which hold secrets) are never printed.
impl std::fmt::Debug for RemoteMovePlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteMovePlan")
            .field("files", &self.files.len())
            .field("src_remote", &self.src_target.is_some())
            .field("dst_remote", &self.dst_target.is_some())
            .field("dst_dir", &self.dst_dir)
            .finish()
    }
}

/// What a fuzzy picker is choosing between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteKind {
    /// A command to run (or, if it takes an argument, to prefill in command mode).
    Commands,
    /// A directory to jump to.
    Jump,
    /// A file to reveal in the active pane (live file finder / recent files).
    File,
}

/// One row in the fuzzy picker.
#[derive(Debug, Clone)]
struct PaletteItem {
    /// The text shown and fuzzy-matched against.
    label: String,
    /// A dimmer note on the right (a description, or the full path).
    detail: String,
    /// What to do on Enter: a command verb (Commands) or a path (Jump).
    value: String,
    /// Commands only: the command takes an argument, so Enter prefills command
    /// mode (`:verb `) instead of running it.
    takes_arg: bool,
}

#[derive(Debug, Clone)]
// `Viewer` is far wider than the rest, and deliberately so: it is the one
// variant that is on screen most of the time, and putting it behind a pointer
// would buy a smaller enum with an indirection on the hot path. Everything
// here that *can* be boxed already is — the view itself, the replace bar, the
// block prompt, the substitute walk, the grep plan — and what is left is a
// long tail of small fields rather than one big one. (Windows' clippy is the
// one that notices; the variant is a shade wider there.)
#[allow(clippy::large_enum_variant)]
enum Popup {
    None,
    ConfirmDelete { targets: Vec<PathBuf> },
    /// The operation queue (`:queue`): the running op and everything waiting.
    OpQueue { cursor: usize },
    /// Files about to lose their UTF-8 BOM (`:nobom`).
    ConfirmNoBom { targets: Vec<PathBuf> },
    /// Files about to be added into the zip the opposite pane is browsing.
    ConfirmZipAdd { archive: PathBuf, sub: String, sources: Vec<PathBuf> },
    /// Members about to be deleted from the zip being browsed. `members` is
    /// the expanded list the op works on; `shown` the rows the user picked.
    ConfirmZipDelete { archive: PathBuf, members: Vec<String>, shown: Vec<String> },
    ConfirmTransfer { op: PendingOp, targets: Vec<PathBuf>, dest: PathBuf },
    /// Overwrite confirmation for a copy-across from a comparison view (`<`/`>`
    /// in the file diff or the folder compare). `back` is the comparison popup
    /// to restore whether the copy is confirmed or cancelled.
    ConfirmDiffCopy { src: PathBuf, dst: PathBuf, is_dir: bool, back: Box<Popup> },
    /// Removing a bookmark. It asks first because a bookmark is a thing
    /// somebody made — a path they went to the trouble of naming — and `d` in
    /// the list sits one key away from `j` and `k`.
    ConfirmShortcutDelete { path: Vec<usize>, idx: usize, name: String, back: Box<Popup> },
    /// One-way "make that side match this one" from the folder compare (`]` =
    /// left→right, `[` = right→left). Copies everything the source has that the
    /// destination lacks or differs on; it never deletes, so `extra` counts the
    /// destination-only entries left untouched. `back` restores the comparison
    /// if cancelled. Each op is `(src, dest_dir, is_dir)`.
    ConfirmDirSync {
        to_right: bool,
        ops: Vec<(PathBuf, PathBuf, bool)>,
        extra: usize,
        back: Box<Popup>,
    },
    /// Confirm deleting a remote entry (`d` in the remote pane). `path` is its
    /// absolute remote path; `side` is which remote pane to re-list after.
    ConfirmRemoteDelete { side: FocusedPane, path: String, name: String, is_dir: bool },
    /// Confirm a host-crossing move (`m`): copy each file across, then delete the
    /// source. `from`/`to` label the ends for the prompt.
    ConfirmRemoteMove { plan: RemoteMovePlan, from: String, to: String },
    TextInput {
        title: String,
        prompt: String,
        buffer: String,
        kind: InputKind,
        /// Caret position, as a char index into `buffer`, so the middle of a
        /// name can be edited rather than only its end.
        cursor: usize,
        /// Whether the whole line is selected — what `Ctrl+A` leaves behind.
        ///
        /// A flag rather than an anchor and a range: everything people actually
        /// do to an address bar is to the whole of it. Select all, then type
        /// over it, or copy it, or cut it. A partial selection would need a
        /// second caret drawn, dragged and shift-extended, and none of that is
        /// what the field is for.
        select_all: bool,
    },
    Notice { lines: Vec<String> },
    /// A read-only report too tall for a `Notice`, so it scrolls like the
    /// manual but carries its own title (`:ragdebug`'s retrieval trace).
    /// `back` is the popup to put back on Esc — a report raised over the chat
    /// with `Ctrl+D` drops the user straight back into the conversation it
    /// explains.
    Report { title: String, lines: Vec<String>, scroll: usize, back: Box<Popup> },
    /// A fuzzy picker over commands or directories: type to filter, Enter runs
    /// or jumps. `shown` holds the indices of `items` currently matching `query`,
    /// best first; `cursor` indexes into `shown`.
    Palette {
        kind: PaletteKind,
        query: String,
        items: Vec<PaletteItem>,
        shown: Vec<usize>,
        cursor: usize,
        scroll: usize,
    },
    /// Disk-usage breakdown of a directory: each immediate child with its total
    /// size, biggest first, drill-downable with Enter.
    DiskUsage {
        dir: PathBuf,
        entries: Vec<cian_core::du::DuEntry>,
        total: u64,
        cursor: usize,
        scroll: usize,
    },
    /// The key manual. Unlike `Notice` it is far taller than any terminal, so
    /// it carries a scroll offset (in lines from the top).
    Manual { lines: Vec<String>, scroll: usize },
    /// Right-click menu, anchored near the pointer.
    ContextMenu { items: Vec<MenuItem>, cursor: usize, at: (u16, u16) },
    /// Background-color picker for the pane that was right-clicked.
    ColorPicker { pane: FocusedPane, cursor: usize },
    /// Sort-order picker for the focused pane.
    SortPicker { cursor: usize },
    /// The macro launcher: pick a macro from `macro.lua` to run. Names are held
    /// here so the renderer stays independent of `App`.
    Macros { cursor: usize, names: Vec<String> },
    /// A git commit log (repo-wide or one file's history). Enter shows the
    /// selected commit's diff in the viewer.
    GitLog {
        title: String,
        dir: PathBuf,
        commits: Vec<cian_core::git::Commit>,
        cursor: usize,
        scroll: usize,
        /// Which VCS produced the log — decides how Enter shows a commit.
        vcs: Vcs,
    },
    /// An image shown as half-block cells (works in any 24-bit terminal). The
    /// decoded grid is cached for the size it was last drawn at; a resize or a
    /// decode failure updates it in the render.
    ImageView {
        path: PathBuf,
        title: String,
        /// `(cols, rows, thumbnail)` cached for the last drawn inner size.
        shown: Option<(u16, u16, cian_core::image::Thumb)>,
        /// Why the image could not be decoded, if it could not.
        error: Option<String>,
    },
    /// Choose the encoding the active shell pane's output is decoded with.
    EncodingPicker { cursor: usize, target: EncTarget },
    /// A file's contents, scrollable.
    Viewer {
        title: String,
        /// The file on disk, so `Shift+Enter` can reveal it in the pane.
        path: PathBuf,
        /// What that file looked like when it was read.
        ///
        /// **A save used to write regardless.** It carried the encoding, the
        /// BOM and the line endings faithfully back onto whatever happened to
        /// be there *now* — so on a shared folder two people editing one file
        /// both saved and the second silently erased the first. `:w` refuses
        /// when this no longer matches; `:w!` is the way to mean it anyway.
        ///
        /// Carried in the popup rather than beside it because a viewer tab is
        /// stashed and restored as a whole `Popup`, and a stamp left behind
        /// would belong to the wrong file the moment you switched tabs.
        stamp: Option<cian_core::stamp::Stamp>,
        /// Boxed: `Popup` is one enum and every variant pays for the widest,
        /// so the viewer's biggest single field lives behind a pointer.
        view: Box<cian_core::viewer::View>,
        /// First visible line.
        scroll: usize,
        /// Cursor line (absolute).
        line: usize,
        /// Cursor column, as a char index into that line.
        col: usize,
        /// Remembered column for vertical motion (vim's "goal column");
        /// `usize::MAX` means "end of line" (as after `$`).
        goal: usize,
        /// Active visual selection mode; `None` in normal mode.
        visual: Option<ViewVisual>,
        /// Selection anchor `(line, col)`, meaningful while `visual` is `Some`.
        anchor: (usize, usize),
        /// While typing a `/` search, the text entered so far; `None` otherwise.
        find_input: Option<String>,
        /// While typing a `:s/old/new/` replace, the text entered so far.
        sub_input: Option<String>,
        /// While typing the text for a block insert/append/replace: the
        /// rectangle it will land in, which edge, and the text so far. Boxed
        /// for the same reason as `sub_walk` — every `Popup` pays for the
        /// widest variant.
        block_input: Option<Box<BlockInput>>,
        /// The file's shape — its headings and definitions — or `None` when
        /// nothing knows how to read this kind of file. Boxed and kept as one
        /// field so the whole `Popup` does not widen by a Vec for it.
        shape: Option<Box<Shape>>,
        /// A confirm-each-one replace in progress (the `c` flag). Boxed: the
        /// hit list would otherwise widen every `Popup` in the program.
        sub_walk: Option<Box<SubWalk>>,
        /// The confirmed search pattern, kept for `n`/`N` and match highlight.
        find_query: Option<String>,
        /// A pending numeric count typed before a motion (vim's `42G`).
        count: Option<usize>,
        /// A pending operator key awaiting its second half (`d` of `dd`).
        pending: Option<char>,
        /// Per-line git change status vs HEAD (the change gutter), keyed by
        /// 0-based line index. Empty when not tracked or unchanged.
        git_lines: std::collections::HashMap<usize, cian_core::git::LineChange>,
        /// True for a Markdown file, so `p` can toggle a rendered preview.
        markdown: bool,
        /// Showing the rendered Markdown preview rather than the raw source.
        ///
        /// The preview is a *full* viewer: `view.lines` is swapped for the
        /// rendered plain text (so the cursor, visual selection, `/` search and
        /// mouse all work over the rendered document), and `md_styles` carries
        /// the per-character colour applied underneath. The render owns the
        /// swap: it re-renders when `preview` flips or the width changes.
        preview: bool,
        /// The original source lines, kept so leaving preview can restore them
        /// (and so the preview can be re-wrapped when the width changes).
        source: Vec<String>,
        /// Per-character base style parallel to `view.lines` while previewing;
        /// empty in source mode.
        md_styles: Vec<Vec<Style>>,
        /// Which source line each previewed line came from, parallel to
        /// `view.lines`. A rendered document has neither the same number of
        /// lines as its source nor the same order, so the outline (which reads
        /// the file) and the cursor (which walks the screen) would otherwise
        /// be counting different things — which is exactly what `]]` did.
        md_map: Vec<usize>,
        /// The inner width the preview was last wrapped to, so the render can
        /// tell when a resize means it must re-render.
        md_width: u16,
        /// The theme the preview's colours were computed under. A cache of
        /// styles is a cache of *colours*, and they stop being right the
        /// moment `:theme` changes.
        md_gen: u64,
        /// A source line the view should be showing once the preview has been
        /// built. Toggling between source and preview keeps your place, and
        /// the two are not the same lines — the map that relates them only
        /// exists after the render, so the wish is left here for it.
        md_seek: Option<usize>,
        /// Per-line git blame, shown as a left gutter when non-empty. Toggled
        /// with `B`; empty means off.
        blame: Vec<cian_core::git::BlameLine>,
        /// Syntax-highlight language for this file, if recognised. Drives the
        /// per-character colours in source (non-preview) mode.
        hl_lang: Option<cian_core::highlight::Lang>,
        /// Cached per-character highlight styles, parallel to `view.lines`.
        /// Empty until computed (and cleared on edit / re-decode so it refreshes).
        hl: Vec<Vec<Style>>,
        /// True for a real text file that can be edited and saved in place
        /// (false for a hex dump, an extracted Office document, etc).
        editable: bool,
        /// In the built-in plain-text editor: keys insert/delete instead of
        /// navigating. Toggled with `i`; left with `Esc`.
        editing: bool,
        /// Unsaved edits are present.
        dirty: bool,
        /// Undo stack for the built-in editor: whole-buffer snapshots, one per
        /// normal-mode edit or insert session (vim's coarse units, so `u` after
        /// typing a paragraph removes the paragraph, not one character).
        undo: Vec<ViewerSnap>,
        /// Columns scrolled off to the left. A long line is not a reason to
        /// lose sight of the cursor, and wrapping one would lose the shape of
        /// a record — so the view follows sideways, as it does downwards.
        hscroll: usize,
        /// `$` in a rectangular selection: the block runs to the end of each
        /// line, however ragged they are. `A` then appends to every one of
        /// them, which is the way to put the same text on the end of lines
        /// that are not the same length.
        block_eol: bool,
        /// `R` — typing overwrites what is there instead of pushing it right.
        /// Left with Esc, like insert; it is the same editor in another gear.
        replacing: bool,
        /// The replace bar, while it is open (Ctrl+H, `:replace`).
        replace: Option<Box<ReplaceBar>>,
        /// What `u` took away, waiting to be put back by `Ctrl+R` / `Ctrl+Y`.
        /// Emptied by the next real edit, as vim empties its own: once the
        /// history forks, the branch that was undone is gone.
        redo: Vec<ViewerSnap>,
    },
    /// The recursive comparison of two directories: a list of differing paths.
    DirCompare {
        left: String,
        right: String,
        left_root: PathBuf,
        right_root: PathBuf,
        entries: Vec<cian_core::dirdiff::Entry>,
        cursor: usize,
        scroll: usize,
        truncated: bool,
    },
    /// The left pane's file against the right pane's, side by side.
    ///
    /// Both the full row list and the folded one are kept: folding is a toggle
    /// people flick back and forth, and recomputing it belongs nowhere near
    /// the render path.
    Diff {
        left: String,
        right: String,
        /// The two files on disk, so the encoding can be switched (re-diff) and
        /// the result saved.
        left_path: PathBuf,
        right_path: PathBuf,
        /// The encoding both sides were decoded with.
        encoding: cian_core::viewer::TextEncoding,
        result: cian_core::diff::Diff,
        folded: Vec<cian_core::diff::Row>,
        fold: bool,
        scroll: usize,
        /// A confirmed text search; rows containing it are highlighted and
        /// `n`/`N` step between them. `None` when no search is active.
        find: Option<String>,
        /// While typing a `/` search, the text entered so far.
        find_input: Option<String>,
    },
    /// An archive's members, with extraction from the list.
    Archive {
        path: PathBuf,
        members: Vec<cian_core::archive::Member>,
        cursor: usize,
        scroll: usize,
    },
    /// Where to send a copy or move: recent destinations plus a way to type
    /// somewhere new.
    DestPicker { op: PendingOp, targets: Vec<PathBuf>, cursor: usize },
    /// Results of a recursive search, filling in as they are found. `by_ai` is
    /// set when the list came from the AI's semantic search (`:ask`) rather than
    /// a `:find` / `:grep` sweep — the same list, named and coloured for its
    /// source.
    FindResults {
        hits: Vec<cian_core::search::Hit>,
        cursor: usize,
        scroll: usize,
        by_ai: bool,
    },
    /// Everything a grep-replace would change, before any of it is written.
    /// Boxed because it is only on screen while the user is reading it, and an
    /// unboxed plan would widen every `Popup` in the program.
    GrepReplace(Box<ReplacePlan>),
    /// SSH: pick a host, then a user on it.
    SshHosts { cursor: usize, filter: String },
    SshUsers { host: usize, cursor: usize },
    /// Browse a remote directory over SFTP to choose files to download. `cwd` is
    /// the remote directory; `marked` are file names selected for download.
    RemoteBrowser {
        label: String,
        cwd: String,
        entries: Vec<cian_scp::RemoteEntry>,
        cursor: usize,
        scroll: usize,
        marked: std::collections::BTreeSet<String>,
        loading: bool,
        /// Download (mark files to fetch) or Upload (choose a destination folder).
        purpose: BrowsePurpose,
    },
    /// Pick where a set of remote files download to: the left/right pane, the
    /// Desktop, or a typed path. `files` are the chosen remote file paths.
    LocalDest { files: Vec<String>, cursor: usize },
    /// The theme gallery (#8): each preset previews live as the cursor moves;
    /// Enter keeps it, Esc restores what was active on entry. `scope` says
    /// whether it drives the whole app or just one file pane.
    ThemePicker { cursor: usize, scope: ThemeScope },
    /// The command-snippet launcher: pick one to send to the active shell.
    /// Items come from `config.snippets`, filtered by `filter`.
    Snippets { cursor: usize, filter: String },
    /// Confirm sending a snippet flagged `confirm = true` (a destructive one).
    ConfirmSnippet { name: String, cmd: String, enter: bool },
    Search { buffer: String },
    History { entries: Vec<PathBuf>, cursor: usize },
    /// Bookmarks. `entries` is the whole tree; `path` is the group currently
    /// open (a breadcrumb of indices), and `cursor` indexes within that level.
    Shortcuts { entries: Vec<Shortcut>, cursor: usize, path: Vec<usize> },
    ConfirmQuit,
    ConfirmClose { target: CloseTarget },
    /// About to open another tab in `side`'s pane, and asking first.
    ///
    /// Opening one is a keystroke away from closing one, and a tab that opens
    /// unasked is invisible until there are six of them — which is how it was
    /// reported: "tabs keep piling up and I don't know where they came from".
    ConfirmNewTab { side: FocusedPane },
    /// An AI-generated shell command awaiting review before it goes to the
    /// prompt (never auto-run).
    /// A command the AI proposed, held for review. `description` is what was
    /// asked for, carried so `r` can send it back with more direction instead
    /// of making the user retype the request from the beginning.
    AiShellConfirm { command: String, description: String },
    /// The AI chat: a transcript, an input line, and whether a reply is pending.
    /// `sel` is a selected range of wrapped transcript lines `(anchor, cursor)`,
    /// for copying, mirroring the F3 viewer's line selection.
    AiChat {
        input: String,
        log: Vec<ChatMsg>,
        scroll: usize,
        pending: bool,
        sel: Option<(usize, usize)>,
        /// Which backend a typed follow-up goes to.
        mode: ChatMode,
        /// How the window presents itself — see [`ChatSkin`].
        skin: ChatSkin,
    },
    /// The chat history picker (`Ctrl+R` in the chat): past conversations this
    /// session, newest first. Enter reopens one, `d` forgets it.
    AiHistory { cursor: usize },
    /// The UI-toggles menu (`T`): a list of on/off settings flipped in place.
    Toggles { cursor: usize },
    /// A copy/move failed because the destination needs administrator rights.
    /// Offers to redo it elevated (Windows only).
    ConfirmElevate { op: PendingOp, targets: Vec<PathBuf>, dest: PathBuf },
    /// An AI-drafted commit message, shown editable before it is committed.
    /// `dir` is the repo the staged diff came from; `stat` summarises the files;
    /// `editing` toggles between preview and typing into `buffer`.
    CommitMessage { buffer: String, stat: String, dir: PathBuf, editing: bool },
    /// The AI's junk-file suggestions, each toggleable, before deletion. Nothing
    /// is deleted from here directly — approving hands the checked paths to the
    /// normal delete confirmation.
    JunkReview { items: Vec<JunkItem>, cursor: usize, scroll: usize },
    /// The AI's proposed folder structure: a set of moves (file → subfolder),
    /// each toggleable. Approving runs the checked moves, creating folders as
    /// needed. `dir` is the folder the moves are relative to.
    StructureReview { items: Vec<MoveItem>, cursor: usize, scroll: usize, dir: PathBuf },
    /// Proposed renames (old → new), each toggleable. Approving renames the
    /// checked files in place. `by_ai` says which side proposed them — the AI
    /// (`:airename`) or the `:renamepattern` rule — which is what the window is
    /// named and coloured for.
    RenameReview { items: Vec<RenameItem>, cursor: usize, scroll: usize, by_ai: bool },
    /// Confirm discarding (reverting) worktree changes to tracked files. This
    /// throws away uncommitted work, so it is gated behind its own dialog.
    ConfirmDiscard { targets: Vec<PathBuf>, dir: PathBuf },
    /// Duplicate files found by content, grouped, each toggleable. Approving
    /// hands the checked copies to the normal delete confirmation.
    DupeReview { items: Vec<DupeItem>, cursor: usize, scroll: usize },
}

/// Why the remote browser is open: to fetch files, or to choose a folder to
/// upload the pending local files into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BrowsePurpose {
    Download,
    Upload,
}

/// What a [`Popup::ThemePicker`] drives (#8): the whole application, or one file
/// pane. Each carries what to restore if the gallery is cancelled.
#[derive(Clone, Debug)]
pub(crate) enum ThemeScope {
    /// Whole-app theme; `revert` is the palette that was active on entry.
    App { revert: ResolvedTheme },
    /// One file pane's override; `side` is 0 = left, 1 = right, and `revert` is
    /// the pane's previous override name (`None` = it was following the app).
    Pane { side: usize, revert: Option<String> },
}

/// One file in a duplicate group. `group` is its 0-based group index (files in
/// the same group are byte-identical); `keeper` marks the one row per group left
/// unchecked by default, so approving deletes the redundant copies, not all of
/// them.
#[derive(Debug, Clone)]
struct DupeItem {
    path: PathBuf,
    group: usize,
    keeper: bool,
    selected: bool,
}

/// One candidate the junk detector flagged: a path, why it thinks so, and
/// whether it is currently checked for deletion.
#[derive(Debug, Clone)]
struct JunkItem {
    path: PathBuf,
    reason: String,
    selected: bool,
}

/// One proposed move in a structure suggestion: take `path` (its name shown as
/// `name`) into the sub-folder `dest` (relative to the pane's directory,
/// created if missing), with the AI's short rationale.
#[derive(Debug, Clone)]
struct MoveItem {
    path: PathBuf,
    name: String,
    dest: String,
    reason: String,
    selected: bool,
}

/// One proposed rename: `path` (currently named `old`) becomes `new` (a bare
/// filename in the same directory).
#[derive(Debug, Clone)]
struct RenameItem {
    path: PathBuf,
    old: String,
    new: String,
    selected: bool,
}

/// The one thing the four review lists (junk, duplicates, structure, rename)
/// have in common: every row carries a checkbox. Their rows are otherwise
/// unrelated, so the shared key and mouse handling asks for no more than this.
trait Checkable {
    fn checked(&mut self) -> &mut bool;
    fn is_checked(&self) -> bool;
    /// What the row is about. Every review is a review of paths.
    fn path(&self) -> &std::path::Path;
}

macro_rules! checkable {
    ($($t:ty),+) => {$(impl Checkable for $t {
        fn checked(&mut self) -> &mut bool { &mut self.selected }
        fn is_checked(&self) -> bool { self.selected }
        fn path(&self) -> &std::path::Path { &self.path }
    })+};
}
checkable!(JunkItem, DupeItem, MoveItem, RenameItem);

/// The paths a review has checked.
fn checked_paths<T: Checkable>(items: &[T]) -> Vec<PathBuf> {
    items.iter().filter(|it| it.is_checked()).map(|it| it.path().to_path_buf()).collect()
}

/// The keys every review list answers to: move by one, jump to either end,
/// toggle the row under the cursor or all of them at once. Written once for
/// the four lists (junk, duplicates, structure, rename), which differ only in
/// what they hold and in the key that approves it.
pub(crate) fn review_list_key<T: Checkable>(items: &mut [T], cursor: &mut usize, code: KeyCode) {
    let n = items.len();
    match code {
        KeyCode::Char('j') | KeyCode::Down => {
            if n > 0 { *cursor = (*cursor + 1).min(n - 1); }
        }
        KeyCode::Char('k') | KeyCode::Up => *cursor = cursor.saturating_sub(1),
        KeyCode::Char('g') | KeyCode::Home => *cursor = 0,
        KeyCode::Char('G') | KeyCode::End => *cursor = n.saturating_sub(1),
        // Space toggles the row under the cursor; `a` toggles every row, off
        // if they are all on and on otherwise.
        KeyCode::Char(' ') => {
            if let Some(it) = items.get_mut(*cursor) {
                let on = it.checked();
                *on = !*on;
            }
        }
        KeyCode::Char('a') => {
            let all_on = items.iter_mut().all(|it| *it.checked());
            for it in items.iter_mut() { *it.checked() = !all_on; }
        }
        _ => {}
    }
}

/// A click on a review list row moves the cursor there and toggles it; the
/// wheel scrolls. The mouse half of [`review_list_key`], shared by the same
/// four lists.
pub(crate) fn review_list_mouse<T: Checkable>(
    items: &mut [T],
    cursor: &mut usize,
    scroll: &mut usize,
    body: Rect,
    ev: MouseEvent,
) {
    let n = items.len();
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if ev.row < body.y || ev.row >= body.y + body.height { return; }
            let idx = *scroll + (ev.row - body.y) as usize;
            if idx < n {
                *cursor = idx;
                let on = items[idx].checked();
                *on = !*on;
            }
        }
        MouseEventKind::ScrollDown => *scroll = (*scroll + 1).min(n.saturating_sub(1)),
        MouseEventKind::ScrollUp => *scroll = scroll.saturating_sub(1),
        _ => {}
    }
}

/// What the encoding picker applies its choice to.
#[derive(Debug, Clone)]
enum EncTarget {
    /// The active shell pane's live output decoding.
    Shell,
    /// A stashed F3 viewer to re-decode and restore when the pick is made.
    Viewer(Box<Popup>),
    /// A stashed file diff to re-run under the chosen encoding.
    Diff(Box<Popup>),
}

/// The replace bar: two fields and the three switches every editor's replace
/// has, on the line `:` and `/` already use.
///
/// It is a bar rather than a dialog on purpose — a dialog over the file hides
/// the thing being replaced, and what makes replace usable is watching each
/// match land. `:s/old/new/` is still there for the vi hand; this is the same
/// engine with the parts named.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReplaceBar {
    pub(crate) find: String,
    pub(crate) with: String,
    /// Which field the typing goes into: false = find, true = replacement.
    pub(crate) in_with: bool,
    /// How the find field is read: as typed, as a shell-style wildcard, or as
    /// a regular expression. Three states rather than a regex switch, because
    /// what a `*` in a search box is nearly always meant to say — "anything,
    /// here" — is neither of the other two.
    pub(crate) pattern: cian_core::substitute::Pattern,
    /// Off means the search is case-insensitive, which is cian's default
    /// everywhere else and what people expect of a fresh dialog.
    pub(crate) case_sensitive: bool,
    /// Whole words only.
    pub(crate) word: bool,
}

/// Typing the text for a rectangular edit. The rectangle is captured when the
/// key is pressed, because the selection is cleared as soon as the prompt
/// opens — leaving it on screen would suggest the cursor still moves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockInput {
    pub(crate) block: cian_core::textops::Block,
    pub(crate) kind: BlockEdit,
    pub(crate) text: String,
}

/// Which edge of the rectangle the typed text lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockEdit {
    /// `I` — at the left edge of every line.
    Insert,
    /// `A` — at the right edge of every line.
    Append,
    /// `c` — replacing what the rectangle covers.
    Replace,
    /// `I` on a line selection — at column zero of every line.
    LineStart,
    /// `A` on a line selection — at each line's own end, wherever that is.
    LineEnd,
}

/// A whole-line transform (`:sort`, `:han`, `:reindent`, …): lines in, lines
/// out, so one call site can run any of them over a selection or a file.
pub(crate) type LineTransform = Box<dyn Fn(&[String]) -> Vec<String>>;

/// A `:s/old/new/c` walk: the replacements still to be offered, and how the
/// answers have gone so far. Hits are visited in order; accepting one shifts
/// the later hits on that same line, which is tracked as it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubWalk {
    pub(crate) hits: Vec<cian_core::substitute::Hit>,
    pub(crate) idx: usize,
    pub(crate) replaced: usize,
    pub(crate) skipped: usize,
}

/// One undo step for the viewer's built-in editor: the buffer and cursor as
/// they were before an edit. Whole-buffer snapshots are deliberate — the
/// files edited here are configs and notes, not gigabyte logs (those aren't
/// `editable`), so simplicity beats a delta encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewerSnap {
    pub(crate) lines: Vec<String>,
    pub(crate) line: usize,
    pub(crate) col: usize,
    /// Set for a hex-edit snapshot: the raw bytes to restore (the text editor
    /// restores `lines` instead and leaves this `None`).
    pub(crate) bytes: Option<Vec<u8>>,
}

/// How the built-in editor answers the keyboard.
///
/// One editor, two grammars over it — not two editors. Everything below the
/// keys is shared: the same buffer, undo stack, selection kinds, search,
/// replace, save. What differs is the layer that decides what a keystroke
/// *means*, and it differs in exactly one way that matters: whether there is a
/// normal mode to be in.
///
/// The seven keys every editor has in common — save, copy, cut, paste, undo,
/// redo, select-all — already meant the same thing in every mode before this
/// existed, which is most of why a second grammar is cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditStyle {
    /// Modal, as vi is: reading until `i`, and the whole change set behind the
    /// letters. The default, and the one cian was built around.
    Vim,
    /// Always typing, as everything that is not vi is. Arrows move, Shift and
    /// an arrow select, and a letter is a letter — including `:`, which has no
    /// command line to open here because there is no mode for it to be in.
    Notepad,
}

impl EditStyle {
    fn from_name(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "vim" | "vi" => Some(Self::Vim),
            "notepad" | "plain" | "normal" => Some(Self::Notepad),
            _ => None,
        }
    }
}

/// The F3 viewer's visual-selection mode, matching vim's three flavours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewVisual {
    /// `v`: character-wise, from the anchor cell to the cursor cell.
    Char,
    /// `V`: line-wise, whole lines between anchor and cursor.
    Line,
    /// `Ctrl-v`: block-wise, the rectangle of columns between them.
    Block,
}

/// A clickable region of the on-screen popup, registered by `draw_popup` and
/// consumed by the mouse handler. Rather than duplicate every popup's layout in
/// the mouse code, the draw side (which owns the geometry) records what each
/// rect means, and clicks are turned back into the popup's own key actions.
#[derive(Debug, Clone, Copy)]
struct PopupZone {
    rect: Rect,
    kind: ZoneKind,
}

/// What clicking a [`PopupZone`] does, expressed as the keystroke it stands in
/// for so the existing popup key handlers do the actual work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZoneKind {
    /// Put the list cursor on this index, then confirm (Enter).
    SelectRow(usize),
    /// Stand in for a character key (a confirm dialog's y/n/a/r button).
    Char(char),
    /// Stand in for Enter / Esc (dialog OK / cancel).
    Enter,
    Esc,
}

/// An entry in the right-click menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    Copy,
    Cut,
    /// Enter cian's `:` command line (shell menu — the shell can't type `:`).
    CommandInput,
    /// Shell paste (send the clipboard to the PTY).
    Paste,
    /// File-clipboard paste into the focused pane (Ctrl+V).
    PasteHere,
    CopyToOther,
    MoveToOther,
    CopyToPath,
    Delete,
    Rename,
    /// Open the selection in the editor in a new shell tab.
    EditTab,
    Background,
    /// Open the theme gallery (#8) for the whole app.
    ThemePick,
    /// Open the theme gallery for just the active file pane (#8).
    ThemePickPane,
    /// OS-native actions group (#9).
    OsMenu,
    /// Open the selection with its default app (the OS "Open" verb).
    OpenDefault,
    /// Show the OS "Open with…" application picker.
    OpenWithOs,
    /// Hand the cloud copy of a synced Office document to the desktop app.
    OfficeOpen,
    /// Write a `.url` shortcut pointing at the cloud copy.
    OfficeLink,
    /// Reveal the selection in the OS file manager.
    RevealInOs,
    /// Open the OS properties / Get-Info panel.
    PropertiesOs,
    HiddenToggle,
    Attributes,
    Hash,
    Compare,
    Ssh,
    /// Send the selected file(s) to a server over SFTP.
    ScpUpload,
    /// Fetch a file from a server over SFTP into this pane.
    ScpDownload,
    /// Begin recording this shell pane's output to a log file.
    StartLog,
    /// Stop the recording running on this shell pane.
    StopLog,
    /// Cycle the encoding the shell output is decoded with.
    Encoding,
    /// Toggle the interface language (English ↔ Japanese).
    Lang,
    /// Open the AI chat.
    AiChat,
    /// Generate a shell command from a description (shell pane).
    AiShellCmd,
    /// Explain the error shown in the shell pane.
    AiExplainError,
    /// Over the viewer's selection: tidy the prose, explain the command, or
    /// review the code. The three things a file open in front of you is
    /// usually wanted for.
    /// Jump the pane to this file's folder, cursor on it.
    RevealInPane,
    AiWriting,
    AiCommandHelp,
    AiCodeFix,
    /// Draft a git commit message from the staged diff.
    AiCommit,
    /// Detect junk files in the current directory.
    AiJunk,
    /// Triage the selected file as a log (errors, timeline, likely cause).
    AiTriageLog,
    /// Find duplicate files by content (not AI).
    FindDupes,
    /// Suggest an organised folder structure for the current directory.
    AiStructure,
    /// Bulk-rename the marked files (or the whole listing) by an instruction.
    AiRename,
    /// Semantic search over the tree from a natural-language query.
    AiSearch,
    /// Summarise the file being read (`:summary`).
    ViewerSummary,
    /// Blame gutter — who last changed each line (`:blame`).
    ViewerBlame,
    /// Force a text encoding (`:enc`).
    ViewerEncoding,
    /// Open the file's mermaid diagrams in a browser (`:mermaid`).
    ViewerMermaid,
    /// The line-transform family, which had a `:` command each and no other
    /// way in. **In notepad style there is no command line**, so `:sort`
    /// existed for a vim hand and did not exist at all for the other — and the
    /// grammar that could reach them is the default, which is why nobody
    /// noticed.
    ViewerLineMenu,
    ViewerSort,
    ViewerRsort,
    ViewerUniq,
    ViewerSubstitute,
    ViewerHan,
    ViewerZen,
    ViewerExpand,
    ViewerUnexpand,
    ViewerReindent,
    ViewerLf,
    ViewerCrlf,
    /// Open the file in the external editor (`:edit`).
    ViewerEdit,
    /// Write the panel's file. `Ctrl+S` everywhere, and here too — notepad
    /// style has no `:w` to fall back on.
    ViewerSave,
    /// Flip the editor between vim keys and notepad keys.
    ///
    /// Also on `T` in the listings — but `T` is a vi motion once the panel
    /// has the keyboard, and a character once notepad style does, so from
    /// inside the editor this menu is how it is reached.
    ViewerEditStyle,
    /// Open the toggles menu (`T` / `:toggle`).
    ///
    /// The switches live behind a single letter that is easy to have never
    /// found. Right-click is where people look for "what can this do", so the
    /// menu carries a way in rather than leaving `T` as the only one.
    TogglesMenu,
    /// Close the panel, throwing away unsaved edits. `:q!` in vim style; in
    /// notepad style there is no command line, so this is the only way past
    /// the refusal that guards a dirty file.
    ViewerCloseDiscard,
    /// Open a server in this pane over SFTP (`:remote` / remote pane).
    RemotePane,
    /// Disk-usage breakdown of the current folder (`:du`).
    DiskUsage,
    /// A submenu grouping the git actions (stage / unstage / discard).
    GitMenu,
    /// `git add` the selection.
    GitStage,
    /// `git reset HEAD` the selection.
    GitUnstage,
    /// `git checkout --` the selection (discard worktree changes).
    GitDiscard,
    /// The commit log (repo, or the selected file's history).
    GitHistory,
    /// The selected file's working-tree diff vs HEAD.
    GitDiff,
    /// A submenu grouping the svn actions (add / revert / update / commit …).
    SvnMenu,
    /// `svn add` the selection.
    SvnAdd,
    /// `svn revert` the selection (discard local changes).
    SvnRevert,
    /// `svn resolve --accept working` the selection.
    SvnResolve,
    /// The selected file's working-copy diff vs BASE.
    SvnDiff,
    /// The commit log (working copy, or the selected file's history).
    SvnLog,
    /// `svn update` the working copy.
    SvnUpdate,
    /// `svn commit` the selection (prompts for a message).
    SvnCommit,
    /// Put the selection on the system clipboard as real file references, so
    /// Finder / Explorer can paste them (`Shift+P`).
    CopyFileRef,
    /// Pattern-based bulk rename of the marked files (`:renamepattern`).
    BulkRename,
    /// Rename by editing a list of names in the editor (`:renamelist`).
    EditorRename,
    /// Open the command-snippet launcher (`:snip`).
    Snippets,
    /// Open the layout-macro launcher (`@` / `:macro`).
    Macros,
    /// Open the shortcuts / bookmarks menu (the `s` key).
    Shortcuts,
    /// A submenu grouping the compress-to-archive actions.
    CompressMenu,
    /// Compress the selection to a `.zip`.
    CompressZip,
    /// Compress the selection to a password-protected `.zip`.
    CompressZipEnc,
    /// Compress the selection to a `.tar.gz`.
    CompressTarGz,
    /// Extract the archive under the cursor into a fresh sub-folder.
    Extract,
    /// Count files/steps under the selection (`:count`).
    Count,
    /// A submenu grouping the AI actions.
    AiMenu,
    /// A submenu grouping the file-transfer actions.
    SendMenu,
    /// A submenu grouping the shell window actions (splits, tabs, zoom).
    WindowMenu,
    /// A submenu grouping the less-common file actions (copy/move to other
    /// pane, copy to a path, bulk rename).
    FileMenu,
    /// A submenu grouping archive actions (compress ▸, extract here).
    ArchiveMenu,
    /// A submenu grouping the read-only "inspect" actions (attributes, hash,
    /// compare, count, find duplicates).
    InspectMenu,
    /// A submenu grouping view/misc actions (show hidden, language, copy path).
    ViewMenu,
    /// A submenu grouping the shell session actions (logging, encoding).
    SessionMenu,
    /// Copy the selection's path text to the system clipboard (the `p` key).
    CopyPathText,
    /// Split the active shell tab left/right (S-F8).
    ShellSplitLR,
    /// Split the active shell tab top/bottom (S-F9).
    ShellSplitTB,
    /// Open a new shell tab (F9).
    ShellNewTab,
    /// Close the active shell split pane (S-F10).
    ShellCloseSplit,
    /// Close the active shell tab (F10).
    ShellCloseTab,
    /// Zoom the shell surface (F12).
    ShellZoom,
    /// Start broadcasting input to every pane in the active shell tab.
    SyncStart,
    /// Stop broadcasting input.
    SyncStop,
    /// Add/remove the focused pane from the sync group (subset of panes).
    SyncMember,
    /// Goes back up from a submenu to its parent.
    Back,
    Quit,
    Manual,
}

impl MenuItem {
    /// Group items open a submenu instead of acting; this is their marker.
    fn is_group(self) -> bool {
        matches!(
            self,
            MenuItem::AiMenu
                | MenuItem::SendMenu
                | MenuItem::WindowMenu
                | MenuItem::FileMenu
                | MenuItem::ArchiveMenu
                | MenuItem::InspectMenu
                | MenuItem::ViewMenu
                | MenuItem::SessionMenu
                | MenuItem::GitMenu
                | MenuItem::SvnMenu
                | MenuItem::CompressMenu
                | MenuItem::OsMenu
        )
    }
}

impl MenuItem {
    fn label(self, lang: Lang) -> &'static str {
        match self {
            MenuItem::Copy => tr(lang, "Copy  (Ctrl+C)", "コピー  (Ctrl+C)"),
            MenuItem::Cut => tr(lang, "Cut  (Ctrl+X)", "切り取り  (Ctrl+X)"),
            // File clipboard paste (Ctrl+V); the shell's Paste is a different
            // action (send to the PTY) and keeps its own :paste hint below.
            MenuItem::PasteHere => tr(lang, "Paste  (Ctrl+V)", "貼り付け  (Ctrl+V)"),
            MenuItem::Paste => tr(lang, "Paste  (:paste)", "貼り付け  (:paste)"),
            MenuItem::CommandInput => tr(lang, "Command  (Ctrl+Enter)", "コマンド入力  (Ctrl+Enter)"),
            MenuItem::CopyToOther => tr(lang, "Copy to other pane  (c)", "反対ペインへコピー  (c)"),
            MenuItem::MoveToOther => tr(lang, "Move to other pane  (m)", "反対ペインへ移動  (m)"),
            MenuItem::CopyToPath => tr(lang, "Copy to  (:copyto)", "指定先へコピー  (:copyto)"),
            MenuItem::Delete => tr(lang, "Delete  (d)", "削除  (d)"),
            MenuItem::Rename => tr(lang, "Rename  (r)", "リネーム  (r)"),
            MenuItem::EditTab => tr(lang, "Edit in new tab  (:vim)", "新規タブで編集  (:vim)"),
            MenuItem::Background => tr(lang, "Background color", "背景色"),
            MenuItem::ThemePick => tr(lang, "Theme (whole app)  (:theme)", "テーマ（全体）  (:theme)"),
            MenuItem::ThemePickPane => tr(lang, "Theme (this pane)", "テーマ（このペイン）"),
            MenuItem::OsMenu => tr(lang, "Open / reveal  ▸", "開く / 場所  ▸"),
            MenuItem::OpenDefault => tr(lang, "Open", "開く"),
            MenuItem::OpenWithOs => tr(lang, "Open with", "プログラムから開く"),
            MenuItem::OfficeOpen => tr(lang, "Open in Office (the cloud copy)  (:office)", "Office で開く（クラウド側）  (:office)"),
            MenuItem::OfficeLink => tr(lang, "Shortcut to the cloud copy  (:officelink)", "クラウド側へのショートカットを作成  (:officelink)"),
            MenuItem::RevealInOs => {
                if cfg!(target_os = "windows") {
                    tr(lang, "Show in Explorer", "エクスプローラーで表示")
                } else if cfg!(target_os = "macos") {
                    tr(lang, "Reveal in Finder", "Finder で表示")
                } else {
                    tr(lang, "Show in file manager", "ファイルマネージャで表示")
                }
            }
            MenuItem::PropertiesOs => {
                if cfg!(target_os = "macos") {
                    tr(lang, "Get Info", "情報を見る")
                } else {
                    tr(lang, "Properties", "プロパティ")
                }
            }
            MenuItem::HiddenToggle => tr(lang, "Show / hide dotfiles  (:hidden)", "ドットファイルの表示切替  (:hidden)"),
            MenuItem::Attributes => tr(lang, "Attributes  (:attr)", "属性  (:attr)"),
            MenuItem::Hash => tr(lang, "Checksum  (:hash)", "チェックサム  (:hash)"),
            MenuItem::Compare => tr(lang, "Compare left ↔ right  (=)", "左右を比較  (=)"),
            MenuItem::CompressMenu => tr(lang, "Compress ▸", "圧縮 ▸"),
            MenuItem::CompressZip => tr(lang, "→ .zip", "→ .zip"),
            MenuItem::CompressZipEnc => tr(lang, "→ .zip  (password)", "→ .zip  (パスワード)"),
            MenuItem::CompressTarGz => tr(lang, "→ .tar.gz", "→ .tar.gz"),
            MenuItem::Extract => tr(lang, "Extract here  (:extract)", "ここに解凍  (:extract)"),
            MenuItem::Count => tr(lang, "Count files & steps  (:count)", "ファイル・ステップ数を数える  (:count)"),
            MenuItem::Ssh => tr(lang, "SSH connect  (:ssh)", "SSH接続  (:ssh)"),
            MenuItem::ScpUpload => tr(lang, "Upload → server", "アップロード → サーバ"),
            MenuItem::ScpDownload => tr(lang, "Download ← server", "ダウンロード ← サーバ"),
            MenuItem::StartLog => tr(lang, "Start session log  (:sessionlog)", "セッションログ開始  (:sessionlog)"),
            MenuItem::StopLog => tr(lang, "Stop session log  ●", "セッションログ停止  ●"),
            MenuItem::Encoding => tr(lang, "Text encoding  (e)", "文字コード  (e)"),
            MenuItem::Quit => tr(lang, "Quit cian  (q)", "cian を終了  (q)"),
            // Labelled with the language it switches *to*, so the action is
            // clear whichever language the menu is currently in.
            MenuItem::Lang => match lang {
                Lang::En => "日本語に切替",
                Lang::Ja => "Switch to English",
            },
            MenuItem::AiChat => tr(lang, "Chat  (:ai)", "チャット  (:ai)"),
            MenuItem::ViewerSummary => tr(lang, "Summarise this file  (:summary)", "このファイルを要約  (:summary)"),
            MenuItem::ViewerBlame => tr(lang, "Who changed each line  (:blame)", "各行の最終変更者  (:blame)"),
            MenuItem::ViewerEncoding => tr(lang, "Text encoding…  (:enc)", "文字コードを指定…  (:enc)"),
            MenuItem::ViewerMermaid => tr(lang, "Mermaid diagrams in a browser  (:mermaid)", "mermaid 図をブラウザで開く  (:mermaid)"),
            MenuItem::ViewerLineMenu => tr(lang, "Line operations ▸", "行の操作 ▸"),
            MenuItem::ViewerSort => tr(lang, "Sort the lines  (:sort)", "行をソート  (:sort)"),
            MenuItem::ViewerRsort => tr(lang, "Sort in reverse  (:rsort)", "行を逆順ソート  (:rsort)"),
            MenuItem::ViewerUniq => tr(lang, "Drop duplicate lines  (:uniq)", "重複行を落とす  (:uniq)"),
            MenuItem::ViewerSubstitute => tr(lang, "Replace…  (:s/old/new/g)", "置換…  (:s/古い/新しい/g)"),
            MenuItem::ViewerHan => tr(lang, "Full-width ASCII → half-width  (:han)", "全角ASCII → 半角  (:han)"),
            MenuItem::ViewerZen => tr(lang, "Half-width kana → full-width  (:zen)", "半角カナ → 全角  (:zen)"),
            MenuItem::ViewerExpand => tr(lang, "Leading tabs → spaces  (:expand)", "行頭のタブ → スペース  (:expand)"),
            MenuItem::ViewerUnexpand => tr(lang, "Leading spaces → tabs  (:unexpand)", "行頭のスペース → タブ  (:unexpand)"),
            MenuItem::ViewerReindent => tr(lang, "Re-indent to a consistent step  (:reindent)", "インデントを揃える  (:reindent)"),
            MenuItem::ViewerLf => tr(lang, "Line endings to LF  (:lf)", "改行を LF にする  (:lf)"),
            MenuItem::ViewerCrlf => tr(lang, "Line endings to CRLF  (:crlf)", "改行を CRLF にする  (:crlf)"),
            MenuItem::ViewerEdit => tr(lang, "Open in my editor  (:edit)", "外部エディタで開く  (:edit)"),
            MenuItem::ViewerSave => tr(lang, "Save  (Ctrl+S)", "保存  (Ctrl+S)"),
            MenuItem::ViewerEditStyle => {
                tr(lang, "Editor keys: vim / notepad", "エディタのキー操作: vim / メモ帳")
            }
            MenuItem::ViewerCloseDiscard => tr(lang, "Close without saving", "保存せずに閉じる"),
            MenuItem::RemotePane => tr(lang, "Open server in pane  (:sftp)", "サーバをペインで開く  (:sftp)"),
            MenuItem::DiskUsage => tr(lang, "Disk usage  (:du)", "容量分析  (:du)"),
            MenuItem::AiShellCmd => tr(lang, "Command from description  (:aicmd)", "説明からコマンド生成  (:aicmd)"),
            MenuItem::AiExplainError => tr(lang, "Explain the last error  (:explain)", "直近のエラーを説明  (:explain)"),
            MenuItem::RevealInPane => tr(lang, "Show where this file is", "このファイルの場所を開く"),
            MenuItem::AiWriting => tr(lang, "Improve this writing", "この文章を推敲"),
            MenuItem::AiCommandHelp => tr(lang, "Explain / write this command", "コマンドを説明・作成"),
            MenuItem::AiCodeFix => tr(lang, "Review and fix this code", "このコードを点検・修正"),
            MenuItem::AiCommit => tr(lang, "Draft commit message  (:aicommit)", "コミットメッセージ生成  (:aicommit)"),
            MenuItem::AiJunk => tr(lang, "Detect junk files  (:aijunk)", "ゴミファイル検出  (:aijunk)"),
            MenuItem::AiTriageLog => tr(lang, "Triage this log  (:ailog)", "このログを診断  (:ailog)"),
            MenuItem::FindDupes => tr(lang, "Find duplicate files  (:duplicate)", "重複ファイルを検出  (:duplicate)"),
            MenuItem::AiStructure => tr(lang, "Suggest folder structure  (:organize)", "ディレクトリ構成を提案  (:organize)"),
            MenuItem::AiRename => tr(lang, "AI rename  (:airename)", "AIリネーム  (:airename)"),
            MenuItem::AiSearch => tr(lang, "Semantic search  (:ask)", "セマンティック検索  (:ask)"),
            MenuItem::GitMenu => tr(lang, "Git ▸", "Git ▸"),
            MenuItem::GitStage => tr(lang, "Stage  (git add)", "ステージ  (git add)"),
            MenuItem::GitUnstage => tr(lang, "Unstage  (git reset)", "アンステージ  (git reset)"),
            MenuItem::GitDiscard => tr(lang, "Discard changes  (git checkout)", "変更を破棄  (git checkout)"),
            MenuItem::GitHistory => tr(lang, "History / log  (git log)", "履歴 / ログ  (git log)"),
            MenuItem::GitDiff => tr(lang, "Diff vs HEAD  (git diff)", "HEADとの差分  (git diff)"),
            MenuItem::SvnMenu => tr(lang, "SVN ▸", "SVN ▸"),
            MenuItem::SvnAdd => tr(lang, "Add  (svn add)", "追加  (svn add)"),
            MenuItem::SvnRevert => tr(lang, "Revert changes  (svn revert)", "変更を破棄  (svn revert)"),
            MenuItem::SvnResolve => tr(lang, "Resolve conflict  (svn resolve)", "競合を解決  (svn resolve)"),
            MenuItem::SvnDiff => tr(lang, "Diff vs BASE  (svn diff)", "BASEとの差分  (svn diff)"),
            MenuItem::SvnLog => tr(lang, "History / log  (svn log)", "履歴 / ログ  (svn log)"),
            MenuItem::SvnUpdate => tr(lang, "Update  (svn update)", "更新  (svn update)"),
            MenuItem::SvnCommit => tr(lang, "Commit  (svn commit)", "コミット  (svn commit)"),
            MenuItem::BulkRename => tr(lang, "Rename by pattern  (:renamepattern)", "パターンでリネーム  (:renamepattern)"),
            MenuItem::EditorRename => tr(lang, "Rename in editor  (:renamelist)", "エディタでリネーム  (:renamelist)"),
            MenuItem::Snippets => tr(lang, "Snippets  (:snip)", "スニペット  (:snip)"),
            MenuItem::Macros => tr(lang, "Macros  (@)", "マクロ  (@)"),
            MenuItem::Shortcuts => tr(lang, "Shortcuts  (s)", "ショートカット  (s)"),
            // The two AI backends read as one family, named for the model behind
            // "simple" is the local model configured in cian.ai
            // is the bridge to the VS Code server.
            MenuItem::AiMenu => tr(lang, "AI - simple ▸", "AI - simple ▸"),
            MenuItem::SendMenu => tr(lang, "Transfer ▸", "転送 ▸"),
            MenuItem::WindowMenu => tr(lang, "Window ▸", "ウィンドウ ▸"),
            MenuItem::FileMenu => tr(lang, "File ▸", "ファイル操作 ▸"),
            MenuItem::ArchiveMenu => tr(lang, "Archive ▸", "アーカイブ ▸"),
            MenuItem::InspectMenu => tr(lang, "Inspect ▸", "調べる ▸"),
            MenuItem::ViewMenu => tr(lang, "View ▸", "表示 ▸"),
            MenuItem::TogglesMenu => tr(lang, "Switches…  (T)", "各種スイッチ…  (T)"),
            MenuItem::SessionMenu => tr(lang, "Session ▸", "セッション ▸"),
            MenuItem::CopyPathText => tr(lang, "Copy path text  (p)", "パスをコピー  (p)"),
            MenuItem::CopyFileRef => tr(
                lang,
                "Copy file(s) — paste into Finder/Explorer  (Shift+P)",
                "ファイルをコピー — Finder/エクスプローラに貼り付け  (Shift+P)",
            ),
            MenuItem::ShellSplitLR => tr(lang, "Split left / right  (S-F8)", "左右に分割  (S-F8)"),
            MenuItem::ShellSplitTB => tr(lang, "Split top / bottom  (S-F9)", "上下に分割  (S-F9)"),
            MenuItem::ShellNewTab => tr(lang, "New tab  (F9)", "新規タブ  (F9)"),
            MenuItem::ShellCloseSplit => tr(lang, "Close split pane  (S-F10)", "分割パネルを閉じる  (S-F10)"),
            MenuItem::ShellCloseTab => tr(lang, "Close tab  (F10)", "タブを閉じる  (F10)"),
            MenuItem::ShellZoom => tr(lang, "Zoom  (F12)", "ズーム  (F12)"),
            MenuItem::SyncStart => tr(lang, "Synchronize input  ⇄", "同時入力を開始  ⇄"),
            MenuItem::SyncStop => tr(lang, "Stop synchronize  ⇄", "同時入力を停止  ⇄"),
            MenuItem::SyncMember => tr(lang, "Toggle this pane in sync group  ⇄", "このペインを同時入力に含める/外す  ⇄"),
            MenuItem::Back => tr(lang, "◂ Back", "◂ 戻る"),
            MenuItem::Manual => tr(lang, "Key manual  (?)", "キー一覧  (?)"),
        }
    }
}

/// A password held until ssh asks for it.
///
/// ssh reads the password from its controlling terminal rather than stdin, so
/// it cannot be piped in — but cian *owns* that terminal, so writing to the PTY
/// when the prompt appears works. This is the same approach TeraTerm's `.ttl`
/// macros take (`wait 'password:'` / `sendln`), and expect(1) before them.
///
/// Waiting for the prompt rather than sending blindly is what keeps this from
/// breaking everything else: a host on key auth never prompts, so the secret is
/// simply never sent and the deadline quietly expires.
struct PendingAuth {
    secret: String,
    /// Give up after this; the connection was probably keyed, refused, or is
    /// asking something else entirely (a host-key confirmation, an MFA code).
    deadline: Instant,
}

/// Never let a secret reach a log line or a panic message.
impl std::fmt::Debug for PendingAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingAuth").field("secret", &"<redacted>").finish()
    }
}

/// How long to watch for a password prompt before giving up.
///
/// In cian-core, with the prompt rule itself: the window runs `ssh` in its
/// shell too now, and a screen that gets a password out of one build must get
/// one out of the other.
use cian_core::auth::AUTH_WINDOW;

/// Two clicks closer together than this on the same row count as a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// A file operation running on a worker thread.
///
/// Copies and moves used to run inline: a 700 MB file locked the UI for
/// fourteen seconds with nothing on screen explaining why. The work now runs
/// off the event loop, reports progress back over a channel, and watches a
/// flag it can be told to stop by.
struct OpJob {
    rx: std::sync::mpsc::Receiver<OpMsg>,
    cancel: Arc<AtomicBool>,
    /// What to call it in the popup.
    label: &'static str,
    latest: cian_core::progress::Progress,
    started: Instant,
    /// Pushed onto the undo stack if the op finishes cleanly (set for moves).
    undo: Option<UndoAction>,
    /// When byte progress last moved — the difference between "slow" and
    /// "stuck". A transfer with no new bytes for [`OP_STALL_SECS`] shows a
    /// stall warning; one that is merely slow keeps its progress moving.
    last_progress: Instant,
    /// When the cancel flag was set, so a worker that ignores it (wedged in a
    /// syscall) can be offered the harder exit after a grace period.
    cancel_requested: Option<Instant>,
}

/// A queued file operation, waiting for the running one to finish.
struct QueuedOp {
    label: &'static str,
    /// Callable more than once so a failed transfer can be retried without
    /// rebuilding the closure. Every op closure only borrows its captures.
    work: Box<dyn FnMut(&mut cian_core::progress::Ctl) -> OpReport + Send>,
    /// Automatic re-runs left when the op fails (transfers get these;
    /// local operations fail for reasons a retry never fixes).
    retries: u8,
}

/// No byte progress for this long counts as a stall (shown, never auto-killed:
/// a tape drive or a saturated link is slow on purpose).
const OP_STALL_SECS: u64 = 30;
/// After a cancel that the worker has not honoured for this long, offer to
/// abandon it: orphan the thread and let the queue move on.
const OP_ABANDON_GRACE_SECS: u64 = 5;

/// A reversible file operation, for the `u` undo stack. Deletes are excluded —
/// they go to the OS trash, which has its own restore.
#[derive(Debug, Clone)]
pub(crate) enum UndoAction {
    /// Undo by renaming `to` back to `from`.
    Rename { from: PathBuf, to: PathBuf },
    /// Undo by removing what was just created.
    Created { path: PathBuf },
    /// Undo by moving each `.0` (where it is now) back to `.1` (where it was).
    Moved { pairs: Vec<(PathBuf, PathBuf)> },
    /// Undo by sending these to the trash: what a copy brought into being,
    /// and nothing else.
    ///
    /// A copy went untracked for a long time on the grounds that undoing one
    /// means deleting, and a key that sometimes deletes is not a key anyone
    /// can trust. The grounds hold; what they call for is a list that can
    /// only ever name the copy's own work. [`cian_core::ops::copy_creates`]
    /// builds it out of the destination names that did not exist a moment
    /// earlier, so a copy that landed on an existing name is left off the
    /// stack rather than half-undone — and the removal is to the trash, which
    /// keeps the one deleting step reversible in its turn.
    Copied { paths: Vec<PathBuf> },
    /// Undo by taking this pane back to `from`.
    ///
    /// Walking into the wrong folder is the commonest thing to want back, and
    /// it was the one thing `u` did not cover: the file operations were on one
    /// stack and where you *are* was on another (`Alt+←`). One stack now, in
    /// the order things happened, which is what undo means everywhere else.
    Navigated { pane: FocusedPane, from: PathBuf, to: PathBuf },
}

enum OpMsg {
    Tick(cian_core::progress::Progress),
    Done(OpReport),
}

/// A recursive search running on a worker thread.
///
/// Kept separate from [`OpJob`] because results stream in rather than a single
/// report arriving at the end: a search over a big tree should be usable while
/// it is still going.
/// A directory comparison running on a worker thread. It streams progress and
/// delivers the whole result when the walk finishes.
struct DiffJob {
    rx: std::sync::mpsc::Receiver<DiffMsg>,
    cancel: Arc<AtomicBool>,
    left_root: PathBuf,
    right_root: PathBuf,
    left: String,
    right: String,
    /// Latest progress, for the bar.
    latest: cian_core::progress::Progress,
    label: &'static str,
    started: Instant,
}

enum DiffMsg {
    Tick(cian_core::progress::Progress),
    Done(cian_core::dirdiff::DirDiff),
}

/// A grep-replace waiting for approval: what it would change, what it could
/// not read, and where the cursor is in the list.
/// A live comparison between the two halves of a split viewer.
#[derive(Debug, Clone)]
pub(crate) struct ViewerDiff {
    /// One mark per line of the focused half, and of the other.
    pub(crate) mine: Vec<cian_core::diff::Mark>,
    pub(crate) theirs: Vec<cian_core::diff::Mark>,
    /// What the two buffers hashed to when this was worked out.
    pub(crate) fp: (u64, u64),
}

/// What the viewer knows about a file's structure: the outline column and the
/// folds, which are the same information read two ways.
#[derive(Debug, Clone)]
pub(crate) struct Shape {
    /// The entries, in file order.
    pub(crate) items: Vec<cian_core::outline::Item>,
    /// Whether the outline column is showing. On by default whenever there is
    /// an outline to show: a jump list you have to remember to ask for is a
    /// jump list nobody uses.
    pub(crate) shown: bool,
    /// The lines that head a closed fold. Indices into the buffer, not into
    /// `items`, so a fold survives the outline being recomputed as long as the
    /// heading itself is still there.
    pub(crate) folds: std::collections::BTreeSet<usize>,
    /// A cheap fingerprint of the buffer the outline was read from, so an edit
    /// can be noticed without re-running the patterns every frame.
    pub(crate) fp: (usize, usize),
}

impl Shape {
    /// Read the shape of `lines`, or `None` when this kind of file has no
    /// rules. `shown` and any folds are carried over from `prev`.
    pub(crate) fn read(
        path: &std::path::Path,
        lines: &[String],
        prev: Option<&Shape>,
    ) -> Option<Box<Shape>> {
        let items = cian_core::outline::outline(path, lines);
        if items.is_empty() {
            return None;
        }
        // Folds are kept only where a heading still starts that line: after an
        // edit, a fold anchored to a line that is no longer a heading would
        // hide a region nobody can see the top of.
        let folds = prev
            .map(|p| p.folds.iter().copied().filter(|l| items.iter().any(|i| i.line == *l)).collect())
            .unwrap_or_default();
        Some(Box::new(Shape {
            items,
            shown: prev.map(|p| p.shown).unwrap_or(true),
            folds,
            fp: fingerprint(lines),
        }))
    }

    /// The extent of the fold headed at buffer line `line`, if there is one
    /// with anything under it.
    pub(crate) fn extent_at(&self, line: usize, total: usize) -> Option<(usize, usize)> {
        let idx = self.items.iter().position(|i| i.line == line)?;
        cian_core::outline::extent(&self.items, idx, total)
    }

    /// One flag per line: is it hidden inside a closed fold?
    ///
    /// A fold hides what is under its heading, never the heading itself — so
    /// a closed fold is always visible as the thing you closed, and closing
    /// everything cannot make the file disappear.
    pub(crate) fn hidden(&self, total: usize) -> Vec<bool> {
        let mut out = vec![false; total];
        for line in &self.folds {
            if let Some((start, end)) = self.extent_at(*line, total) {
                for h in out.iter_mut().take(end.min(total.saturating_sub(1)) + 1).skip(start + 1) {
                    *h = true;
                }
            }
        }
        out
    }

    /// The heading of the outermost closed fold covering `line`, for pulling a
    /// cursor back out of a region it can no longer see.
    pub(crate) fn enclosing_fold(&self, line: usize, total: usize) -> Option<usize> {
        self.folds
            .iter()
            .copied()
            .filter(|f| {
                self.extent_at(*f, total).is_some_and(|(s, e)| line > s && line <= e)
            })
            .min()
    }
}

/// A real hash of the buffer, for when "probably unchanged" is not good
/// enough.
///
/// [`fingerprint`] counts lines and bytes, which is fast and blind to any edit
/// that keeps both — `old` → `new` being exactly that. A stale outline costs a
/// wrong heading; a stale comparison says two files agree when they do not, so
/// this one reads the text. Only used while a comparison is running, where the
/// diff itself costs far more than the hash.
pub(crate) fn content_key(lines: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lines.hash(&mut h);
    h.finish()
}

/// Enough of a buffer to notice it changed, without hashing it.
pub(crate) fn fingerprint(lines: &[String]) -> (usize, usize) {
    (lines.len(), lines.iter().map(|l| l.len()).sum())
}

#[derive(Debug, Clone)]
pub(crate) struct ReplacePlan {
    /// One row per changed line, in the order [`cian_core::grepedit::plan`]
    /// found them (grouped by file, then by line).
    pub(crate) changes: Vec<cian_core::grepedit::Change>,
    pub(crate) skipped: Vec<cian_core::grepedit::Skipped>,
    pub(crate) cursor: usize,
    pub(crate) scroll: usize,
    /// `old → new`, for the title — the prompt is gone by the time this shows.
    pub(crate) what: String,
}

struct FindJob {
    rx: std::sync::mpsc::Receiver<FindMsg>,
    cancel: Arc<AtomicBool>,
    /// Pre-rendered for the popup title; the borrow checker objects to
    /// formatting it while `popup` is mutably borrowed for drawing.
    root_label: String,
    query: String,
    mode: cian_core::search::Mode,
    done: Option<cian_core::search::Outcome>,
    /// When set, the results are not a browsable popup but a listing destined
    /// for the active pane — a branch view. The string is the flat-view label;
    /// on completion the accumulated hits are panelized into the pane.
    to_pane: Option<String>,
}

enum FindMsg {
    Hit(cian_core::search::Hit),
    Done(cian_core::search::Outcome),
}

/// What a finished AI reply should be used for, so one job plumbing serves
/// every AI feature.
#[derive(Debug, Clone)]
enum AiPurpose {
    /// Append to the chat transcript.
    Chat,
    /// A shell command to review and insert at the prompt. `description` rides
    /// along so the review can offer another try without losing the request.
    ShellCommand { description: String },
    /// A git commit message drafted from the staged diff. `dir`/`stat` are
    /// carried through so the editable preview can commit into the right repo.
    CommitMessage { dir: PathBuf, stat: String },
    /// Junk-file detection over a directory listing. `names` is the name→path
    /// list the model was shown, so its answer can be validated back to real,
    /// absolute paths (a hallucinated name simply matches nothing).
    Junk { names: Vec<(String, PathBuf)> },
    /// Structure suggestion over a directory listing. `names` validates the
    /// reply back to real paths; `dir` is the folder moves are relative to.
    Structure { names: Vec<(String, PathBuf)>, dir: PathBuf },
    /// Bulk rename over a chosen set of files. `names` validates the reply back
    /// to real paths.
    Rename { names: Vec<(String, PathBuf)> },
    /// Semantic search: the model picks relevant paths from a catalog. `hits`
    /// is the catalog it was shown, so the reply validates back to real hits.
    SemSearch { hits: Vec<cian_core::search::Hit> },
}

/// A pending AI request; the worker sends the assistant's reply (or an error
/// message) back over the channel, tagged with what to do with it.
struct AiJob {
    rx: std::sync::mpsc::Receiver<Result<String, String>>,
    purpose: AiPurpose,
}

/// What the AI is being asked to do with a piece of the open file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AiOverText {
    Writing,
    Command,
    Code,
}

/// Which backend a chat talks to, so a typed follow-up goes back to the same
/// place the conversation started (not always the local `:ai` model).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum ChatMode {
    /// The local python `:ai` assistant — the only backend there is.
    Ai,
}

/// How a chat window presents itself: the name in its frame, and whether it
/// wears the local model's cyan and signs its answers "AI - simple".
///
/// Deliberately separate from [`ChatMode`], which only says where a typed
/// follow-up goes: every AI-simple action opens the same chat, so the title has
/// to name the action that opened it ("Triage this log", not just "Chat"), and
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ChatSkin {
    /// Shown at the top of the window — the menu item that opened it.
    title: String,
    /// True when the local `cian.ai` model is the one answering.
    simple: bool,
}

impl ChatSkin {
    /// The default look for a mode, used when nothing more specific applies.
    fn of(mode: ChatMode) -> Self {
        ChatSkin { title: mode.title().to_string(), simple: mode == ChatMode::Ai }
    }
    /// A named AI-simple window: the local model, titled for the action that
    /// opened it.
    fn simple(title: impl Into<String>) -> Self {
        ChatSkin { title: title.into(), simple: true }
    }
}

impl ChatMode {
    /// The name shown in the chat title.
    fn title(self) -> &'static str {
        match self {
            ChatMode::Ai => "Chat",
        }
    }
    /// A short badge for the history list.
    fn badge(self) -> &'static str {
        match self {
            ChatMode::Ai => "simple",
        }
    }
}

/// One line of an AI chat transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ChatMsg {
    /// True for the user's turn, false for the assistant's.
    user: bool,
    text: String,
}

/// Work deferred until a shrink transition finishes.
#[derive(Debug, Clone, Copy)]
enum PendingClose {
    /// Remove the shell's active split pane.
    ShellPane,
}

/// The file register lives in `cian-core` — both front ends hold one, and the
/// rules for what a paste does are judgements rather than plumbing.
pub(crate) use cian_core::clip::{self, Clipboard as FileClipboard, Op as ClipOp};

/// Preset pane backgrounds.
///
/// These exist to answer "which pane am I typing into?", so they are pitched
/// to be unmistakable at a glance rather than tasteful — an earlier, subtler
/// set failed at exactly that. Still dark enough to keep normal terminal
/// foreground colors readable on top.
/// Saturated hues spaced around the wheel, pushed as strong as they can go
/// while foreground text stays readable (luminance kept under 90). Blue carries
/// little luminance, so blues can run brightest; greens are held back most.
/// Verified by `the_palette_is_distinct_enough_to_tell_panes_apart`.
/// Per-pane background tints: dark enough for foreground text to stay readable
/// (luminance < 90), and pairwise distinct enough to tell two panes apart at a
/// glance — both enforced by a test. A richer, more saturated spread than the
/// original set, and more of them.
/// The same fourteen, as ratatui colours. The table itself is
/// `cian_core::theme::PANE_BG_PRESETS` — both front ends offer this list, and a
/// window whose "navy" was a different navy would be two programs wearing one
/// name (the same reason the palettes moved).
fn pane_bg_presets() -> Vec<(&'static str, Option<Color>)> {
    cian_core::theme::PANE_BG_PRESETS
        .iter()
        .map(|(name, rgb)| {
            (*name, rgb.map(|c| Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)))
        })
        .collect()
}

/// Resolve a macro's `bg = "…"`: a preset name (matched on its first word, so
/// `"crmaine"` finds `"crmaine (^_-)"`), else a `#rrggbb` / named / `"r,g,b"`
/// spec. `None` for an unknown spec or the "default" preset.
pub(crate) fn resolve_bg(spec: &str) -> Option<Color> {
    if let Some(c) = cian_core::theme::pane_bg(spec) {
        return Some(Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8));
    }
    // "default" resolves to no colour, and so does an unknown preset name;
    // anything else is a colour spec of its own.
    if cian_core::theme::PANE_BG_PRESETS
        .iter()
        .any(|(n, _)| n.split_whitespace().next().unwrap_or(n).eq_ignore_ascii_case(spec.trim()))
    {
        return None;
    }
    theme::parse_color(spec)
}

/// What a close-confirmation popup will close when accepted.
#[derive(Debug, Clone, Copy)]
enum CloseTarget {
    /// The active split pane in the shell.
    ShellPane,
    /// The whole active shell tab, splits and all.
    ///
    /// Asked about, like the other two. It used to close on the keypress, which
    /// left F10 the one key in cian that could end a running shell with no
    /// question asked — while Shift+F10, which closes one *pane* of that same
    /// tab, stopped to ask. The bigger loss was the quieter one.
    ShellTab,
    /// The active tab of a file pane.
    FileTab(FocusedPane),
    /// The file open in the editor panel, with unsaved edits in it.
    ///
    /// Only ever raised when it is dirty: three Escs on a clean file just
    /// close it, and asking about work that is already on disk would be a
    /// question with one answer.
    ViewerFile,
}

#[derive(Debug, Clone)]
enum InputKind {
    Rename { original: PathBuf },
    NewFile { parent: PathBuf },
    NewDir { parent: PathBuf },
    /// Naming a shortcut. `path` is the group it lives in; `edit_idx` is set
    /// when renaming an existing one; `group` makes it a folder (no target).
    ShortcutName { path: Vec<usize>, edit_idx: Option<usize>, group: bool },
    ShortcutTarget { path: Vec<usize>, edit_idx: Option<usize>, name: String },
    /// A path typed to jump to (or a file to open).
    JumpPath,
    /// A name to search for, recursively from the current directory.
    FindRecursive,
    /// Text to look for inside the files below the current directory.
    GrepRecursive,
    /// The replacement text for a grep-replace. `paths` are the files the grep
    /// matched and `pattern` the needle it matched them with, so the prompt
    /// only has to ask for the half the user has not typed yet.
    GrepReplaceWith { paths: Vec<PathBuf>, pattern: String },
    /// A directory typed as the destination of a pending copy or move.
    DestPath { op: PendingOp, targets: Vec<PathBuf> },
    /// A password for a zip about to be created. Rendered masked.
    ZipPassword { dest: PathBuf, sources: Vec<PathBuf> },
    /// A new name for a single file being copied/moved into `dest_dir`.
    TransferAs { op: PendingOp, src: PathBuf, dest_dir: PathBuf },
    /// A directory to write a session log into; the file name is generated.
    LogDir,
    /// What this shell tab is for.
    ShellName,
    /// A natural-language description to turn into a shell command via AI.
    AiShellCmd,
    /// A second try at that command. The model answered, the answer missed, and
    /// this is the direction to give it: `description` is what was asked for the
    /// first time and `rejected` is what came back, both sent again so the model
    /// is correcting a draft rather than starting over from a fragment.
    AiShellRefine { description: String, rejected: String },
    /// A natural-language instruction for how to bulk-rename the chosen files.
    AiRename,
    /// A natural-language query for semantic search over the tree.
    AiSearch,
    /// A filename to save the diff/compare result into. All three renderings are
    /// carried here (the source popup is replaced by the prompt); the format is
    /// picked from the extension the user types — `.html`/`.htm`, `.md`, else
    /// the plain-text form.
    DiffSaveAs { text: String, html: String, md: String },
    /// A name for an archive about to be created from `sources`, in the given
    /// format. The extension is appended if missing.
    CompressName { kind: CompressKind, sources: Vec<PathBuf> },
    /// The password for an encrypted zip about to be extracted. Rendered masked.
    ExtractPassword { archive: PathBuf, members: Vec<String>, dest: PathBuf, strip: String },
    /// Renaming a member (or a whole directory) inside the zip being browsed.
    RenameZipMember { archive: PathBuf, sub: String, from: String, is_dir: bool },
    /// The log message for an `svn commit` of the given paths.
    SvnCommit { paths: Vec<PathBuf> },
    /// A typed local directory to download the given remote files into.
    LocalDestPath { files: Vec<String> },
    /// The chmod mode (octal, e.g. 777; blank = keep) for the `idx`-th pending
    /// upload file to `remote`. Files are prompted one at a time so each can get
    /// its own mode; the collected modes live in `App::scp_upload_modes`.
    UploadChmod { remote: String, idx: usize },
    /// The chmod mode for files just downloaded into `dir` (local, Unix only).
    DownloadChmod { files: Vec<String>, dir: PathBuf },
    /// A bulk-rename pattern (template or `s/re/rep/flags`) for these files.
    BulkRenamePattern { targets: Vec<PathBuf> },
    /// Manual connection (#2), step 1: `user@host[:port]` typed by hand.
    /// `for_scp` is true when a transfer is being set up (vs a plain shell login).
    ManualSshTarget { for_scp: bool },
    /// Manual connection, step 2: the password for the typed server. Rendered
    /// masked.
    ManualSshPass { user: String, host: String, port: u16, for_scp: bool },
    /// A name for a new remote directory (`A`) in the remote pane on `side`.
    RemoteMkdir { side: FocusedPane },
    /// A name for a new empty remote file (`a`) in the remote pane on `side`.
    RemoteTouch { side: FocusedPane },
    /// A new name for the remote entry at `from` (its absolute remote path) in
    /// the remote pane on `side`.
    RemoteRename { side: FocusedPane, from: String },
}

/// The archive format chosen from the right-click "Compress" submenu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompressKind {
    Zip,
    /// A password-protected (AES-256) zip.
    ZipEnc,
    TarGz,
}

impl InputKind {
    /// Whether the field holds a secret and should be shown as dots.
    fn is_secret(&self) -> bool {
        matches!(self, InputKind::ZipPassword { .. } | InputKind::ExtractPassword { .. } | InputKind::ManualSshPass { .. })
    }

    /// Whether Shift+Enter puts a newline in the field instead of submitting it.
    ///
    /// Only where a paragraph is the honest answer. A filename is one line by
    /// nature and a newline in one is a mistake the field should not make
    /// possible; a description of what you want a command to do is not, and
    /// having to say it without a line break made it a wall of text.
    fn is_multiline(&self) -> bool {
        matches!(
            self,
            InputKind::AiShellCmd
                | InputKind::AiShellRefine { .. }
                | InputKind::AiRename
                | InputKind::AiSearch
        )
    }
}

/// A bookmark: either a leaf (`target` set) or a group/folder (`children` set)
/// that drills into more shortcuts. The two are mutually exclusive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shortcut {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<Shortcut>>,
}

impl Shortcut {
    fn leaf(name: String, target: String) -> Self {
        Self { name, target: Some(target), children: None }
    }
    fn group(name: String) -> Self {
        Self { name, target: None, children: Some(Vec::new()) }
    }
    fn is_group(&self) -> bool {
        self.children.is_some()
    }
    fn target_str(&self) -> &str {
        self.target.as_deref().unwrap_or("")
    }

    /// Convert from the UI-agnostic node the Lua store round-trips.
    fn from_node(n: &cian_lua::shortcuts::Node) -> Self {
        Self {
            name: n.name.clone(),
            target: n.target.clone(),
            children: n.children.as_ref().map(|ch| ch.iter().map(Shortcut::from_node).collect()),
        }
    }

    fn to_node(&self) -> cian_lua::shortcuts::Node {
        cian_lua::shortcuts::Node {
            name: self.name.clone(),
            target: self.target.clone(),
            children: self.children.as_ref().map(|ch| ch.iter().map(Shortcut::to_node).collect()),
        }
    }
}

/// The list of shortcuts at `path` (indices to descend through groups). Empty if
/// the path does not resolve.
fn sc_level<'a>(entries: &'a [Shortcut], path: &[usize]) -> &'a [Shortcut] {
    let mut cur = entries;
    for &i in path {
        match cur.get(i).and_then(|s| s.children.as_deref()) {
            Some(ch) => cur = ch,
            None => return &[],
        }
    }
    cur
}

/// Mutable variant of [`sc_level`].
fn sc_level_mut<'a>(entries: &'a mut Vec<Shortcut>, path: &[usize]) -> Option<&'a mut Vec<Shortcut>> {
    let mut cur = entries;
    for &i in path {
        cur = cur.get_mut(i)?.children.as_mut()?;
    }
    Some(cur)
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

/// Build the request-side [`cian_ai::AiConfig`] from the parsed Lua config.
/// The mapping lives in cian-ai now, because the GUI's engine needs the same
/// one — this is the call site, kept for startup and `:reload`.
pub(crate) fn ai_config_from(config: &cian_lua::Config) -> Option<cian_ai::AiConfig> {
    cian_ai::AiConfig::from_lua(config)
}

impl ShortcutStore {
    /// The Lua file bookmarks are stored in now. Portable-aware: a copy next to
    /// the executable wins for both reading and writing (see [`cian_lua`]).
    pub fn default_path() -> PathBuf {
        cian_lua::config_write_path("shortcuts.lua")
            .unwrap_or_else(|| PathBuf::from("shortcuts.lua"))
    }

    /// A legacy `shortcuts.<ext>` to migrate, resolved the same portable-aware
    /// way as the Lua file so a carried-along old file is still found.
    fn legacy_path(ext: &str) -> Option<PathBuf> {
        cian_lua::config_read_path(&format!("shortcuts.{ext}"))
            .filter(|p| p.exists())
    }

    pub fn load_or_default() -> Self {
        // Prefer the Lua file (portable copy first, then the user dir).
        if let Some(lua) = cian_lua::config_read_path("shortcuts.lua").filter(|p| p.exists()) {
            if let Ok(nodes) = cian_lua::shortcuts::load(&lua) {
                return Self { entries: nodes.iter().map(Shortcut::from_node).collect(), path: Self::default_path() };
            }
        }
        // Otherwise migrate a legacy YAML, then a legacy TOML, writing the Lua
        // copy and leaving the old file in place (a harmless safety net).
        let path = Self::default_path();
        for ext in ["yaml", "toml"] {
            let Some(legacy) = Self::legacy_path(ext) else { continue };
            let Ok(text) = std::fs::read_to_string(&legacy) else { continue };
            let parsed = if ext == "yaml" {
                serde_yml::from_str::<ShortcutsFile>(&text).ok()
            } else {
                toml::from_str::<ShortcutsFile>(&text).ok()
            };
            if let Some(file) = parsed {
                let store = Self { entries: file.shortcuts, path };
                let _ = store.save();
                return store;
            }
        }
        Self { entries: Vec::new(), path }
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let nodes: Vec<cian_lua::shortcuts::Node> = self.entries.iter().map(Shortcut::to_node).collect();
        std::fs::write(&self.path, cian_lua::shortcuts::to_lua(&nodes))?;
        Ok(())
    }
}

/// A scrollbar the mouse can take hold of, recorded by the renderer each
/// frame with the geometry the pointer will be measured against.
///
/// The bars were drawn but not remembered, so there was nothing to hit: they
/// reported where you were and could not be used to go anywhere.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScrollTrack {
    pub(crate) rect: Rect,
    pub(crate) what: ScrollWhat,
    /// Rows (or columns) of content the track stands for.
    pub(crate) total: usize,
    /// How much of it is on screen — the thumb's share of the track.
    pub(crate) shown: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollWhat {
    /// A file listing's cursor.
    Pane(FocusedPane),
    /// The editor panel, down and across.
    ViewerRows,
    ViewerCols,
}

/// Which version-control system a pane's directory belongs to. Both report the
/// same [`cian_core::git::RepoStatus`] display type, so the UI is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vcs {
    Git,
    Svn,
}

/// A pane's cached VCS status and the directory it was computed for.
struct GitState {
    cwd: PathBuf,
    /// Which VCS the status came from (`None` when the directory is in neither).
    kind: Option<Vcs>,
    status: Option<cian_core::git::RepoStatus>,
}

#[derive(Debug, Default, Clone, Copy)]
struct LayoutRects {
    left: Rect,
    right: Rect,
    shell: Rect,
}

impl LayoutRects {
    /// The rectangle a pane was drawn in this frame.
    fn for_pane(&self, pane: FocusedPane) -> Rect {
        match pane {
            FocusedPane::Left => self.left,
            FocusedPane::Right => self.right,
            FocusedPane::Shell => self.shell,
        }
    }
}

/// Which split a draggable border adjusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DividerTarget {
    /// The horizontal border between the file panes and the shell panel.
    Main,
    /// The vertical border between the left and right file panes.
    Panes,
    /// A border inside the shell panel's split tree.
    ShellSplit { tab: usize, node: usize },
}

/// A border the user can grab and drag to re-proportion a split. Rebuilt every
/// frame during rendering, since it depends on the current geometry.
#[derive(Debug, Clone, Copy)]
struct Divider {
    /// The band of cells that counts as grabbing this border.
    zone: Rect,
    /// The area being divided; the drag position is mapped into this.
    parent: Rect,
    /// Whether the border moves horizontally or vertically.
    dir: Direction,
    target: DividerTarget,
}

impl Divider {
    /// Convert an absolute mouse position into a percentage for the first
    /// child, clamped so neither side can be squeezed out of existence.
    fn ratio_at(&self, col: u16, row: u16) -> u16 {
        let (pos, start, len) = match self.dir {
            Direction::Horizontal => (col, self.parent.x, self.parent.width),
            Direction::Vertical => (row, self.parent.y, self.parent.height),
        };
        if len == 0 {
            return 50;
        }
        let offset = pos.saturating_sub(start).min(len);
        let pct = (offset as u32 * 100 / len as u32) as u16;
        pct.clamp(MIN_SPLIT_PCT, 100 - MIN_SPLIT_PCT)
    }
}

/// Neither side of a split may shrink below this share of its parent, so a
/// border can never be dragged far enough to make a pane unusable.
const MIN_SPLIT_PCT: u16 = 15;

/// How often the panes are checked against the filesystem. Long enough to be
/// invisible in cost, short enough that a file appearing feels immediate.
const WATCH_INTERVAL: Duration = Duration::from_millis(1200);

/// How many copy/move destinations to remember.
const DEST_HISTORY_CAP: usize = 15;

/// How long an operation flash stays visible.
const FLASH_SECS: f32 = 0.45;

/// Default transition length. Long enough to read as motion, short enough that
/// it never gets in the way of fast keyboard work.
const DEFAULT_ANIM_MS: u64 = 150;

/// A layout transition in flight.
///
/// Transitions are *purely visual*: PTYs keep their old size for the duration
/// and are resized exactly once, when the transition lands. Resizing a PTY per
/// frame would send a SIGWINCH storm to the shell and make it reflow a dozen
/// times, which looks far worse than the animation looks good.
#[derive(Debug, Clone, Copy)]
struct Anim {
    kind: AnimKind,
    start: Instant,
    dur: Duration,
}

#[derive(Debug, Clone, Copy)]
enum AnimKind {
    /// A surface growing to fill the window, or shrinking back out of it.
    Zoom { from: Rect, to: Rect },
    /// One shell split pane growing to fill the shell panel (Shift+F12), or
    /// shrinking back into its slot. Like `Zoom`, but the backdrop keeps the
    /// splits so the pane visibly grows out of them.
    PaneZoom { from: Rect, to: Rect },
    /// A split's ratio easing between two values — used both when a split is
    /// created (the new pane grows in) and when one is closed (it shrinks away).
    Ratio { target: DividerTarget, from: u16, to: u16 },
}

impl Anim {
    /// Eased 0.0..=1.0 position through the transition.
    fn progress(&self) -> f32 {
        if self.dur.is_zero() {
            return 1.0;
        }
        let t = (self.start.elapsed().as_secs_f32() / self.dur.as_secs_f32()).clamp(0.0, 1.0);
        // Ease-out cubic: quick to start, settling gently. Reads as "snappy"
        // rather than "slow" at these durations.
        1.0 - (1.0 - t).powi(3)
    }

    fn done(&self) -> bool {
        self.start.elapsed() >= self.dur
    }
}


/// Linear interpolation between two rects at eased position `t`.
fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    let f = |x: u16, y: u16| -> u16 {
        (x as f32 + (y as f32 - x as f32) * t).round().max(0.0) as u16
    };
    Rect {
        x: f(a.x, b.x),
        y: f(a.y, b.y),
        width: f(a.width, b.width).max(1),
        height: f(a.height, b.height).max(1),
    }
}

/// Rendering overrides applied while a transition is in flight.
#[derive(Debug, Default, Clone, Copy)]
struct AnimOverride {
    /// Use this ratio instead of the divider's stored one.
    ratio: Option<(DividerTarget, u16)>,
    /// Leave PTY sizes alone; they are applied once the transition lands.
    freeze_pty: bool,
    /// Draw the shell's split panes even when one is flagged maximized — used
    /// while a pane-zoom transition floats the growing pane above the splits.
    show_splits: bool,
}

impl AnimOverride {
    /// The ratio to render `target` at: the override if it applies, else the
    /// stored value clamped to a usable range.
    fn ratio_for(&self, target: DividerTarget, stored: u16) -> u16 {
        match self.ratio {
            Some((t, r)) if t == target => r,
            _ => stored.clamp(MIN_SPLIT_PCT, 100 - MIN_SPLIT_PCT),
        }
    }
}

/// Files being dragged from one pane to another.
///
/// cian cannot take part in the OS's drag and drop — a console application has
/// no window to be a drag source or target — but it owns the mouse events
/// inside its own surface, so dragging between its panes works.
#[derive(Debug, Clone)]
struct FileDrag {
    from: FocusedPane,
    paths: Vec<PathBuf>,
    /// Where the pointer is now, so the drop target can be highlighted.
    over: Option<FocusedPane>,
    /// True once the pointer has actually moved; a press and release without
    /// motion is a click, not a drag.
    moved: bool,
    /// The entry index the drag started on, kept so a drop can tell where the
    /// gesture began.
    anchor: usize,
}

pub struct App {
    pub left: PaneTabs,
    pub right: PaneTabs,
    pub shell: ShellPane,
    pub focused: FocusedPane,
    pub mode: Mode,
    pub command_buffer: String,
    /// In-progress text for [`Mode::Filter`].
    pub filter_buffer: String,
    pub message: Option<String>,
    pub last_file_pane: FocusedPane,
    pub should_quit: bool,
    pub visual_anchor: Option<usize>,
    clipboard: Option<arboard::Clipboard>,
    popup: Popup,
    layout_rects: LayoutRects,
    /// Percentage of the window given to the file panes; the shell gets the
    /// rest. Adjusted by dragging the border between them.
    main_pct: u16,
    /// Percentage of the file-pane area given to the left pane.
    panes_pct: u16,
    /// Draggable borders for the current frame, rebuilt during rendering.
    dividers: Vec<Divider>,
    /// `(tab, leaf, outer rect, inner PTY rect)` for each shell split pane on
    /// screen, so a click can land on the pane under the pointer and a drag can
    /// map to a terminal cell.
    shell_leaves: Vec<(usize, usize, Rect, Rect)>,
    /// Clickable tab-label rects, rebuilt each frame: which pane's strip, the
    /// tab index, and where it sits, so a tab can be switched with the mouse.
    tab_rects: Vec<(FocusedPane, usize, Rect)>,
    /// Clickable column-header rects (`Name`/`Size`/`Date`), rebuilt each
    /// frame; a click sorts by that column, a repeat flips the direction.
    sort_rects: Vec<(FocusedPane, cian_core::SortKey, Rect)>,
    /// Clickable path-segment rects on the active tab's title (a breadcrumb).
    /// The `usize` is how many trailing components to strip from the cwd.
    crumb_rects: Vec<(FocusedPane, usize, Rect)>,
    /// The ◀ / ▶ history arrows at the head of each pane title; the bool is
    /// true for forward.
    nav_rects: Vec<(FocusedPane, bool, Rect)>,
    /// Member list of the archive being browsed (`Enter` on a zip/tar),
    /// cached so navigation inside it does not re-scan — see [`arcview`].
    archive_cache: Option<arcview::ArchiveCache>,
    /// Show the invisible characters in the viewer: a trailing space, an
    /// ideographic space, a tab. Off by default (they are noise while reading)
    /// and turned on for the pass where they matter — `:ws`, or the toggles.
    show_ws: bool,
    /// The column ruler and the crosshair on the cursor's line and column.
    /// On by default: knowing which column you are in is most of what a fixed
    /// -width record is about, and counting them by eye is what the ruler
    /// exists to stop.
    show_ruler: bool,
    /// Cursor-follow preview (`:preview`): while on, the shell panel's area
    /// previews the file under the cursor whenever a file pane has focus.
    preview_on: bool,
    /// The loaded preview, cached by path (see [`preview::PreviewState`]).
    preview: Option<preview::PreviewState>,
    /// Protocol state for a previewed image, separate from the F3 popup's
    /// `img_proto` (which is cleared whenever that popup is closed).
    preview_gfx: Option<(PathBuf, ratatui_image::protocol::StatefulProtocol)>,
    /// Ask the main loop to wipe the terminal before the next frame.
    ///
    /// Terminal graphics (kitty / iTerm2 / sixel) paint into a layer the cell
    /// buffer does not model, so ratatui's damage tracking has no reason to
    /// repaint over a picture — leaving it stuck on screen above whatever came
    /// next. Every place an image stops being shown sets this, and the loop
    /// pays for one full clear exactly then.
    full_clear: bool,
    /// Ask the main loop to repaint every cell — without wiping the screen.
    ///
    /// The weaker half of [`App::full_clear`], and the one nearly everything
    /// actually wanted. A popup opening or closing needs the surface painted
    /// again, because a glyph whose ink overhangs its cell leaves the overhang
    /// behind (see [`App::popup_shape`]); it does not need the screen blanked
    /// first. It was blanking it anyway, and `Terminal::clear` is expensive in
    /// a way that shows: it asks the terminal where the cursor is and waits for
    /// the reply, wipes every cell, then writes the whole surface back. On a
    /// large window that last write is tens of kilobytes and the terminal
    /// paints it as it arrives — so every popup, `c` to copy included, flashed
    /// black and filled back in. Repainting over what is already there has no
    /// blank moment to see.
    full_repaint: bool,
    /// Terminal-graphics capability, when the terminal answered the startup
    /// query with a real protocol (kitty / iTerm2 / sixel). `None` falls back
    /// to the half-block cell renderer — including always in tests, which
    /// never query a terminal.
    gfx_picker: Option<ratatui_image::picker::Picker>,
    /// The decoded image + protocol state for the open image preview, keyed by
    /// path so a new image re-decodes. Lives outside `Popup` because the
    /// protocol state is neither `Debug` nor comparable.
    img_proto: Option<(PathBuf, ratatui_image::protocol::StatefulProtocol)>,
    /// The terminal advertised an image protocol and then would not draw with
    /// it. Set once and kept: it fails every frame, and half-blocks are a
    /// worse picture rather than no picture.
    gfx_failed: bool,
    /// The context menu's on-screen rect (inner area), for clicking its items.
    menu_rect: Rect,
    /// Parent context menus stashed while a submenu is open, so Esc/← drills
    /// back up instead of closing everything.
    menu_stack: Vec<Popup>,
    /// The viewer's text body rect, for mapping a mouse click to a line.
    /// `:key` — report every keystroke as cian received it, for finding out
    /// whether a binding is broken or the terminal simply never sent the key.
    /// The viewer's other open files. The active one is `popup`; these are the
    /// rest, in order, with `viewer_tab_idx` saying where the active one sits
    /// among them. Whole `Popup::Viewer` values, so a tab keeps its cursor,
    /// its folds and its unsaved edits while another is on screen.
    viewer_tabs: Vec<Popup>,
    viewer_tab_idx: usize,
    /// The other half of a split viewer: a whole second `Popup::Viewer`, drawn
    /// beside the first. `viewer_split_lr` says which way they are stacked and
    /// `viewer_split_focus` which one the keyboard is pointed at.
    viewer_split: Option<Box<Popup>>,
    viewer_split_lr: bool,
    viewer_split_focus: bool,
    /// The viewer put aside while a menu — or something the menu opened — is on
    /// screen, so choosing "ask the AI about this" does not throw away the file
    /// (and its unsaved edits) that the question was about.
    viewer_return: Option<Box<Popup>>,
    /// `=` in a split: the two halves compared, one mark per line of each.
    /// Recomputed whenever either buffer moves on, so it stays true while both
    /// are being edited — which is the whole point of doing it here rather
    /// than in a diff window.
    viewer_diff: Option<Box<ViewerDiff>>,
    /// What was last yanked in the viewer, kept inside cian as well as on the
    /// system clipboard. A machine reached over SSH often has no clipboard
    /// service at all, and copy-and-paste within one file should not depend on
    /// one being there.
    yank: Option<String>,
    /// Temp files that came out of an archive, and where they came from.
    /// Keyed by the temp path so a tab keeps its origin across a switch —
    /// saving one writes it back into the zip rather than leaving the edit in
    /// a temporary file nobody will look at again.
    arc_edits: std::collections::HashMap<PathBuf, (PathBuf, String)>,
    key_probe: bool,
    /// The message was raised by the last keystroke, rather than left over
    /// from an earlier one. Only a fresh message may take a footer.
    message_fresh: bool,
    /// Whether the terminal accepted the enhanced-keyboard request at startup.
    /// Reported by `:key`, because it is the first thing to suspect when every
    /// Ctrl combination goes quiet at once.
    kbd_enhanced: bool,
    /// Whether the "your IME is on" note has been made for the run of keys
    /// currently arriving. Cleared by the first key an input method could not
    /// have produced. See [`App::note_ime_is_on`].
    ime_warned: bool,
    /// Whether notepad style is in the middle of a run of typed characters,
    /// for coalescing them into one undo step. See `notepad_editor_key`.
    notepad_typing: bool,
    /// Which grammar the editor panel answers to. See [`EditStyle`].
    ///
    /// Live, and on `App` rather than on the panel: it is a property of who is
    /// sitting at the keyboard, not of the file they happen to have open, so it
    /// must not reset when the panel closes. `T` flips it; `init.lua` sets the
    /// one a machine starts with.
    edit_style: EditStyle,
    /// The viewer's bordered frame, whose top row holds the tab arrows.
    viewer_frame: Rect,
    viewer_rect: Rect,
    /// Where the viewer's outline column was drawn, so a click on an entry can
    /// jump to it. Zero-width when the column is not showing.
    outline_rect: Rect,
    /// Where each of the viewer's tabs was drawn in its title bar, so one can
    /// be clicked. Rebuilt every frame.
    viewer_tab_rects: Vec<(Rect, usize)>,
    /// Where the viewer's ✕ button was drawn, so a click can find it.
    viewer_close_rect: Rect,
    /// vi's marks — `ma` here, `'a` back — kept per file, so a mark set in
    /// one is not jumped to in another.
    vim_marks: std::collections::HashMap<(PathBuf, char), (usize, usize)>,
    /// Where the cursor was before each far jump (`G`, a search, a mark), and
    /// how far back through them `Ctrl+O` has walked.
    vim_jumps: Vec<(PathBuf, usize, usize)>,
    vim_jump_at: usize,
    /// The keys of the last change, replayed by `.`. Recorded while a command
    /// that alters the file is being typed — including what was typed into
    /// the editor, which is most of what `.` is for.
    vim_last_change: Option<Vec<KeyEvent>>,
    /// `m`, `'` or a backtick, waiting for the letter that names the mark.
    vim_mark_wait: Option<char>,
    /// True while `.` is replaying, so the replay is not recorded as itself.
    vim_replaying: bool,
    /// The terminal font size cian last asked for — see `font.rs`.
    font_level: i64,
    /// Which pane the viewer is docked in, when `Enter` opened it there
    /// rather than over the whole window. The viewer is the same viewer
    /// either way — it is only drawn somewhere smaller, and the pane it is
    /// docked in gets its listing back when the file closes.
    viewer_dock: Option<FocusedPane>,
    vim_recording: Option<Vec<KeyEvent>>,
    vim_obj: Option<char>,
    vim_wait: Option<char>,
    vim_last_find: Option<(char, char)>,
    /// `r` waiting for the character to stamp in, with the count it was given
    /// (`3rx` overwrites three).
    vim_replace: Option<usize>,
    /// A heavy preview waiting for the cursor to settle: what it is for, and
    /// when it was first asked for.
    preview_wanted: Option<(PathBuf, std::time::Instant)>,
    /// A picture being decoded on another thread, and the file it is for.
    ///
    /// Decoding is nearly all of the cost of showing one — several megabytes
    /// of PNG unpacked whole before anything can be scaled — and doing it
    /// while drawing a frame takes that time out of the interface. The cursor
    /// stopped dead on every large image.
    preview_decode: Option<(PathBuf, std::sync::mpsc::Receiver<Option<image::DynamicImage>>)>,
    /// The scrollbars on screen, for the mouse to grab.
    scroll_tracks: Vec<ScrollTrack>,
    /// The bar being dragged, so the pointer keeps it after leaving the track.
    scroll_drag: Option<ScrollWhat>,
    /// Consecutive Escs pressed in the panel with nothing left to peel off.
    /// One is a mistake, three in a row is a decision — and `:q!` is a lot to
    /// type when the answer is "get me out of here".
    viewer_escapes: u8,
    /// Which key that run is made of, so Esc and Backspace count separately.
    viewer_escape_key: Option<crossterm::event::KeyCode>,
    /// Whether the system clipboard actually took the last thing yanked.
    ///
    /// Writing it can fail — another program holding the pasteboard, a
    /// machine with no clipboard service at all — and the failure used to be
    /// discarded, which left `p` preferring a *stale* clipboard over the text
    /// just copied. The copy looked like it had worked and the paste produced
    /// something else entirely.
    yank_on_clipboard: bool,
    /// The two halves of a split, as drawn: clicking the one not in focus
    /// crosses to it.
    viewer_half_rects: [Rect; 2],
    /// The viewer's line-number gutter width, so a click maps to a char column.
    viewer_gutter: u16,
    /// Clickable regions of whatever popup is on screen, rebuilt every frame by
    /// `draw_popup`, so dialogs and pickers can be driven entirely by mouse.
    popup_zones: Vec<PopupZone>,
    /// The last copy/move that failed on a permission error, kept so a Windows
    /// user can retry it elevated. `(op, sources, destination dir)`.
    pending_elevation: Option<(PendingOp, Vec<PathBuf>, PathBuf)>,
    /// The active shell pane's slot rect, stashed on pane-zoom so the shrink
    /// back knows where to land (the split rects are gone while zoomed).
    pane_zoom_return: Option<Rect>,
    /// Wall-clock start, used to phase the slow "recording" border pulse.
    started: Instant,
    /// The local side of an SFTP transfer being set up, carried through the SSH
    /// host/user picker: `(direction, files to upload, local save dir)`.
    scp_dir: Option<(ScpDir, Vec<PathBuf>, PathBuf)>,
    /// SFTP connection backing a remote pane, per file-pane side (Left=0,
    /// Right=1). Set while that pane is browsing a host; cleared on leave.
    remote_targets: [Option<(cian_scp::Target, String)>; 2],
    /// A pending remote mutation (mkdir/touch/rename/delete): which side to
    /// re-list, and the result message channel.
    #[allow(clippy::type_complexity)]
    remote_mut: Option<(FocusedPane, std::sync::mpsc::Receiver<Result<String, String>>)>,
    /// A remote-pane directory listing in flight, tagged with the side it fills.
    remote_pane_ls: Option<(FocusedPane, RemoteLsRx)>,
    /// A remote file being downloaded to a temp path so `F3` can view it.
    remote_view: Option<RemoteView>,
    /// Runtime overrides for two config options, flipped from the toggles menu
    /// (`T`). `None` = follow init.lua; `Some(v)` = the user's live choice.
    notify_runtime: Option<bool>,
    verify_runtime: Option<bool>,
    /// Files opened in the viewer this session, most-recent first, for the recent
    /// entries in the file finder (`:files` / `:recent`).
    recent_files: Vec<PathBuf>,
    /// Local temp files opened from a remote pane (F3), mapped to where they came
    /// from, so saving one uploads it back. `(target, remote absolute path)`.
    remote_edits: std::collections::HashMap<PathBuf, (cian_scp::Target, String)>,
    /// A remote pane side to re-list once the running op finishes (e.g. after an
    /// upload landed files on it), since a synthetic listing does not auto-refresh.
    remote_refresh: Option<FocusedPane>,
    /// An SFTP transfer whose remote path is being entered, if any.
    scp_pending: Option<ScpPending>,
    /// Per-file chmod modes collected while prompting an upload one file at a
    /// time (index-aligned with `scp_pending.locals`).
    scp_upload_modes: Vec<Option<u32>>,
    /// The server connection being browsed for a download, reused for the
    /// directory listings and the transfer itself.
    scp_target: Option<(cian_scp::Target, String)>,
    /// A pending remote directory listing: the worker sends `(cwd, entries)` or
    /// an error message. Polled from the main loop.
    remote_ls: Option<RemoteLsRx>,
    /// Time and row of the last left-click in a file pane, to detect a
    /// double-click (which activates the entry).
    last_click: Option<(Instant, u16)>,
    /// An in-progress or finished text selection in a shell pane (its own
    /// selection, since cian holds the mouse the terminal would otherwise use).
    shell_sel: Option<ShellSel>,
    /// The border currently being dragged, if any.
    drag: Option<Divider>,
    /// Files picked up by the mouse and not yet dropped.
    file_drag: Option<FileDrag>,
    /// Directories recently copied or moved into, most recent first.
    dest_history: Vec<PathBuf>,
    /// Files awaiting a paste (see [`FileClipboard`]).
    file_clip: Option<FileClipboard>,
    /// Pane to briefly highlight after an operation landed there, and when it
    /// started. Makes it obvious *where* a copy/move/delete took effect.
    flash: Option<(FocusedPane, Instant)>,
    /// Layout transition in flight, if any.
    anim: Option<Anim>,
    /// Work to run when the current transition finishes (e.g. actually closing
    /// the pane that just finished shrinking away).
    anim_then: Option<PendingClose>,
    /// When the recording border last pulsed. State rather than a loop-local
    /// because both event loops — the terminal one and the windowed one — run
    /// the same `tick_background`, and the throttle has to survive between
    /// their turns.
    last_pulse: Instant,
    /// The last "still blank" second written to the log, so each is said once.
    blank_said: u64,
    /// What was on top last frame, and how far it was scrolled.
    ///
    /// A popup covers cells it does not own, and every renderer under cian
    /// repaints only what changed. When one opens, closes or scrolls, what
    /// changed is "most of the screen, in a way the cell diff cannot always
    /// see" — a glyph whose ink overhangs its cell leaves the overhang behind,
    /// and the leftovers pile up as white blocks along the lines and stay on
    /// the panes after the popup has gone. So the frame that changes a popup
    /// asks for the whole surface to be repainted, which is cheap once and
    /// exact.
    popup_shape: Option<(std::mem::Discriminant<Popup>, usize)>,
    /// Transition length; zero disables animation.
    anim_dur: Duration,
    /// The focused surface's rect from before it was zoomed.
    ///
    /// While zoomed, `layout_rects` describes the zoomed layout — the focused
    /// surface fills the window and the others are empty — so the rect to
    /// shrink back into is not recoverable from it and has to be kept.
    zoom_return: Option<Rect>,
    /// Show the contextual key-hint bar.
    show_key_hints: bool,
    /// How fast a transfer to or from a server may go, in bytes a second.
    ///
    /// `None` is "as fast as the link allows", which is the default and the
    /// wrong answer on somebody else's network in the middle of the day. Set
    /// with `cian.set_option("transfer_limit", "2M")` or `:limit 2M`.
    transfer_limit: Option<u64>,
    /// The active theme's preset name (e.g. "dracula"), or "custom" when the
    /// config tweaked colors past any named preset. Drives the `:theme` picker's
    /// initial highlight and the status readout.
    theme_name: String,
    /// Interface language for the key manual (Japanese by default).
    lang: Lang,
    /// Language for the key manual and the right-click menu specifically —
    /// `menu_lang` overrides `lang` for those two surfaces; else follows `lang`.
    menu_lang: Lang,
    /// Whether init.lua named `menu_lang` outright.
    ///
    /// If it did, it keeps it: switching languages from the menu moves
    /// everything else and leaves those two surfaces where they were asked to
    /// be. If it did not, they follow — which is what "switch to English" has
    /// to mean, and did not: the menu that the switch was chosen *from* stayed
    /// in Japanese, because it reads `menu_lang` and only `lang` was moved.
    menu_lang_pinned: bool,
    /// Cached git status per file pane `[left, right]`, recomputed when the
    /// pane's directory changes or on an explicit refresh.
    git: [Option<GitState>; 2],
    /// Cached free/total disk space of each file pane's mount `[left, right]`,
    /// refreshed alongside `git` when the pane's directory changes or after a
    /// file operation. `Some(cwd, None)` remembers a mount that could not be
    /// queried, so we don't re-probe it every frame.
    disk: [Option<(PathBuf, Option<cian_core::disk::Usage>)>; 2],
    /// A command to type into the shell once it is ready. Needed because the
    /// PTY spawns on a background thread, so the shell may not exist yet at
    /// the moment the user picks a connection.
    pending_shell_input: Option<String>,
    /// A target path chosen for a shortcut being added from somewhere other
    /// than the file cursor (e.g. the history list), consumed by the name step.
    pending_shortcut_target: Option<String>,
    /// A password waiting for ssh to ask for it. See [`PendingAuth`].
    pending_auth: Option<PendingAuth>,
    /// A copy/move/delete running on a worker thread.
    op_job: Option<OpJob>,
    /// Operations waiting for the running one to finish, oldest first.
    op_queue: std::collections::VecDeque<QueuedOp>,
    /// The progress popup was dismissed (`b`/Enter) to keep working while the
    /// op runs; the status line carries a chip instead. Reset when the queue
    /// drains.
    op_bar_hidden: bool,
    /// A recursive search running on a worker thread.
    find_job: Option<FindJob>,
    /// The grep-results popup stashed while viewing one hit in F3, so Esc from
    /// the viewer returns to the list rather than closing everything.
    find_return: Option<Box<Popup>>,
    /// AI helper config from `cian.ai{...}`; `None` disables every AI feature.
    ai: Option<cian_ai::AiConfig>,
    /// Whether the AI helper actually works (python + packages + sign-in),
    /// probed once on a background thread at startup and cached. `None` until
    /// the probe lands (treated as "not ready yet" — the AI menu stays hidden).
    ai_ready: Option<bool>,
    /// The in-flight AI availability probe (see [`App::spawn_ai_probe`]). Polled
    /// from the main loop; the check must never block the UI thread — it spawns
    /// python, which can take seconds on the first run.
    ai_probe: Option<std::sync::mpsc::Receiver<bool>>,
    /// When the app started, for the brief "starting up" splash.
    startup_at: Instant,
    /// A pending AI request running on a worker thread.
    ai_job: Option<AiJob>,
    /// Which way the input-method switch is currently thrown (`None` until
    /// the first sync). See `ime.rs`.
    ime_on: Option<bool>,
    /// A pending `:searchfiles` corpus search — its (name, path) hits, or an
    /// error — to panelize into the active pane when it lands.
    #[allow(clippy::type_complexity)]
    /// Past AI conversations this session, newest first, for the history
    /// picker (`Ctrl+R` in the chat). Each is a transcript plus the backend it
    /// spoke to, so reopening one still routes follow-ups correctly.
    ai_history: Vec<ai::StoredChat>,
    /// The chat transcript's on-screen body rect, the effective scroll offset,
    /// and the flat wrapped lines — rebuilt each frame so a mouse drag can map
    /// to a line range and copy it.
    ai_rect: Rect,
    ai_scroll: usize,
    ai_lines: Vec<String>,
    /// Images pasted into the open chat (Ctrl+V), as paths to temp PNGs. Sent
    /// with the next question and cleared then; also cleared on a new chat.
    chat_attachments: Vec<std::path::PathBuf>,
    /// The junk-review list body rect, stashed so a click can map to a row.
    junk_rect: Rect,
    /// The structure-review list body rect, for the same reason.
    struct_rect: Rect,
    /// The rename-review list body rect, for the same reason.
    rename_rect: Rect,
    /// The dupe-review list body rect, for the same reason.
    dupe_rect: Rect,
    /// A running duplicate scan, delivering its groups when finished.
    dupes_job: Option<std::sync::mpsc::Receiver<Vec<Vec<PathBuf>>>>,
    diff_job: Option<DiffJob>,
    /// When the panes were last checked against the filesystem.
    last_watch: Instant,
    /// Per-pane background overrides, indexed by [`Self::bg_slot`].
    /// Session-only: deliberately not persisted.
    pane_bg: [Option<Color>; 2],
    /// Per-pane theme override (#8) by preset name, indexed [left, right]. `None`
    /// follows the whole-app theme. Session-only.
    pane_theme: [Option<String>; 2],
    last_search_query: Option<String>,
    pub shortcuts: ShortcutStore,
    /// User-defined macros loaded from `macro.lua` (portable-aware).
    macros: Vec<cian_lua::macros::Macro>,
    /// Why `macro.lua` failed to load, if it did — shown when the menu is empty.
    macro_error: Option<String>,
    /// A layout macro currently building itself out across ticks.
    macro_run: Option<macro_run::MacroRun>,
    /// File/step-counter settings from `count.lua` (portable-aware).
    count_opts: cian_core::count::Options,
    /// A running count, delivering its report when finished.
    count_job: Option<std::sync::mpsc::Receiver<cian_core::count::Report>>,
    /// A disk-usage analysis in flight (its directory + the sized children).
    du_job: Option<std::sync::mpsc::Receiver<(PathBuf, Vec<cian_core::du::DuEntry>)>>,
    /// The file finder's tree walk, running while its picker is already open.
    file_scan: Option<crate::palette::FileScan>,
    /// A file the user asked to edit; the main loop suspends the TUI, runs the
    /// external editor, and restores. See [`crate::edit`].
    pending_edit: Option<edit::PendingEdit>,
    /// Reversible operations, newest last; `u` undoes the last one.
    undo_stack: Vec<UndoAction>,
    /// What `Ctrl+Y` / `:redo` would put back. Emptied by any new action, as
    /// a redo chain is everywhere else.
    redo_stack: Vec<UndoAction>,
    /// Set by the routes that are *already* an undo — back, forward, undo,
    /// redo — so stepping through history does not itself become history.
    nav_suppressed: bool,
    pending_g: bool,
    /// When true, only the focused surface is drawn, filling the window.
    pub zoomed: bool,
    /// When CIAN_DEBUG_KEYS is set, show each shell keypress in the status bar.
    debug_keys: bool,
    config: Config,
    /// User keymap overrides: plain character keys (no Ctrl) the user bound via
    /// `cian.set_keymap`. Only contains entries the user set; everything else
    /// falls through to the built-in defaults.
    keymap: HashMap<(char, KeyModifiers), Action>,
}

impl App {
    pub fn new(left: PathBuf, right: PathBuf, config: Config) -> Result<Self> {
        // Build the keymap from user overrides (invalid action names are
        // validated and reported separately in `run`).
        let mut keymap: HashMap<(char, KeyModifiers), Action> = HashMap::new();
        for (spec, name) in &config.keymaps {
            if let (Some(k), Some(a)) = (crate::theme::parse_key_spec(spec), action_from_name(name)) {
                keymap.insert(k, a);
            }
        }
        let shell_cmd = config
            .options
            .shell
            .clone()
            .unwrap_or_else(cian_pty::default_shell);
        // Honour the show_hidden option on the initial panes (it defaults to
        // true, cian's long-standing behaviour).
        let show_hidden = config.options.show_hidden.unwrap_or(true);
        let mut left_pane = Pane::new(left)?;
        let mut right_pane = Pane::new(right)?;
        left_pane.set_show_hidden(show_hidden);
        right_pane.set_show_hidden(show_hidden);
        let (macros, macro_error) = macro_run::load_macros();
        let count_opts = count::load_count_opts();
        Ok(Self {
            left: PaneTabs::single(left_pane),
            right: PaneTabs::single(right_pane),
            shell: ShellPane::new(shell_cmd),
            focused: FocusedPane::Left,
            mode: Mode::Normal,
            command_buffer: String::new(),
            filter_buffer: String::new(),
            message: None,
            last_file_pane: FocusedPane::Left,
            should_quit: false,
            visual_anchor: None,
            clipboard: arboard::Clipboard::new().ok(),
            popup: Popup::None,
            layout_rects: LayoutRects::default(),
            main_pct: 60,
            panes_pct: 50,
            dividers: Vec::new(),
            shell_leaves: Vec::new(),
            last_click: None,
            shell_sel: None,
            tab_rects: Vec::new(),
            menu_rect: Rect::new(0, 0, 0, 0),
            menu_stack: Vec::new(),
            viewer_tabs: Vec::new(),
            viewer_tab_idx: 0,
            viewer_split: None,
            viewer_split_lr: true,
            viewer_split_focus: false,
            viewer_return: None,
            viewer_diff: None,
            yank: None,
            arc_edits: std::collections::HashMap::new(),
            key_probe: false,
            message_fresh: false,
            kbd_enhanced: false,
            ime_warned: false,
            notepad_typing: false,
            // Vim unless a machine says otherwise. cian's editor is vi-shaped
            // and the person who reached for cian in the first place is very
            // likely to want that; the other grammar is for the colleague they
            // hand it to.
            edit_style: config
                .options
                .edit_style
                .as_deref()
                .and_then(EditStyle::from_name)
                .unwrap_or(EditStyle::Vim),
            viewer_frame: Rect::new(0, 0, 0, 0),
            viewer_rect: Rect::new(0, 0, 0, 0),
            outline_rect: Rect::new(0, 0, 0, 0),
            viewer_tab_rects: Vec::new(),
            viewer_close_rect: Rect::new(0, 0, 0, 0),
            vim_marks: std::collections::HashMap::new(),
            vim_jumps: Vec::new(),
            vim_jump_at: 0,
            vim_last_change: None,
            vim_mark_wait: None,
            vim_replaying: false,
            font_level: config.font.as_ref().map(|f| f.start).unwrap_or(0),
            viewer_dock: None,
            vim_recording: None,
            vim_obj: None,
            vim_wait: None,
            vim_last_find: None,
            vim_replace: None,
            preview_wanted: None,
            preview_decode: None,
            scroll_tracks: Vec::new(),
            scroll_drag: None,
            viewer_escapes: 0,
            viewer_escape_key: None,
            yank_on_clipboard: false,
            viewer_half_rects: [Rect::new(0, 0, 0, 0); 2],
            viewer_gutter: 0,
            popup_zones: Vec::new(),
            pending_elevation: None,
            pane_zoom_return: None,
            started: Instant::now(),
            scp_dir: None,
            remote_targets: [None, None],
            remote_mut: None,
            remote_pane_ls: None,
            remote_view: None,
            notify_runtime: None,
            verify_runtime: None,
            recent_files: Vec::new(),
            remote_edits: std::collections::HashMap::new(),
            remote_refresh: None,
            scp_pending: None,
            scp_upload_modes: Vec::new(),
            scp_target: None,
            remote_ls: None,
            drag: None,
            file_drag: None,
            dest_history: Vec::new(),
            file_clip: None,
            flash: None,
            anim: None,
            anim_then: None,
            last_pulse: Instant::now(),
            blank_said: 0,
            popup_shape: None,
            anim_dur: Duration::from_millis(
                config.options.animation_ms.unwrap_or(DEFAULT_ANIM_MS),
            ),
            show_key_hints: config.options.key_hints.unwrap_or(true),
            // `theme::install` has already set the active theme from the config.
            transfer_limit: config.options.transfer_limit.as_deref().and_then(parse_rate),
            theme_name: theme_name_of(&theme()).unwrap_or("custom").to_string(),
            lang: Lang::from_opt(config.options.lang.as_deref()),
            menu_lang: match config.options.menu_lang.as_deref() {
                Some(s) => Lang::from_opt(Some(s)),
                None => Lang::from_opt(config.options.lang.as_deref()),
            },
            menu_lang_pinned: config.options.menu_lang.is_some(),
            git: [None, None],
            disk: [None, None],
            ai: ai_config_from(&config),
            ai_ready: None,
            ai_probe: None,
            startup_at: Instant::now(),
            ai_job: None,
            ime_on: None,
            ai_history: Vec::new(),
            ai_rect: Rect::new(0, 0, 0, 0),
            junk_rect: Rect::new(0, 0, 0, 0),
            struct_rect: Rect::new(0, 0, 0, 0),
            rename_rect: Rect::new(0, 0, 0, 0),
            dupe_rect: Rect::new(0, 0, 0, 0),
            dupes_job: None,
            ai_scroll: 0,
            ai_lines: Vec::new(),
            chat_attachments: Vec::new(),
            sort_rects: Vec::new(),
            crumb_rects: Vec::new(),
            nav_rects: Vec::new(),
            gfx_picker: None,
            img_proto: None,
            gfx_failed: false,
            show_ws: true,
            show_ruler: true,
            // **Off unless asked for, in both front ends.** The window has
            // said so from the start ("reading every file the cursor passes
            // over is a lot of disk for a feature you want on the ten seconds
            // you are looking for something"), and this said the opposite —
            // so the same init.lua, with nothing written in it, gave two
            // different windows. 2026-09-06, his call: match the window.
            // `:preview` and the T menu turn it on, and `cian.set_option
            // ("preview", true)` makes that the default again.
            preview_on: config.options.preview.unwrap_or(false),
            preview: None,
            preview_gfx: None,
            full_clear: false,
            full_repaint: false,
            archive_cache: None,
            zoom_return: None,
            pending_shell_input: None,
            pending_shortcut_target: None,
            pending_auth: None,
            op_job: None,
            op_queue: std::collections::VecDeque::new(),
            op_bar_hidden: false,
            find_job: None,
            find_return: None,
            diff_job: None,
            last_watch: Instant::now(),
            pane_bg: [None, None],
            pane_theme: [None, None],
            last_search_query: None,
            shortcuts: ShortcutStore::load_or_default(),
            macros,
            macro_error,
            macro_run: None,
            count_opts,
            count_job: None,
            du_job: None,
            file_scan: None,
            pending_edit: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            nav_suppressed: false,
            pending_g: false,
            zoomed: false,
            debug_keys: std::env::var("CIAN_DEBUG_KEYS").is_ok(),
            config,
            keymap,
        })
    }

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

    /// Is the active pane looking inside an archive? Six places ask, and the
    /// answer decides whether a file operation goes to disk or to the zip.
    pub(crate) fn in_archive(&self) -> bool {
        self.active_pane().map(|p| p.archive_view().is_some()).unwrap_or(false)
    }

    /// Where the active pane is, if there is one.
    pub(crate) fn cwd(&self) -> Option<PathBuf> {
        self.active_pane().map(|p| p.cwd.clone())
    }

    /// What an operation acts on: the marked entries, or the one under the
    /// cursor when nothing is marked. Never `..`.
    ///
    /// Seven callers asked the pane for this, all spelling it the same way.
    pub(crate) fn target_paths(&self) -> Vec<PathBuf> {
        self.active_pane().map(|p| p.target_paths()).unwrap_or_default()
    }
    fn active_pane_mut(&mut self) -> Option<&mut Pane> {
        self.active_file_tabs_mut().map(|t| t.active_mut())
    }

    fn opposite_pane_cwd(&self) -> Option<PathBuf> {
        self.opposite_pane_ref().map(|p| p.cwd.clone())
    }

    /// The pane opposite the focused one (None when the shell has focus).
    fn opposite_pane_ref(&self) -> Option<&Pane> {
        let other = match self.focused {
            FocusedPane::Left => &self.right,
            FocusedPane::Right => &self.left,
            FocusedPane::Shell => return None,
        };
        Some(other.active_ref())
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

    /// Is there a shell panel on screen at all?
    ///
    /// Always, now. The question survives because the single-pane views —
    /// details and the icon grid — were the filer and nothing else, and had no
    /// shell panel under them. Those were the windowed build's, and it is gone.
    /// The callers still have to ask, because the answer is the one thing that
    /// tells `focus_direction` whether Ctrl+j leads anywhere.
    pub(crate) fn has_shell_panel(&self) -> bool {
        true
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
        if next == FocusedPane::Shell && !self.has_shell_panel() {
            return;
        }
        if next != self.focused {
            self.focus(next);
        }
    }

    fn reload_active(&mut self) {
        if let Some(p) = self.active_pane_mut() {
            let _ = p.reload();
        }
    }

    fn open_in_other_pane(&mut self, new_tab: bool) -> Result<()> {
        // A directory opens the other pane on it; anything else (a file, or an
        // empty pane) opens the other pane on *this* directory, so the two
        // panes line up on the same folder.
        let target = match self.active_pane() {
            Some(p) => match p.selected() {
                Some(e) if e.is_dir => e.path.clone(),
                _ => p.cwd.clone(),
            },
            None => return Ok(()),
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

    /// `o` — make the ACTIVE pane show the same directory as the other pane
    /// (pull). E.g. on the right pane, the right pane jumps to the left's cwd.
    fn sync_active_from_other(&mut self) -> Result<()> {
        let other_cwd = match self.focused {
            FocusedPane::Left => self.right.active_ref().cwd.clone(),
            FocusedPane::Right => self.left.active_ref().cwd.clone(),
            FocusedPane::Shell => return Ok(()),
        };
        if let Some(p) = self.active_pane_mut() {
            if p.cwd == other_cwd {
                self.message = Some(tr(self.lang, "panes already in the same directory", "両ペインは既に同じディレクトリです").into());
                return Ok(());
            }
            p.jump_to(other_cwd.clone())?;
        }
        self.message = Some(format!("this pane → {}", other_cwd.display()));
        Ok(())
    }

    /// `O` — make the OTHER pane show the same directory as the active pane
    /// (push). E.g. on the right pane, the left pane jumps to the right's cwd.
    fn sync_other_from_active(&mut self) -> Result<()> {
        let cwd = match self.active_pane() {
            Some(p) => p.cwd.clone(),
            None => return Ok(()),
        };
        let other = match self.focused {
            FocusedPane::Left => &mut self.right,
            FocusedPane::Right => &mut self.left,
            FocusedPane::Shell => return Ok(()),
        };
        if other.active_ref().cwd == cwd {
            self.message = Some(tr(self.lang, "panes already in the same directory", "両ペインは既に同じディレクトリです").into());
            return Ok(());
        }
        other.active_mut().jump_to(cwd.clone())?;
        // Focus stays on the active pane.
        self.message = Some(format!("other pane → {}", cwd.display()));
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

    /// The path under the cursor in the active file pane, if any. Shared by the
    /// OS-native actions below.
    fn selected_os_path(&self) -> Option<PathBuf> {
        self.active_pane().and_then(|p| p.selected()).map(|e| e.path.clone())
    }

    /// Reveal the selected file in the OS file manager (#9).
    fn reveal_in_os(&mut self) {
        let Some(path) = self.selected_os_path() else {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        };
        match cian_core::os::reveal(&path) {
            Ok(()) => self.message = Some(format!("revealed: {}", path.display())),
            Err(e) => self.message = Some(format!("reveal failed: {e}")),
        }
    }

    /// Show the OS "Open with…" picker for the selected file (#9).
    fn open_with_os(&mut self) {
        let Some(path) = self.selected_os_path() else {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        };
        match cian_core::os::open_with(&path) {
            Ok(()) => self.message = Some(format!("open with…: {}", path.display())),
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    /// Open the OS properties / Get-Info panel for the selected file (#9).
    fn properties_os(&mut self) {
        let Some(path) = self.selected_os_path() else {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        };
        match cian_core::os::properties(&path) {
            Ok(()) => self.message = Some(format!("properties: {}", path.display())),
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    /// Text on the system clipboard, if any.
    /// Put the clipboard wherever cian is currently taking text, as if the
    /// terminal had delivered a paste.
    ///
    /// A terminal turns Ctrl+V into a paste event before cian sees the key, so
    /// the terminal build never needed this. The window build has no such
    /// middleman — and no clipboard code of its own — so Ctrl+V there went to
    /// the shell as a raw ^V and did nothing.
    pub(crate) fn paste_from_clipboard(&mut self) {
        match self.clipboard_text() {
            Some(t) => self.insert_into_active_text(&t),
            None => {
                self.message = Some(
                    tr(self.lang, "clipboard has no text", "クリップボードにテキストがありません")
                        .into(),
                )
            }
        }
    }

    fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard.as_mut()?.get_text().ok().filter(|t| !t.is_empty())
    }

    /// Ask where to write this shell pane's session log. The file name is
    /// generated on submit from the time and the pane's host.
    fn start_log_prompt(&mut self) {
        if self.shell.active_session().is_none() {
            self.message = Some(tr(self.lang, "no shell here to log", "記録するシェルがここにありません").into());
            return;
        }
        // Seed with a sensible directory: the focused file pane's, else home.
        let seed = self
            .last_file_pane_cwd()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        // Preview the generated file name so it is clear what lands in the dir.
        let host = self
            .shell
            .active_title()
            .and_then(|t| host_from_title(&t))
            .unwrap_or_else(|| "shell".to_string());
        let prompt = format!("directory for the log  (file: <time>_{}.log):", host);
        self.open_popup(text_input("session log — folder", &prompt, seed, InputKind::LogDir));
    }

    /// `:shellname` — say what this shell tab is for.
    ///
    /// A strip reading `shell 1 | shell 2 | shell 3` is a strip you have to
    /// open every tab to read, and the reason there is a second tab is always
    /// that the first is busy with something in particular. Empty puts the
    /// number back.
    fn start_shell_name_prompt(&mut self) {
        if self.shell.count() == 0 {
            self.message = Some(tr(self.lang, "no shell here to name", "名前を付けるシェルがありません").into());
            return;
        }
        let at = self.shell.active_tab_index();
        let seed = self.shell.tab_name(at).unwrap_or("").to_string();
        self.open_popup(text_input(
            "shell tab name",
            tr(self.lang, "what this tab is for  (empty puts the number back):",
               "このタブの用途  (空にすると番号に戻ります):"),
            seed,
            InputKind::ShellName,
        ));
    }

    /// Start logging the active shell pane into `dir`, building the file name
    /// from the timestamp and the pane's host (e.g. `20260723_140501_myhost.log`).
    fn start_session_log(&mut self, dir: &str) {
        let dir = expand_path(dir.trim());
        if !dir.is_dir() {
            self.message = Some(format!("not a directory: {}", dir.display()));
            return;
        }
        // Host from the pane's title (`user@host: cwd`), sanitized; a plain
        // "shell" when the shell set no title.
        let host = self
            .shell
            .active_title()
            .and_then(|t| host_from_title(&t))
            .unwrap_or_else(|| "shell".to_string());
        let name = format!("{}_{}.log", cian_core::timestamp_compact(), host);
        let path = dir.join(&name);
        match self.shell.active_session() {
            Some(s) => match s.start_log(&path) {
                Ok(()) => self.message = Some(format!("● logging to {}", path.display())),
                Err(e) => self.message = Some(format!("log failed: {}", e)),
            },
            None => self.message = Some(tr(self.lang, "no shell here to log", "記録するシェルがここにありません").into()),
        }
    }

    fn stop_session_log(&mut self) {
        match self.shell.active_session() {
            Some(s) if s.is_logging() => {
                let where_ = s.log_path().map(|p| p.display().to_string()).unwrap_or_default();
                s.stop_log();
                self.message = Some(format!("log saved: {}", where_));
            }
            _ => self.message = Some(tr(self.lang, "this pane is not logging", "このペインは記録していません").into()),
        }
    }

    /// Any shell pane currently recording, across all tabs? Drives the pulsing
    /// border and the keep-repainting-while-logging tick.
    fn any_logging(&self) -> bool {
        self.shell.tabs.iter().any(|t| {
            t.nodes.iter().any(|n| {
                matches!(n, Some(Node::Leaf { session, .. }) if session.is_logging())
            })
        })
    }

    /// The file pane a file-oriented action should use: the focused one, or —
    /// when the shell has focus — the last file pane that did.
    fn effective_file_pane(&self) -> &Pane {
        let tabs = match self.focused {
            FocusedPane::Left => &self.left,
            FocusedPane::Right => &self.right,
            FocusedPane::Shell => match self.last_file_pane {
                FocusedPane::Right => &self.right,
                _ => &self.left,
            },
        };
        tabs.active_ref()
    }

    /// The last-focused file pane's directory, for seeding prompts.
    fn last_file_pane_cwd(&self) -> Option<PathBuf> {
        let tabs = match self.last_file_pane {
            FocusedPane::Right => &self.right,
            _ => &self.left,
        };
        Some(tabs.active_ref().cwd.clone())
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
pub(crate) fn encode_key(key: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
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

/// Hand something to whatever the desktop opens it with. The three-way
/// platform split lives in `cian-core`, so the GUI's engine shares it.
fn os_open(target: impl AsRef<std::ffi::OsStr>) -> Result<()> {
    cian_core::proc::open_with_desktop(target)?;
    Ok(())
}

/// The file cian writes runtime preferences to (theme, and room for more
/// later). It sits with the other config in the portable dir or `~/.config/cian`
/// and is managed by cian, not hand-edited — `:theme` keeps it in sync.
/// Remembered UI state lives in cian-lua, beside the config paths it uses.
/// The GUI's engine remembers the same things in the same file — a look chosen
/// in one and not the other would be two programs.
pub(crate) use cian_lua::{state_get, state_set};
// The two pure ones are used only by the tests that pin their behaviour down;
// naming them here as well would be an unused import in every other build.
#[cfg(test)]
pub(crate) use cian_lua::{state_get_in, state_with};

/// The theme name saved from a previous session's `:theme`, if any. Applied at
/// startup on top of init.lua so a chosen theme survives a restart.
fn load_saved_theme() -> Option<String> {
    state_get("theme")
}

/// Persist the chosen whole-app theme so the next launch keeps it. Best-effort:
/// a read-only config dir just means it does not stick, which is not worth
/// interrupting the user over.
pub(crate) fn save_theme_pref(name: &str) {
    state_set("theme", name);
}

// `os_reveal` / `os_open_with` / `os_properties` moved to `cian_core::os`.
// The windowed engine had written its own `revealos` — one verb with two
// implementations, and the copies had drifted: the engine's built Explorer's
// argument with `Command::arg`, which mis-quotes a path holding a space, so
// reveal silently showed Documents from a OneDrive Desktop. Both call the one
// implementation now.

/// The user's home directory: `$HOME`, or `$USERPROFILE` on Windows.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Expand `~`, `$VAR`/`${VAR}` and `%VAR%` in a typed path.
///
/// A path is usually typed after copying it from somewhere, and the somewhere
/// is often a shell or an Explorer address bar, where these forms are normal.
/// Parse a `-n N` argument (or the bare `-N` shorthand `head`/`tail` accept).
fn parse_dash_n(args: &[&str]) -> Option<usize> {
    let mut it = args.iter().copied();
    while let Some(a) = it.next() {
        if a == "-n" {
            return it.next().and_then(|v| v.parse().ok());
        }
        // `-20` shorthand.
        if let Some(num) = a.strip_prefix('-') {
            if let Ok(n) = num.parse::<usize>() {
                return Some(n);
            }
        }
    }
    None
}

/// Pull a host name out of a terminal title like `user@host: ~/dir`, for a
/// log file name. Returns a filesystem-safe token, or None if there's no `@`.
fn host_from_title(title: &str) -> Option<String> {
    let after_at = title.split('@').nth(1)?;
    // The host runs up to the first `:`, space, or slash.
    let host: String = after_at
        .chars()
        .take_while(|c| !matches!(c, ':' | ' ' | '/' | '\t'))
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// Reverse the cells covered by a shell selection, so the drag is visible. The
/// selection is linear (like a terminal's): from the anchor to the end in
/// reading order, whole rows in between.
fn highlight_shell_selection(f: &mut Frame, sel: &ShellSel) {
    let inner = sel.inner;
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let (a, b) = (sel.anchor, sel.end);
    let (start, end) = if (a.0, a.1) <= (b.0, b.1) { (a, b) } else { (b, a) };
    let buf = f.buffer_mut();
    for gr in start.0..=end.0 {
        let first = if gr == start.0 { start.1 } else { 0 };
        let last = if gr == end.0 { end.1 } else { inner.width.saturating_sub(1) };
        for gc in first..=last {
            let x = inner.x + gc;
            let y = inner.y + gr;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

/// Map an on-screen `(col, row)` to a `(grid_row, grid_col)` inside `inner`,
/// clamped to the area — for translating a mouse position to a terminal cell.
fn grid_pos(inner: Rect, col: u16, row: u16) -> (u16, u16) {
    let gr = row.saturating_sub(inner.y).min(inner.height.saturating_sub(1));
    let gc = col.saturating_sub(inner.x).min(inner.width.saturating_sub(1));
    (gr, gc)
}

/// Quote a path for a POSIX shell (single quotes, with the usual `'\''`
/// escape). On Windows the shell is usually PowerShell or cmd, whose quoting
/// differs, but a path with no odd characters passes through either way and
/// this at least keeps spaces together.
///
/// Deliberately not `cian_scp::shell_quote`, which quotes unconditionally. The
/// result here is spliced into a command line the user reads and edits (`%f`,
/// `%d`), so an ordinary path is left bare rather than dressed in quotes it
/// does not need.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    if s.chars().all(|c| c.is_alphanumeric() || "._-/:\\".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build a text-input popup with the caret at the end of the seeded text —
/// where you want it for editing an existing name or path.
fn text_input(
    title: impl Into<String>,
    prompt: impl Into<String>,
    buffer: String,
    kind: InputKind,
) -> Popup {
    let cursor = buffer.chars().count();
    Popup::TextInput {
        title: title.into(),
        prompt: prompt.into(),
        buffer,
        kind,
        cursor,
        select_all: false,
    }
}

/// Byte offset of the `n`-th char, or the string's length past the end. Used to
/// edit a `String` at a caret expressed as a char index (so CJK is handled).
fn char_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map(|(b, _)| b).unwrap_or(s.len())
}

fn insert_str_at(buffer: &mut String, cursor: &mut usize, s: &str) {
    let b = char_byte(buffer, *cursor);
    buffer.insert_str(b, s);
    *cursor += s.chars().count();
}

fn insert_char_at(buffer: &mut String, cursor: &mut usize, c: char) {
    let b = char_byte(buffer, *cursor);
    buffer.insert(b, c);
    *cursor += 1;
}

/// Delete the char before the caret (Backspace).
fn backspace_at(buffer: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let start = char_byte(buffer, *cursor - 1);
    let end = char_byte(buffer, *cursor);
    buffer.replace_range(start..end, "");
    *cursor -= 1;
}

/// Delete the char at the caret (Delete).
fn delete_at(buffer: &mut String, cursor: &mut usize) {
    let n = buffer.chars().count();
    if *cursor >= n {
        return;
    }
    let start = char_byte(buffer, *cursor);
    let end = char_byte(buffer, *cursor + 1);
    buffer.replace_range(start..end, "");
}

/// Render a single-line field with a visible caret at `cursor`, masking the
/// text with dots when it is a secret.
/// Parse a chmod field: blank → no change `(None, None)`; a valid octal mode →
/// `(Some(mode), None)`; anything else → `(None, Some(error))`.
fn parse_chmod(s: &str) -> (Option<u32>, Option<String>) {
    let t = s.trim();
    if t.is_empty() {
        return (None, None);
    }
    match u32::from_str_radix(t, 8) {
        Ok(m) if m <= 0o7777 => (Some(m), None),
        _ => (None, Some(format!("invalid chmod {:?} — use an octal mode like 777", t))),
    }
}

fn expand_path(input: &str) -> PathBuf {
    let mut out = String::with_capacity(input.len());
    let b: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            // %VAR% (Windows)
            '%' => {
                if let Some(end) = b[i + 1..].iter().position(|c| *c == '%') {
                    let name: String = b[i + 1..i + 1 + end].iter().collect();
                    if let Some(v) = std::env::var_os(&name) {
                        out.push_str(&v.to_string_lossy());
                        i += end + 2;
                        continue;
                    }
                }
                out.push('%');
                i += 1;
            }
            // $VAR and ${VAR} (Unix)
            '$' => {
                let (name, adv) = if b.get(i + 1) == Some(&'{') {
                    match b[i + 2..].iter().position(|c| *c == '}') {
                        Some(end) => (b[i + 2..i + 2 + end].iter().collect::<String>(), end + 3),
                        None => (String::new(), 1),
                    }
                } else {
                    let end = b[i + 1..]
                        .iter()
                        .position(|c| !c.is_alphanumeric() && *c != '_')
                        .unwrap_or(b.len() - i - 1);
                    (b[i + 1..i + 1 + end].iter().collect::<String>(), end + 1)
                };
                match (name.is_empty(), std::env::var_os(&name)) {
                    (false, Some(v)) => {
                        out.push_str(&v.to_string_lossy());
                        i += adv;
                    }
                    _ => {
                        out.push('$');
                        i += 1;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    // Quotes survive copy-paste from shells and Explorer; strip a matched pair.
    let t = out.trim();
    let unquoted = |q: char| t.strip_prefix(q).and_then(|x| x.strip_suffix(q));
    let t = unquoted('"').or_else(|| unquoted('\'')).unwrap_or(t);
    expand_tilde(Path::new(t))
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
/// Files on the OS clipboard, and putting them there. Both live in cian-core
/// now: three platforms, three unrelated mechanisms, and two front ends.
use cian_core::fileclip::{files as os_clipboard_files, put_files as os_clipboard_file_refs};
// The filter that keeps only paths that exist — the tests pin its behaviour
// down, because a clipboard query happily hands back plain text as a path.
#[cfg(test)]
use cian_core::fileclip::keep_existing;

fn shortcut_icon(target: &str) -> &'static str {
    if !theme::nerd_fonts() {
        return "";
    }
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
        // Only the name matters here: this is used to pick an icon by extension.
        let entry = cian_core::Entry {
            name_lower: name.to_lowercase(),
            name,
            path: path.clone(),
            is_dir: false,
            len: 0,
            modified: None,
            cloud: false,
            is_parent: false,
        };
        return icon_for(&entry);
    }
    "\u{f15b}" // default file
}

/// One row of the manual: the built-in key(s), the remappable action they run
/// (if any), and what it does, in English and Japanese.
struct ManualEntry {
    keys: &'static str,
    action: Option<Action>,
    en: &'static str,
    ja: &'static str,
}

impl ManualEntry {
    fn desc(&self, lang: Lang) -> &'static str {
        match lang {
            Lang::En => self.en,
            Lang::Ja => self.ja,
        }
    }
}

const fn entry(
    keys: &'static str,
    action: Option<Action>,
    en: &'static str,
    ja: &'static str,
) -> ManualEntry {
    ManualEntry { keys, action, en, ja }
}

/// The manual's contents, grouped into sections. Entries carrying an [`Action`]
/// are remappable, so [`manual_lines`] can append whatever extra keys the user
/// bound to them in `init.lua`.
fn manual_sections() -> Vec<((&'static str, &'static str), Vec<ManualEntry>)> {
    use Action::*;
    vec![
        (
            ("General", "基本"),
            vec![
                entry("q", Some(Quit), "quit (confirms)", "終了（確認あり）"),
                entry(":", Some(Command), "command mode (:q, :shell, :man)", "コマンドモード（:q, :shell, :man）"),
                entry("?, Ctrl+.", None, "show this manual (also right-click)", "このマニュアルを表示（右クリックでも）"),
                entry("Esc", None, "clear marks and filter / leave shell", "マーク・フィルタ解除／シェルを抜ける"),
            ],
        ),
        (
            ("Navigation", "移動"),
            vec![
                entry("j, Down", Some(CursorDown), "cursor down", "カーソルを下へ"),
                entry("k, Up", Some(CursorUp), "cursor up", "カーソルを上へ"),
                entry("Shift+D", Some(PageDown), "move 10 lines down", "10行下へ"),
                entry("Shift+U", Some(PageUp), "move 10 lines up", "10行上へ"),
                entry("gg", None, "jump to top", "先頭へジャンプ"),
                entry("G", Some(CursorBottom), "jump to bottom", "末尾へジャンプ"),
                entry("Enter", Some(EnterDir), "enter folder / read the file / go into an archive (Ctrl+Enter launches it)", "ディレクトリに入る／ファイルを読む／アーカイブの中へ（Ctrl+Enter でアプリ起動）"),
                entry("Backspace", Some(Parent), "up one level", "1階層上へ"),
                entry("drag in from Finder", None, "drop files on the window: they MOVE into this pane (asks first)", "Finder等からドロップ：このペインへ移動（先に確認）"),
                entry("Shift+P", Some(CopyFileRef), "put the selection on the clipboard for Finder/Explorer to paste", "選択をクリップボードへ（Finder/エクスプローラで貼り付け）"),
                entry("Alt+← / Alt+h", None, "back to the previous directory (or click ◀ in the title)", "直前のディレクトリへ戻る（タイトルの ◀ クリックでも）"),
                entry("Alt+→ / Alt+l", None, "forward again (or click ▶)", "進む（▶ クリックでも）"),
                entry("h", None, "this pane's directory history (also :back)", "このペインの移動履歴（:back でも）"),
                entry("F3", None, "look inside: text/hex, an image, or an archive's list", "中身を見る：テキスト/16進・画像・アーカイブの一覧"),
                entry("  Enter on a zip", None, "browse INSIDE the archive; copy out=extract, copy in=add, r/d rename/delete (zip)", "zipにEnterでアーカイブの中へ（コピー=展開／逆コピー=追加、r/d でリネーム・削除）"),
                entry(":preview", None, "cursor-follow preview in the shell panel (Shift+J shows the shell)", "シェル枠にカーソル追従プレビュー（Shift+J でシェル表示）"),
                entry(":queue", None, "operations queue: b backgrounds the bar, x stops/removes, x again abandons", "操作キュー：b でバー格納、x で停止/削除（再度x=見捨て）"),
                entry("  ☁ column", None, "cloud-only (not downloaded) files; sweeps skip them — T to include", "☁ 列：未ダウンロードのクラウドファイル。一括処理は飛ばす（T で読ませる）"),
                entry(":nobom", None, "strip UTF-8 BOMs from marked files (UTF-16 kept; viewer badges BOMs)", "マークから UTF-8 BOM 除去（UTF-16 は保持・ビューアにバッジ表示）"),
                entry("  i on a binary", None, "hex edit: 0-9a-f overwrites bytes, Ctrl+S saves with a .bak", "バイナリで i：hex編集（0-9a-f 上書き、Ctrl+S で .bak を残して保存）"),
                entry("  edit in viewer", None, "i/a/o/O/I = insert (Ctrl+S save, Esc leave), :edit = external editor", "ビューア内編集：i/a/o/O/I 挿入（Ctrl+S 保存, Esc 終了）／ E 外部エディタ"),
                entry("  : in viewer", None, "replace: s/old/new/ (g all, c confirm each, i ignore case)", "ビューアで : ：置換 s/old/new/（g 行内全部, c 1件ずつ, i 大小無視）"),
                entry("    :sort :rsort :uniq", None, "sort / reverse-sort / drop duplicate lines (selection or whole file)", "行のソート／逆順／重複削除（選択範囲またはファイル全体）"),
                entry("    :han :zen", None, "full-width ASCII → half, half-width kana → full / and back", "全角英数→半角・半角カナ→全角／その逆"),
                entry("    :expand :unexpand :reindent", None, "leading tabs ↔ spaces, and re-indent to a consistent step", "先頭のTAB⇔空白、インデントを一定幅に整形"),
                entry("    :ws", None, "show trailing spaces, tabs and ideographic spaces", "行末空白・TAB・全角スペースを表示"),
                entry("    :lf :crlf", None, "convert line endings (shown in the title)", "改行コードを変換（タイトルに表示）"),
                entry("  Shift+F8/F9/F10", None, "split left-right / top-bottom / close it — Shift+H,L or a click crosses over", "左右分割 / 上下分割 / 解除 — Shift+H,L かクリックで行き来"),
                entry("  = in a split", None, "compare the two halves in place — ]c / [c step, both stay editable", "分割中の = で両側を比較 — ]c / [c で移動、どちらも編集可能なまま"),
                entry("  ? in viewer", None, "the keys this window has, rather than all of cian's", "ビューアで ?：この画面で使えるキーだけを表示"),
                entry("  right-click / S-Enter", None, "the viewer's menu: ask the AI about the selection, copy, reveal, theme", "ビューアのメニュー：選択範囲をAIに聞く・コピー・場所を開く・テーマ"),
                entry("  p / P", None, "paste after / before the cursor — whole lines when whole lines were copied", "カーソルの後/前に貼り付け — 行単位でコピーしたものは行単位で"),
                entry("  :preview", None, "the rendered Markdown, and back to the source (Ctrl+E where the terminal allows it)", "Markdown の描画表示とソースの切替（端末が許せば Ctrl+E でも）"),
                entry("  F2 / Shift+F2", None, "the next / previous open file, or click ◂ ▸ in the title — F3 on marked files opens them all", "次/前の開いているファイル（タイトルの ◂ ▸ クリックでも）— マークして F3 で全部開く"),
                entry("  :r after /", None, "replace what the search found — the prompt arrives with the pattern in it. `r` on its own stamps one character over the cursor, as in vi", "検索したものを置換 — パターンは入力済みで開く。`r` 単体は vi と同じくカーソル位置に1文字上書き"),
                entry("  :ruler", None, "the column scale, with the cursor's column marked and its line tinted. On by default", "列のルーラー（カーソルの桁を強調、カーソル行に色）。既定でオン"),
                entry("  :ws", None, "the invisible characters — tab, trailing space, ideographic space, line ending. On by default", "見えない文字の表示 — TAB・行末の空白・全角空白・改行。既定でオン"),
                entry("  :expand all", None, "convert every tab, not only the indent (destroys TSV separators — hence by name)", "行中のタブも全部変換（TSV の区切りも消えるので、明示的に指定）"),
                entry("  outline", None, "]] / [[ next/prev section, click an entry to jump, :outline hides the column", "]] / [[ 次/前の見出し、項目クリックで移動、:outline で列を隠す"),
                entry("  folding", None, "Space or za fold/unfold here, zA all (either way), or click the ▾ in the gutter", "Space か za で折りたたみ切替、zA で全部（開いていれば閉じ、閉じていれば開く）、余白の ▾ クリックでも可"),
                entry("  :w :wq :q :q!", None, "save / save and close / close / close discarding — when Ctrl+S is taken by the terminal", "保存 / 保存して閉じる / 閉じる / 破棄して閉じる — Ctrl+S が端末に取られている場合に"),
                entry("  V then I / A", None, "line selection: put text at the start, or at the end, of every line", "行選択：全行の先頭、または全行の末尾に文字を入れます"),
                entry("  Ctrl+Q / Alt+v / :block", None, "rectangle: d cuts it, I/A insert at the left/right edge, c replaces", "矩形選択：d で切り取り、I/A で左端/右端に挿入、c で置換"),
                entry("  normal mode", None, "x/dd/D/J delete·join, u undo, v+d cut selection (d/u scroll via Ctrl)", "ノーマルモード：x/dd/D/J 削除·結合, u 取消, v+d 選択削除（スクロールは Ctrl+d/u）"),
                entry(":edit", None, "edit the file in your external editor (E in the viewer)", "外部エディタで編集（ビューア内は E）"),
                entry(":vi / :vim / :nvim", None, "open the file in that editor in a new shell tab", "新規シェルタブでそのエディタでファイルを開く"),
                entry(":renamelist", None, "rename marked (or all) files by editing the list in your editor", "マーク（無ければ全部）の名前一覧をエディタで編集してリネーム"),
                entry("  in viewer", None, "hjkl move, /n/N search, %/{/}/NG jump, v/V/C-v select y copy", "ビューア内：hjkl移動, /n/N検索, %/{/}/NG移動, v/V/C-v選択 yコピー"),
                entry("  :blame", None, "toggle the git blame gutter (who last changed each line)", "ビューア内：git blame ガター切替（各行の最終変更者）"),
                entry("  from a grep hit", None, "Ctrl+n/N next/prev hit, :enc encoding (reveal is in the menu)", "grepヒットから：Ctrl+n/N 次/前, e 文字コード（場所を開くはメニュー内）"),
                entry("=", None, "compare left ↔ right: two files (line diff), or two folders (recursive)", "左右を比較：ファイル同士（行差分）／ディレクトリ同士（再帰）"),
                entry("  > / <", None, "  in a comparison: copy the entry across to the other side (confirms overwrite)", "  比較画面：エントリを反対側へコピー（上書きは確認）"),
                entry("  c / w", None, "  in a comparison: copy to clipboard / save side-by-side (.html or .md, else .txt)", "  比較画面：クリップボードへ／左右並びで保存（.html か .md、他は .txt）"),
                entry("Bksp", Some(Parent), "parent folder (bind `-` to it in init.lua if you want that too)", "親ディレクトリへ（`-` も使いたければ init.lua で割当）"),
                entry("Left / Right", None, "focus the left / right pane", "左／右のペインにフォーカス"),
                entry("h", Some(History), "history popup", "履歴ポップアップ"),
                entry("z", None, "go to a typed path (also :cd)", "入力したパスへ移動（:cd でも）"),
                entry("Ctrl+Shift+P, Ctrl+,, C", None, "command palette: fuzzy-find any command", "コマンドパレット：全コマンドをあいまい検索"),
                entry("Z", None, "fuzzy-jump to a recent / bookmarked directory (also :jump)", "最近/ブックマークのディレクトリへあいまいジャンプ（:jump でも）"),
                entry("T", None, "UI toggles menu: dotfiles, input sync, notifications… (also :toggle)", "UIトグルメニュー：隠しファイル/入力同期/通知…（:toggle でも）"),
                entry(":each", None, "run a shell command per marked file — {} = path (:each grep -l foo {})", "マーク各ファイルにコマンド実行 — {} = パス（:each grep -l foo {}）"),
                entry("F5", None, "refresh now (:refresh)", "今すぐ再読み込み（:refresh）"),
                entry("f", Some(Search), "search in this folder", "このディレクトリ内を検索"),
                entry("Shift+F", None, "find by name, whole tree below here", "名前で検索（ここ以下のツリー全体）"),
                entry("Ctrl+F / Ctrl+G", Some(Action::GrepRecursive), "grep inside files, whole tree below here (:grep too)", "ファイル内をgrep（ここ以下のツリー全体）— Ctrl+G（サクラと同じ）や :grep でも可"),
                entry("  patterns", None, "  bare text = literal; /re/ = regex, /re/i ignores case; grep also reads SJIS", "  裸の文字列=そのまま、/re/=正規表現（/re/i で大小無視）、grep は SJIS も読む"),
                entry("  p in results", None, "panelize: load the find/grep matches into the pane to mark & operate on", "検索結果を p でペイン化：マークして一括操作できます"),
                entry("  r in results", None, "replace across every file the grep matched: preview each line, Space unchecks", "grep 結果の全ファイルを一括置換：1行ずつ確認、Space で除外"),
                entry("b", None, "branch view: flatten this subtree into the pane, one row per file (b/Esc to leave)", "ブランチビュー：この配下を1ファイル1行に平坦化（b/Esc で戻る）"),
                entry("n", Some(SearchNext), "next match", "次のマッチ"),
                entry("N", Some(SearchPrev), "previous match", "前のマッチ"),
                entry("/", None, "filter list as you type", "入力に応じて一覧を絞り込み"),
                entry("// , Ctrl+P", None, "fuzzy-find a file anywhere below here", "この下のどこかにあるファイルをあいまい検索"),
                entry(",", None, "sort by name / size / date / ext", "ソート：名前／サイズ／日時／拡張子"),
                entry("Shift+S", None, "ssh picker (also :ssh, or right-click)", "SSHピッカー（:ssh・右クリックでも）"),
                entry(":remote", None, "open a server IN this pane (a remote pane; carmine frame). Enter/l navigate, Esc leaves", "サーバをこのペインで開く（リモートペイン・カーマイン枠）。Enter/l で移動、Esc で戻る"),
                entry("Enter, Esc", None, "while filtering: keep / clear it", "フィルタ中：適用したまま／解除"),
            ],
        ),
        (
            ("Marks and file operations", "マークとファイル操作"),
            vec![
                entry("Space", Some(MarkDown), "toggle mark, move down", "マーク切替して下へ"),
                entry("Shift+Space", Some(MarkUp), "toggle mark, move up", "マーク切替して上へ"),
                entry("v", Some(Visual), "visual select", "ビジュアル選択"),
                entry("  a", None, "  in visual: select all (or gg v G)", "  ビジュアル中：全選択（gg v G でも）"),
                entry("  gg / G", None, "  in visual: extend to top / bottom", "  ビジュアル中：先頭／末尾まで伸ばす"),
                entry("V", Some(InvertMarks), "invert all marks", "全マークを反転"),
                entry("u  Ctrl+Z", None, "undo the last rename / create / copy / move / folder step — an undone copy goes to the trash (also :undo)", "直前のリネーム／作成／コピー／移動／ディレクトリ移動を取り消し ── コピーはゴミ箱へ（:undo でも）"),
                entry("Ctrl+R  Ctrl+Shift+Z", None, "redo what u just undid (Ctrl+Y, :redo)", "u で取り消した操作をやり直し（Ctrl+Y・:redo でも）"),
                entry("c", Some(Copy), "copy to opposite pane", "反対ペインへコピー"),
                entry("m", Some(Move), "move to opposite pane", "反対ペインへ移動"),
                entry("Ctrl+C", None, "copy to the file clipboard (Windows-style)", "ファイルクリップボードへコピー（Windows流）"),
                entry("Ctrl+X", Some(Cut), "cut to the file clipboard", "ファイルクリップボードへ切り取り"),
                entry("Ctrl+V, y", Some(Paste), "paste the file clipboard here", "ファイルクリップボードをここに貼り付け"),
                entry("d", Some(Delete), "delete (to trash)", "削除（ゴミ箱へ）"),
                entry("r", Some(Rename), "rename", "リネーム"),
                entry(":renamepattern", None, "bulk rename by pattern: {name}_{n3}.{ext} or s/re/rep/gi (preview first)", "パターン一括リネーム：{name}_{n3}.{ext} / s/re/rep/gi（先にプレビュー）"),
                entry("a", Some(NewFile), "new file", "新規ファイル"),
                entry("A", Some(NewDir), "new directory", "新規ディレクトリ"),
                entry("o", Some(SyncFromOther), "this pane → other pane's directory", "このペインを反対ペインと同じ場所に"),
                entry("O", Some(SyncToOther), "other pane → this pane's directory", "反対ペインをこのペインと同じ場所に"),
                entry("Ctrl+Enter", Some(OpenOther), "a folder → the opposite pane; a file → your own app", "ディレクトリは反対ペインで開く／ファイルは既定のアプリで開く"),
                entry("p", Some(CopyPath), "copy path text to clipboard", "パス文字列をクリップボードにコピー"),
                entry("Shift+P", Some(CopyFileRef), "copy file(s) to clipboard", "ファイルをクリップボードにコピー"),
                entry("s", Some(Shortcuts), "shortcuts menu", "ショートカットメニュー"),
                entry("@", None, "run a macro (layout builder; also :macro / right-click)", "マクロを実行（レイアウト構築；:macro／右クリックでも）"),
                entry(":count", None, "count files & steps (marked, or the whole tree)", "ファイル・ステップ数を数える（マーク or ツリー全体）"),
                entry(":du", None, "disk usage: what's biggest here (Enter into a folder, - up)", "容量分析: 何が大きいか（Enter でディレクトリへ、- で上へ）"),
                entry(":hidden", None, "show / hide dotfiles (also right-click)", "ドットファイルの表示切替（右クリックでも）"),
                entry(":attr", None, "attributes;  :chmod 644,  :readonly on|off", "属性；  :chmod 644,  :readonly on|off"),
                entry(":hash", None, "checksum;  :hash md5  /  :hash sha256", "チェックサム；  :hash md5  /  :hash sha256"),
                entry(":stage / :unstage", None, "git add / git reset the selection (in a repo)", "選択を git add / git reset（リポジトリ内）"),
                entry(":discard", None, "git/svn: throw away worktree changes (git checkout / svn revert)", "作業ツリーの変更を破棄（git checkout / svn revert）"),
                entry(":log", None, "commit log / a file's history — git or svn (also right-click)", "コミットログ／ファイル履歴 — git・svn（右クリックでも）"),
                entry(":gitdiff", None, "the selected file's diff vs HEAD/BASE — git or svn", "選択ファイルの HEAD／BASE との差分 — git・svn"),
                entry(":svnupdate", None, "svn update the working copy (also right-click SVN ▸)", "svn update で作業コピーを更新（右クリック SVN ▸ でも）"),
                entry(":svncommit", None, "svn commit the selection (prompts for a message)", "選択を svn commit（メッセージ入力）"),
                entry(":svnresolve", None, "svn resolve --accept working (mark conflicts resolved)", "svn resolve --accept working（競合を解決）"),
                entry("right-click", None, "upload/download to a configured host (SFTP or SCP)", "設定したホストへアップ／ダウンロード（SFTP/SCP）"),
                entry("M / Shift+Enter", Some(Menu), "context menu for the entry (also :menu)", "エントリのコンテキストメニュー（:menu でも）"),
            ],
        ),
        (
            ("Panes and tabs", "ペインとタブ"),
            vec![
                entry("Shift+H/J/K/L", None, "move focus between panes", "ペイン間でフォーカス移動"),
                entry("Ctrl+Shift+←→↑↓", None, "resize panes (border follows the arrow)", "ペインのリサイズ（境界が矢印方向へ）"),
                entry("drag a border", None, "resize any split (mouse)", "境界をドラッグで分割をリサイズ（マウス）"),
                entry("double-click", None, "enter a folder, or open a file (OS default)", "ディレクトリに入る／ファイルを開く（OS標準）"),
                entry("drag an entry", None, "to the other pane: copy (Shift: move)", "反対ペインへ：コピー（Shift で移動）"),
                entry("  ", None, "  onto the shell: type its path there", "  シェルへ：パスをそこに入力"),
                entry(":copyto", None, "copy to a recent or typed directory", "最近使った／入力したディレクトリへコピー"),
                entry(":moveto", None, "move there instead", "同じ選び方で移動"),
                entry("right-click", None, "context menu (copy/cut/paste, color)", "コンテキストメニュー（コピー/カット/貼り付け、色）"),
                entry("Ctrl+H/J/K/L", None, "same (needs kitty keyboard support)", "同上（kittyキーボード対応が必要）"),
                entry("t, F9", None, "new tab", "新規タブ"),
                entry("w", None, "close tab", "タブを閉じる"),
                entry("F1 / F2", None, "previous / next tab", "前／次のタブ"),
                entry("Tab", None, "cross to the other pane — a listing or a file, whichever is there", "反対のペインへ — 一覧でもファイルでも"),
                entry("Shift+Tab", None, "the next tab of whatever has the focus (F1 / F2 step either way)", "フォーカス中のパネルの次のタブへ（F1 / F2 は前後）"),
                entry("click a tab", None, "switch to it (mouse)", "クリックで切替（マウス）"),
                entry("F10", None, "close tab (confirms)", "タブを閉じる（確認あり）"),
            ],
        ),
        (
            ("Commands (type : then the name — Linux-style)", "コマンド（: に続けて名前を入力 — Linux風）"),
            vec![
                entry(":mkdir", None, "make a directory;  :mkdir -p a/b/c", "ディレクトリ作成；  :mkdir -p a/b/c"),
                entry(":touch", None, "create a file, or bump its mtime", "ファイル作成／mtimeを更新"),
                entry(":cp / :mv", None, "no arg → other pane;  or  :mv <dest>", "引数なし→反対ペイン；  または  :mv <宛先>"),
                entry(":rm", None, "delete the selection (to trash)", "選択物を削除（ゴミ箱へ）"),
                entry(":cd", None, ":cd <path>  /  :cd ..  /  :cd -  /  :cd ~", ":cd <パス>  /  :cd ..  /  :cd -  /  :cd ~"),
                entry(":pwd", None, "show the directory, copy it to the clipboard", "ディレクトリを表示しクリップボードにコピー"),
                entry(":ls", None, "refresh;  :ls -a  toggles dotfiles", "再読み込み；  :ls -a でドットファイル切替"),
                entry(":stat", None, "attributes (same as :attr)", "属性（:attr と同じ）"),
                                entry(":wc", None, "line / word / byte counts", "行／単語／バイト数"),
                entry(":head / :tail", None, "first / last lines;  :tail -n 40", "先頭／末尾の行；  :tail -n 40"),
                entry(":df", None, "free disk space;  :df -h -k -m -g", "ディスク空き容量；  :df -h -k -m -g"),
                entry(":theme", None, "theme gallery;  :theme dracula  sets one directly", "テーマ一覧；  :theme dracula で直接指定"),
                entry(":reload", None, "re-read init.lua (borders need a restart)", "init.luaを再読込（枠線は再起動が必要）"),
                entry(":redraw", None, "repaint the screen from nothing, after a stray control character scrambles it", "画面を一から描き直す（制御文字で表示が乱れたとき）"),
                entry("  editing a member", None, "F3 inside a zip, edit, Ctrl+S or :w — it goes back into the zip", "zip 内で F3 → 編集 → Ctrl+S か :w で zip に書き戻す"),
                entry(":office", None, "open a synced Office file's cloud copy in the desktop app (also right-click ▸ OS)", "同期された Office 文書のクラウド側をアプリで開く（右クリック ▸ OS からも）"),
                entry(":officelink", None, "write a .url shortcut to the cloud copy — the thing to paste into a mail", "クラウド側への .url ショートカットを作成（メールに貼るのはこれ）"),
                entry("Ctrl+A, :markall", None, "mark everything here — in the viewer, select the whole file", "ここにある全部をマーク — ビューアではファイル全体を選択"),
                entry(":key", None, "report every keystroke as cian receives it, and which keyboard mode is in use", "受け取ったキーをそのまま表示（キーボードのモードも表示）"),
                entry("  set_keymap", None, "init.lua: cian.set_keymap(\"alt+g\", \"grep_recursive\") — modifiers allowed", "init.lua: cian.set_keymap(\"alt+g\", \"grep_recursive\") — 修飾キーも書ける"),
                entry("  CIAN_LEGACY_KEYS=1", None, "start without the enhanced-keyboard request — try it if every Ctrl shortcut is dead", "拡張キーボード要求なしで起動 — Ctrl 系が全滅するときに試す"),
                entry(":where", None, "which config files cian reads/writes (portable vs ~/.config)", "cianが読み書きする設定ファイルの場所（ポータブル/~/.config）"),
                entry(":mark", None, "mark by wildcard;  :mark *.rs   :unmark *", "ワイルドカードでマーク；  :mark *.rs   :unmark *"),
                entry(":ai", None, "AI - simple: chat with the local model  — needs cian.ai in init.lua", "AI - simple: ローカルモデルとチャット  — init.luaのcian.aiが必要"),
                entry(":aicmd", None, "AI: shell command from a description", "AI: 説明からシェルコマンド生成"),
                entry(":aidiff", None, "AI: explain the diff on screen (x in the diff view)", "AI: 表示中の差分を説明（差分画面で x）"),
                entry(":ailog", None, "AI: triage the selected log file (errors, cause, next check)", "AI: 選択中のログを診断（エラー・原因・次の確認）"),
                entry(":zip", None, "bundle selection;  :zip -e  for a password", "選択物をまとめる；  :zip -e でパスワード付き"),
                entry(":tar / :targz", None, "make a .tar / .tar.gz (also right-click ▸ Compress)", ".tar / .tar.gz を作成（右クリック▸圧縮でも）"),
                entry(":unzip", None, "extract the archive here (also right-click ▸ Extract)", "アーカイブをここに解凍（右クリック▸解凍でも）"),
                entry(":!cmd", None, "run in shell;  % = selection, %f file, %d dir", "シェルで実行；  % =選択, %f ファイル, %d ディレクトリ"),
            ],
        ),
        (
            ("Shell panel (focus: click, Shift+J, or :shell)", "シェルパネル（フォーカス：クリック・Shift+J・:shell）"),
            vec![
                entry("F1-F8", None, "switch to shell tab 1-8", "シェルタブ 1-8 に切替"),
                entry("F9", None, "new shell tab", "新規シェルタブ"),
                entry("F10", None, "close shell tab", "シェルタブを閉じる"),
                entry("Shift+F1/F2", None, "focus next / previous split pane", "次／前の分割ペインにフォーカス"),
                entry("Shift+F8", None, "v-split (panes side by side)", "左右分割（ペインを横に並べる）"),
                entry("Shift+F9", None, "h-split (panes stacked)", "上下分割（ペインを縦に積む）"),
                entry("Shift+F10", None, "close split pane (confirms)", "分割ペインを閉じる（確認あり）"),
                entry("wheel / Shift+PgUp / Shift+↑", None, "scroll back through output that has gone past (Shift+Home / End for the two ends; typing returns to live)", "流れた出力をさかのぼる（Shift+Home / End で両端、入力すれば最新へ）"),
                entry("F12", None, "zoom focused surface (toggle)", "フォーカス中の面をズーム（トグル）"),
                entry("Shift+F12", None, "zoom active split pane (toggle)", "アクティブな分割ペインをズーム（トグル）"),
                entry(":shellname", None, "name this shell tab for what it is doing (empty puts the number back)", "このシェルタブに用途の名前を付ける（空で番号に戻る）"),
                entry(":sync", None, "synchronize: type into all panes at once (also right-click)", "同時入力：全ペインへ一括入力（右クリックでも）"),
                entry("Ctrl+Shift+Enter / :snip", None, "snippet launcher → send a saved command to the shell; works from the shell too (cian.snippets)", "スニペットランチャー → 定型コマンドをシェルへ送信；シェルからも可（cian.snippets）"),
                entry("drag", None, "select text; it is copied to the clipboard on release", "テキスト選択；離すとクリップボードにコピー"),
                entry("right-click", None, "menu: paste, log, SFTP, text encoding, color", "メニュー：貼り付け、ログ、SFTP、文字コード、色"),
                entry("Esc", None, "back to files (full-screen apps keep it)", "ファイルに戻る（全画面アプリはEscを保持）"),
            ],
        ),
    ]
}

/// Render the manual in `lang`, folding in the user's `init.lua` key overrides.
///
/// A user-bound key is appended to the action's built-in keys, matching what
/// the running app does (a binding replaces its default; extra aliases show up
/// here so the manual and the keyboard agree).
/// The manual, keeping only the lines that mention the viewer.
///
/// `?` inside a file should answer "what can I do *here*", and the whole
/// manual — every file operation, every transfer, every SSH key — buries that
/// answer among four screens of things this window cannot do.
/// `?` in the viewer: the keys of *this* window, grouped the way vi's own
/// quick reference is — by what you are doing, not by where the code puts
/// them.
///
/// Written out rather than filtered out of the whole manual. That filter
/// matched on the word "viewer", so it produced whatever happened to mention
/// it — `:nobom` under a heading called "move" — and missed every key that
/// did not, which was most of them.
pub(crate) fn viewer_manual_lines(lang: Lang) -> Vec<String> {
    // (keys, english, japanese). An empty key line is a blank separator.
    type Row = (&'static str, &'static str, &'static str);
    const MOVE: &[Row] = &[
        ("h j k l", "left, down, up, right", "左・下・上・右"),
        ("w  b", "word forward, word back", "次の語・前の語"),
        ("0  $", "start, end of line", "行頭・行末"),
        ("gg  G", "top, bottom — 48G is line 48", "先頭・末尾 — 48G で48行目"),
        ("{count}", "before a motion, repeats it: 3j, 5w, 2}", "動作の前に付けて回数指定: 3j 5w 2}"),
        ("Ctrl+D  Ctrl+U", "half a page down, up", "半ページ下・上"),
        ("Ctrl+F  Ctrl+B", "a page down, up", "1ページ下・上"),
        ("{  }", "previous, next blank line", "前後の空行へ"),
        ("%", "the matching bracket", "対応する括弧へ"),
        ("gg  G  5gg", "the top, the bottom, line 5", "先頭・末尾・5行目"),
        ("w b e", "word by word — a word stops at punctuation", "単語単位 — 記号で区切られます"),
        ("W B E  ge gE", "…and WORD by WORD, which runs to the next space", "…WORD 単位（空白まで一続き）・ge は前の語の末尾へ"),
        ("]]  [[", "next, previous heading or definition", "次・前の見出し／定義"),
        ("zz  zt  zb", "this line to the middle, top, bottom", "この行を中央・上・下へ"),
        ("m a   ' a", "set a mark here, jump back to it (` a for the column)", "ここにマーク／マークへ戻る（` a は桁も）"),
        ("Ctrl+O  Ctrl+I", "back and forward through the places you jumped from", "ジャンプ元を戻る・進む"),
    ];
    const FIND: &[Row] = &[
        ("/", "search — bare text is literal, /re/ is a regex", "検索 — 素の文字はリテラル、/re/ は正規表現"),
        ("n  N", "next, previous match", "次・前の一致"),
        ("*  #", "search the word under the cursor, forward, back", "カーソル位置の語を前方・後方検索"),
        (":r", "replace what you just searched for", "直前の検索語を置換"),
        (":s/old/new/", "replace — g all on a line, c confirm, i ignore case", "置換 — g 行内全部・c 確認・i 大小無視"),
    ];
    const SELECT: &[Row] = &[
        ("v  V", "select by character, by line", "文字選択・行選択"),
        ("Ctrl+Q  Alt+v", "rectangular selection (:block if neither arrives)", "矩形選択（どちらも届かなければ :block）"),
        ("in one: $ then A", "…to the end of every line, however ragged", "矩形で $ のあと A — 長さの違う各行の末尾に追記"),
        ("Ctrl+A", "select the whole file", "ファイル全体を選択"),
        ("o", "swap which end of the selection moves", "選択の伸ばす側を入れ替え"),
        ("y", "copy the selection", "選択をコピー"),
        ("p  P", "paste after, at the cursor", "カーソルの後・位置に貼り付け"),
    ];
    // The keys everything else in the world uses for the same seven things.
    // They work while reading, while editing and over a selection, and each
    // has a `:` twin for the terminal that keeps Ctrl to itself.
    const SHORTCUTS: &[Row] = &[
        ("Ctrl+S", "save (:w)", "保存（:w）"),
        ("Ctrl+C  Ctrl+X", "copy, cut — the selection, or this line", "コピー・切り取り — 選択、なければこの行"),
        ("Ctrl+V", "paste (p and P place it vi's way)", "貼り付け（p・P は vi 流の位置）"),
        ("Ctrl+Z", "undo (u, :undo)", "取り消し（u・:undo）"),
        ("Ctrl+Y  Ctrl+R", "redo (:redo)", "やり直し（:redo）"),
        ("Ctrl+A", "select the whole file", "ファイル全体を選択"),
        ("Ctrl+H", "replace (:replace) — see below", "置換（:replace）— 下記参照"),
    ];
    const REPLACE: &[Row] = &[
        ("Ctrl+H  :replace", "the replace bar — two fields, on the line below", "置換バーを開く — 2欄、下部の入力行に出ます"),
        ("Tab", "move between find and replacement", "置換前 ↔ 置換後"),
        ("Enter", "replace this one and stop on it", "1件だけ置換して、そこで止まる"),
        ("Shift+Enter", "replace all of them", "すべて置換"),
        ("Alt+n", "the next match, without replacing it", "置換せずに次の一致へ"),
        ("Alt+r", "as typed → * ? wildcard → regex, in that order", "文字通り → ワイルドカード(*?) → 正規表現 の順に切替"),
        ("Alt+c  Alt+w", "match case, whole words", "大小区別・単語単位"),
        (r"\n \t \r", r"in either field — \r is a CR inside a line; :lf converts the file's endings", r"どちらの欄でも使える — \r は行内の CR。ファイル全体の改行は :lf / :crlf"),
        ("*", "wildcard: any text. In regex it repeats the character before it — there, any text is `.*`", "ワイルドカードでは任意の文字列。正規表現では直前の文字の繰り返しなので、任意の文字列は `.*`"),
        (":s/old/new/gci", "the same thing said vi's way", "同じことを vi 流に書く場合"),
    ];
    const GRAMMAR: &[Row] = &[
        ("{op}{motion}", "d c y take any motion — dw d$ d} dfx c2w y%", "d c y はどの移動とも組める — dw d$ d} dfx c2w y%"),
        ("dd  cc  yy", "…or the whole line, doubled", "…重ねると行単位"),
        ("{op}i{obj}", "inside a word, quotes, brackets — diw ci\" di( di{", "語・引用符・括弧の内側 — diw ci\" di( di{"),
        ("{op}a{obj}", "…and the delimiters with it — daw da( da\"", "…区切りごと — daw da( da\""),
        ("f x  t x", "to the next x, or just before it (F T backwards)", "次の x へ／その手前へ（F T は後方）"),
        (";  ,", "repeat that, forwards and backwards", "直前の f/t を前方・後方に繰り返し"),
    ];
    const EDIT: &[Row] = &[
        ("i a A o O I", "insert — before, after, at the end of the line, on a new one", "挿入 — 前・後・行末・新しい行"),
        ("s  S  C", "substitute a character, a line, to the end of the line", "1文字・1行・行末までを打ち直す"),
        ("r x  R", "stamp one character over this one (3rx three), or overwrite until Esc", "1文字を上書き（3rx で3文字）・R は Esc まで上書き"),
        ("x  dd  D", "delete a character, a line, to end of line", "1文字・1行・行末まで削除"),
        ("gJ  :combine", "join the next line up — gJ without a space, :combine with one", "次行を連結 — gJ は空白なし、:combine は空白あり"),
        (":combine 3  :combine!", "three lines, or without the space", ":combine 3 で3行、:combine! は空白なし"),
        ("d", "cut the selection", "選択を切り取り"),
        (">>  <<", "shift lines by a tab stop (> and < on a selection)", "行をタブ幅ずらす（選択中は > と <）"),
        ("~", "swap the case under the cursor", "カーソル位置の大小を反転"),
        ("jj  ｊｊ  っｊ", "leave insert mode — the last two are what a Japanese IME makes of pressing j twice", "挿入モードを抜ける ── 後ろ2つは、IME オンで j を2回押したときに出るもの"),
        ("ZZ  ZQ", "save and close / close without saving", "保存して閉じる ／ 保存せずに閉じる"),
        ("u", "undo (Ctrl+Z)", "取り消し（Ctrl+Z）"),
        ("Ctrl+R", "redo (Ctrl+Y, :redo)", "やり直し（Ctrl+Y・:redo）"),
        (".", "do that change again", "直前の変更をもう一度"),
        ("V then I  A", "insert at the start, end of every selected line", "選択全行の先頭・末尾に挿入"),
        (":edit", "open it in your own editor", "外部エディタで開く"),
        (":notepad  :editstyle vim", "swap the whole grammar: notepad keys, or vi's back again (T, or the panel's menu)", "文法ごと切替：メモ帳のキー／vi のキーに戻す（T かパネルのメニューでも）"),
        ("in notepad style", "Shift+arrows select, Alt+Shift a rectangle, Ctrl+arrows by word; no normal mode, so : is a colon and Esc ×3 leaves", "メモ帳文法：Shift+矢印で選択・Alt+Shift で矩形・Ctrl+矢印で単語。ノーマルモードが無いので : はただの文字、Esc 3回で退出"),
    ];
    const FILES: &[Row] = &[
        ("F2  Shift+F2", "next, previous open file", "次・前の開いているファイル"),
        ("Shift+F8  Shift+F9", "split left-right, top-bottom", "左右・上下に分割"),
        ("Shift+F10", "close the split", "分割を解除"),
        ("Shift+H  Shift+L", "cross to the other half", "もう片方へ移動"),
        ("=", "mark what differs between the halves", "左右の差分に印"),
        ("]c  [c", "step through those differences", "差分を順に移動"),
        (":w  :q  :wq  :q!", "save, close, save and close, close discarding", "保存・閉じる・保存して閉じる・破棄して閉じる"),
        (":w <name>", "save it as that, and go on editing it", "その名前で保存し、以後それを編集"),
        ("Enter (in a listing)", "open a file in this panel", "一覧で Enter — このパネルで開く"),
        ("F3 (in a listing)", "open it in the *other* pane instead", "一覧で F3 — 反対のペインで開く"),
        ("F12", "this panel fills the window, and back", "このパネルを全画面に／戻す"),
        ("Shift+H  L  J", "focus the left pane, the right, the shell", "左ペイン／右ペイン／シェルへ移動"),
        ("Tab", "cross to the pane beside it, and back", "隣のペインへ移動／戻る"),
        ("Shift+Tab", "the next open file in this panel (F2 / Shift+F2 too)", "このパネルの次のファイルへ（F2 / Shift+F2 でも）"),
        (":new", "a blank file to type into, docked here", "空のファイルをここに開く"),
        ("]c  [c  Tab", "next / previous difference, while comparing", "比較中：次・前の差分へ"),
        ("✕", "the button in the corner closes it too — Esc does not", "右上の ✕ でも閉じる — Esc では閉じません"),
    ];
    const VIEW: &[Row] = &[
        ("Space  za  zA", "fold this section, toggle every fold", "この節を折りたたむ・全体を切替"),
        (":outline  :ruler", "the shape column, the column scale", "アウトライン列・ルーラー"),
        (":version", "which build this is, and what pictures use", "このビルドと画像の描画方式"),
        (":image", "pictures: the terminal's protocol ↔ half-blocks", "画像: 端末のプロトコル ↔ 半角ブロック"),
        ("a long line", "the view follows the cursor sideways; the bars on the frame say how much is off screen", "長い行は横スクロールで追従。枠のバーが画面外の量を示します"),
        (":ws", "show tabs, trailing spaces, line endings", "タブ・行末空白・改行を表示"),
        (":preview", "rendered Markdown ↔ source", "Markdown 表示 ↔ ソース"),
        (":enc", "force a text encoding", "文字コードを指定"),
        (":blame", "who last changed each line", "各行の最終変更者"),
        (":mermaid", "open the mermaid diagrams in a browser", "mermaid 図をブラウザで開く"),
    ];
    const ASK: &[Row] = &[
        (":summary", "summarise this file", "このファイルを要約"),
        ("Shift+Enter", "the menu — ask, copy, theme (right-click too)", "メニュー — 相談・コピー・テーマ（右クリックでも）"),
    ];
    const LINES: &[Row] = &[
        (":sort :rsort :uniq", "order and de-duplicate", "並べ替え・重複除去"),
        (":han  :zen", "full-width ↔ half-width — the selection, or the file", "全角 ↔ 半角 — 選択範囲、なければファイル全体"),
        (":expand  :unexpand", "tabs ↔ spaces", "タブ ↔ 空白"),
        (":lf  :crlf  :nobom", "line ending, byte-order mark", "改行コード・BOM"),
        (":reindent", "one indent ladder for the whole file", "インデントを揃える"),
        (":g/re/d", "delete every line that matches (:v/re/d keeps them)", "一致した行を削除（:v/re/d は一致だけ残す）"),
    ];
    let sections: &[((&str, &str), &[Row])] = &[
        (("The usual shortcuts", "おなじみのショートカット"), SHORTCUTS),
        (("Replace", "置換"), REPLACE),
        (("Move", "移動"), MOVE),
        (("Find and replace", "検索と置換"), FIND),
        (("Select and copy", "選択とコピー"), SELECT),
        (("Operators and objects", "オペレータとテキストオブジェクト"), GRAMMAR),
        (("Edit", "編集"), EDIT),
        (("Whole lines", "行の加工"), LINES),
        (("Files and splits", "ファイルと分割"), FILES),
        (("What is shown", "表示"), VIEW),
        (("Ask", "相談"), ASK),
    ];
    let mut out = vec![match lang {
        Lang::En => "cian — the text editor panel".to_string(),
        Lang::Ja => "cian — テキストエディタパネル".to_string(),
    }];
    for ((en, ja), rows) in sections {
        out.push(String::new());
        out.push(match lang {
            Lang::En => en.to_string(),
            Lang::Ja => ja.to_string(),
        });
        for (keys, e, j) in *rows {
            out.push(format!("  {:<19} {}", keys, if lang == Lang::Ja { j } else { e }));
        }
    }
    out.push(String::new());
    out.push(match lang {
        Lang::En => "  Esc  drop a selection, a search, a half-typed command".to_string(),
        Lang::Ja => "  Esc  選択・検索・入力途中のコマンドを取り消す".to_string(),
    });
    out.push(match lang {
        Lang::En => "  Ctrl+.  or  :man   every key cian has".to_string(),
        Lang::Ja => "  Ctrl+.  または  :man   cian の全キー".to_string(),
    });
    out
}

pub fn manual_lines(keymap: &HashMap<(char, KeyModifiers), Action>, lang: Lang) -> Vec<String> {
    let header = match lang {
        Lang::En => "cian — key manual",
        Lang::Ja => "cian — キー一覧",
    };
    let mut out = vec![header.to_string()];
    for ((en_title, ja_title), entries) in manual_sections() {
        let title = match lang {
            Lang::En => en_title,
            Lang::Ja => ja_title,
        };
        out.push(String::new());
        out.push(title.to_string());
        for e in entries {
            let mut keys = e.keys.to_string();
            if let Some(action) = e.action {
                // Extra keys the user bound to this action, sorted for stability.
                let mut extra: Vec<String> = keymap
                    .iter()
                    .filter(|(_, a)| **a == action)
                    .map(|((c, m), _)| {
                        let mut s = String::new();
                        if m.contains(KeyModifiers::CONTROL) {
                            s.push_str("Ctrl+");
                        }
                        if m.contains(KeyModifiers::ALT) {
                            s.push_str("Alt+");
                        }
                        s.push(*c);
                        s
                    })
                    .collect();
                extra.sort();
                for c in extra {
                    keys.push_str(&format!(", {}", c));
                }
            }
            out.push(format!("  {:<17} {}", keys, e.desc(lang)));
        }
    }
    out
}

/// Plain-text manual for `cian -man`, using the user's own config so the keys
/// it lists match the keys that will actually work — and its `lang` option.
pub fn manual_text() -> String {
    let config = cian_lua::load();
    let mut keymap: HashMap<(char, KeyModifiers), Action> = HashMap::new();
    for (spec, name) in &config.keymaps {
        if let (Some(k), Some(a)) = (crate::theme::parse_key_spec(spec), action_from_name(name)) {
            keymap.insert(k, a);
        }
    }
    let lang = Lang::from_opt(config.options.lang.as_deref());
    manual_lines(&keymap, lang).join("\n")
}

/// The picture renderer for `how`: the terminal's answer (`auto`), a named
/// protocol, or none at all (`blocks`).
///
/// A terminal can be wrong about itself — iTerm2 answering the kitty query
/// and then drawing nothing — so naming one is a step away rather than a
/// config file away. `None` means the half-block renderer, which is a worse
/// picture and always a picture.
pub(crate) fn image_picker(how: &str) -> Option<ratatui_image::picker::Picker> {
    use ratatui_image::picker::{Picker, ProtocolType as P};
    use std::io::IsTerminal;
    if how == "blocks" {
        return None;
    }
    // The query below asks the terminal a question and waits for the answer.
    // With no terminal on either end there is nobody to answer it, and the wait
    // is forever — which is exactly what happened: cian's Windows CI job sat on
    // this for six hours, every push, until the runner killed it. macOS and
    // Linux happen to fail the read quickly; Windows does not.
    //
    // Nothing is lost by declining. Half-blocks are what a pipe gets anyway.
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        return None;
    }
    let named = match how {
        "iterm2" => Some(P::Iterm2),
        "kitty" => Some(P::Kitty),
        "sixel" => Some(P::Sixel),
        _ => None,
    };
    // The query is what knows the font size, which every protocol needs to
    // size a picture in cells — so it runs even when the protocol is named.
    let mut picker = Picker::from_query_stdio().ok()?;
    match named {
        Some(p) => picker.set_protocol_type(p),
        // Half-blocks as the terminal's answer means "no": cian's own cell
        // renderer already does that, with caching.
        None if picker.protocol_type() == P::Halfblocks => return None,
        None => {}
    }
    Some(picker)
}

/// Version line for `cian --version`.
///
/// Includes the commit because "which build am I running?" is otherwise
/// unanswerable, and an old exe left on PATH looks exactly like missing
/// features.
pub fn version_text() -> String {
    format!("cian {} ({})", env!("CARGO_PKG_VERSION"), env!("CIAN_COMMIT"))
}

/// One-screen usage synopsis for `cian -h`.
pub fn usage_text() -> String {
    // Report the paths this build actually resolves rather than the Unix
    // spelling: on Windows `~/.config/...` is not something the user can paste
    // anywhere, and "where does my config go?" is the first thing they need.
    let cfg = cian_lua::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(could not resolve a home directory)".into());
    let shortcuts = ShortcutStore::default_path().display().to_string();

    [
        "cian — a two-pane terminal file manager".to_string(),
        String::new(),
        "USAGE:".to_string(),
        "    cian [LEFT_PATH] [RIGHT_PATH]".to_string(),
        "    cian --macro <FILE.lua>        run a macro file once at startup".to_string(),
        "    cian <FILE.lua>                same (a *.lua argument is a macro)".to_string(),
        "    cian --macro-name <NAME>       run a named macro from your config".to_string(),
        String::new(),
        "ARGS:".to_string(),
        "    LEFT_PATH     directory for the left pane  (default: current dir)".to_string(),
        "    RIGHT_PATH    directory for the right pane (default: current dir)".to_string(),
        String::new(),
        "OPTIONS:".to_string(),
        "    -h, --help    show this help".to_string(),
        "    -V, --version show the version and commit".to_string(),
        "    -man, --man   show the full key manual (also ? or Ctrl+. in-app)".to_string(),
        "    -m, --macro <FILE.lua>   build a macro's layout at startup".to_string(),
        "    --macro-name <NAME>      build a named macro from macro.lua / macro/".to_string(),
        String::new(),
        "CONFIG:".to_string(),
        format!("    {}", cfg),
        format!("    {}", shortcuts),
        "    (override the config directory with $CIAN_CONFIG_DIR)".to_string(),
        "    a fully-commented starter init.lua is in examples/init.lua".to_string(),
        String::new(),
        "ENVIRONMENT:".to_string(),
        "    CIAN_LOG      append diagnostics to this file (debugging)".to_string(),
    ]
    .join("\n")
}

/// Is the notice below worth showing?
///
/// Split out from the text so both answers can be checked on any platform.
fn wants_terminal_advice(windows: bool, modern: bool) -> bool {
    windows && !modern
}

/// Note when the host terminal will not do cian justice.
///
/// cian cannot restyle the console it was launched into — the font and colors
/// belong to the host. Running `cian-tui.exe` straight from Explorer or cmd
/// lands in the legacy console, where Nerd Font icons become boxes. Saying so
/// once at startup beats leaving it looking broken.
fn terminal_advice() -> Vec<String> {
    vec![
        "This looks like the legacy Windows console.".to_string(),
        "cian works, but file-type icons need a Nerd Font, which that console".to_string(),
        "cannot use. For the intended look, start it from Windows Terminal:".to_string(),
        String::new(),
        "    wt cian-tui".to_string(),
        String::new(),
        "or from WezTerm — or run cian.exe, which brings its own font and its".to_string(),
        "own window. (This notice only appears in the legacy console.)".to_string(),
    ]
}

/// Restore the terminal before a panic unwinds out of the TUI.
///
/// Without this, a panic leaves the terminal in raw mode inside the alternate
/// screen: the panic message is invisible, the shell prompt is unusable, and
/// the user has to run `reset`. The hook puts the terminal back first, so the
/// backtrace lands on a normal screen (and in `$CIAN_LOG` if enabled).
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut out = io::stdout();
        let _ = execute!(out, PopKeyboardEnhancementFlags);
        let _ = execute!(out, DisableBracketedPaste);
        let _ = execute!(out, DisableMouseCapture);
        let _ = disable_raw_mode();
        let _ = execute!(out, LeaveAlternateScreen);
        cian_core::log::log(&format!("PANIC: {}", info));
        original(info);
    }));
}

/// Where a well-known folder actually is.
///
/// `~/Desktop` is the whole answer on macOS and on Linux. It is only sometimes
/// the answer on Windows: OneDrive moves the Desktop, Documents and Pictures
/// folders inside itself when "back up your folders" is on — which is what a
/// personal account gets by default — and the Japanese client names them
/// デスクトップ, ドキュメント, ピクチャ. `~/Desktop` then does not exist, cian
/// left the entry out of よく使う項目, and it looked as though it had simply
/// forgotten the desktop.
///
/// Checked in the order they should win: the plain path, then whatever OneDrive
/// says about itself, then the OneDrive folder in the home directory.
pub(crate) fn known_dir(home: &Path, english: &str, japanese: &str) -> Option<PathBuf> {
    let mut roots = vec![home.to_path_buf()];
    for var in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
        if let Some(v) = std::env::var_os(var) {
            roots.push(PathBuf::from(v));
        }
    }
    roots.push(home.join("OneDrive"));
    roots
        .iter()
        .flat_map(|root| [root.join(english), root.join(japanese)])
        .find(|p| p.is_dir())
}

/// The directory to open when no path was given on the command line: the
/// configured `home`, else the Desktop, else the home directory, else `.`.
fn default_home(config: &cian_lua::Config) -> PathBuf {
    if let Some(h) = &config.options.home {
        let p = expand_path(h);
        if p.is_dir() {
            return p;
        }
    }
    if let Some(home) = home_dir() {
        if let Some(desktop) = known_dir(&home, "Desktop", "デスクトップ") {
            return desktop;
        }
        if home.is_dir() {
            return home;
        }
    }
    PathBuf::from(".")
}

/// What to run once, automatically, at startup — driven by the command line.
/// This is cian's TeraTerm-`.ttl`-style hook: point it at a macro and cian comes
/// up with that layout already built.
pub enum StartupMacro {
    /// Nothing — the normal interactive start.
    None,
    /// `--macro <file>` (or a `*.lua` argument): load this file and run its
    /// first macro once.
    File(PathBuf),
    /// `--macro-name <name>`: run a macro of this name from the loaded config.
    Named(String),
}

/// A transfer ceiling as a number of bytes a second: `2M`, `500k`, `1.5MB/s`.
///
/// Written the way anyone writes a speed, and read the way everyone means it —
/// `M` is a million bytes here, not 1,048,576, because a line rented in
/// megabits is sold in powers of ten and a limit that is 5% out is a limit that
/// argues with the invoice.
pub(crate) use cian_core::parse_rate;

/// The same number written the way it was asked for.
pub(crate) fn rate_text(bps: u64) -> String {
    match bps {
        b if b >= 1_000_000_000 => format!("{:.1}G/s", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.1}M/s", b as f64 / 1e6),
        b if b >= 1_000 => format!("{:.0}k/s", b as f64 / 1e3),
        b => format!("{b}B/s"),
    }
}

/// Build the application state: config, theme, session, startup macro.
///
/// Everything done before anyone owns a screen; raw mode and the alternate
/// screen follow it and do not belong in here.
fn prepare_app(
    left: Option<PathBuf>,
    right: Option<PathBuf>,
    startup: StartupMacro,
) -> Result<App> {
    // Load user config (never fails; problems are reported below).
    let config = cian_lua::load();

    // With no paths on the command line, pick up where the last session left
    // off; an explicit path always wins over the remembered one.
    let session = if left.is_none() && right.is_none() {
        session::restore()
    } else {
        None
    };
    let fallback = default_home(&config);
    let left = left
        .or_else(|| session.as_ref().and_then(|s| s.left_dir()))
        .unwrap_or_else(|| fallback.clone());
    let right = right
        .or_else(|| session.as_ref().and_then(|s| s.right_dir()))
        .unwrap_or(fallback);

    // How wide a tab reaches, before anything measures one.
    if let Some(w) = config.options.tab_width {
        cian_core::viewer::set_tab_width(w);
    }
    // Resolve and install the color theme before any drawing happens.
    let theme_errors = theme::install(
        &config.theme,
        config.options.borders.as_deref(),
        config.options.nerd_fonts.unwrap_or(true),
    );
    // A theme chosen via `:theme` in a previous session overrides init.lua's, so
    // the choice survives a restart. Unknown names are ignored (init.lua wins).
    if let Some(name) = load_saved_theme() {
        if let Some(t) = theme_preset(&name) {
            set_theme(t);
        }
    }

    // Collect all non-fatal config issues for a single startup notice.
    let mut startup_errors = config.errors.clone();
    startup_errors.extend(theme_errors);
    for (c, name) in &config.keymaps {
        if action_from_name(name).is_none() {
            startup_errors.push(format!("keymap: unknown action {:?} (key '{}')", name, c));
        }
    }

    if wants_terminal_advice(cfg!(windows), theme::modern_terminal()) {
        startup_errors.extend(terminal_advice());
    }

    let mut app = App::new(left, right, config)?;
    // Bring back past chat conversations so `Ctrl+R` in the chat spans restarts.
    // Install the cloud-sweep policy before anything can sweep.
    cian_core::cloud::set_include(app.config.options.read_cloud_files.unwrap_or(false));
    app.ai_history = ai::restore_ai_history();
    // Probe AI availability off-thread so the first right-click never blocks on
    // python starting up.
    app.spawn_ai_probe();
    // Put back the font size chosen in an earlier session.
    app.apply_saved_font();
    // Restore which pane had focus, if a session set it.
    if session.as_ref().map(|s| s.focused_right()).unwrap_or(false) {
        app.focus(FocusedPane::Right);
    }
    if !startup_errors.is_empty() {
        let mut lines = vec!["config loaded with issues:".to_string(), String::new()];
        let total = startup_errors.len();
        lines.extend(startup_errors.into_iter().take(10));
        if total > 10 {
            lines.push(format!("... and {} more", total - 10));
        }
        app.popup = Popup::Notice { lines };
    }

    // A startup macro (from `--macro` / `--macro-name` / a `*.lua` argument):
    // queue it so it builds as soon as the shell is up, like a TeraTerm `.ttl`.
    match startup {
        StartupMacro::None => {}
        StartupMacro::Named(name) => {
            if !app.start_macro_by_name(&name) {
                app.message = Some(format!("no macro named {:?} (check macro.lua / macro/)", name));
            }
        }
        StartupMacro::File(path) => match cian_lua::macros::load(&path) {
            Ok(ms) if !ms.is_empty() => app.begin_macro(&ms[0]),
            Ok(_) => app.message = Some(format!("{}: no macro found in file", path.display())),
            Err(e) => app.message = Some(format!("macro {}: {}", path.display(), e)),
        },
    }

    Ok(app)
}

pub fn run(left: Option<PathBuf>, right: Option<PathBuf>, startup: StartupMacro) -> Result<()> {
    let mut app = prepare_app(left, right, startup)?;

    install_panic_hook();
    cian_core::log::log("cian starting");

    // Name the window. Costs nothing and stops a bare `cian.exe` from sitting
    // in a console still labelled with whatever launched it.
    let _ = execute!(io::stdout(), SetTitle("cian"));

    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    // Ask the terminal to disambiguate Ctrl-h / Ctrl-i / Ctrl-m from Backspace/Tab/Enter.
    // Supported by WezTerm, kitty, foot, etc. Silently ignored elsewhere.
    //
    // A terminal that takes the request but reports the result in a form this
    // build cannot read loses every Ctrl combination while plain keys keep
    // working — the whole shortcut set goes quiet at once, which is not a
    // symptom anyone would connect back to a startup handshake. `CIAN_LEGACY_KEYS=1`
    // skips the request, both as the way to confirm that is what happened and
    // as the way to keep working while it is true.
    let legacy_keys = std::env::var("CIAN_LEGACY_KEYS").is_ok_and(|v| v != "0" && !v.is_empty());
    let kbd_enhanced = !legacy_keys
        && execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
    app.kbd_enhanced = kbd_enhanced;

    // Ask the terminal whether it can draw real images (kitty / iTerm2 /
    // sixel). Queried here — after the alternate screen, before any events are
    // read — per ratatui-image's contract. Halfblocks (the answer everywhere
    // else) means "no": cian's own cell renderer already does that, with
    // caching, so the picker is only kept when it buys actual pixels.
    app.gfx_picker = image_picker(state_get("images").as_deref().unwrap_or("auto"));

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app);

    // Remember where the panes were for next launch.
    app.save_session();
    // Give the keyboard back the way it was found.
    app.release_ime();
    // And end every shell before anything starts dropping. Closing a
    // pseudo-console on Windows waits for the program inside it, and a wedged
    // shell never leaves — which turns quitting into a hang. See
    // [`cian_pty::PtySession::kill_now`].
    app.shell.kill_all();

    if kbd_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(terminal.backend_mut(), DisableBracketedPaste);
    let _ = execute!(terminal.backend_mut(), DisableMouseCapture);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

/// Suspend the TUI, run the external editor attached to the real terminal on
/// the queued file, then restore the alternate screen and reload. cian owns the
/// terminal here, so this is where the leave/enter dance belongs.
fn suspend_and_edit<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let Some(edit) = app.pending_edit.take() else { return Ok(()) };
    let Some(cmd) = edit::resolve_editor(&app.config) else {
        app.message = Some(tr(
            app.lang,
            "no editor found — install nvim/vim/vi, or set cian.set_option(\"editor\", …)",
            "エディタが見つかりません — nvim/vim/vi を入れるか cian.set_option(\"editor\", …) を設定してください",
        ).into());
        return Ok(());
    };

    // Hand the terminal back to a normal cooked state for the editor.
    let mut out = io::stdout();
    disable_raw_mode()?;
    let _ = execute!(out, PopKeyboardEnhancementFlags);
    execute!(out, DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen)?;

    // `Command::new`, deliberately, where everything else in cian uses
    // `proc::quiet`: this is the one program cian starts *for* the user on the
    // terminal they are looking at, and vim needs the console it inherits.
    // Denying it one would open the editor into nowhere.
    let status = Command::new(&cmd[0]).args(&cmd[1..]).arg(&edit.path).status();

    // Take it back and rebuild the screen.
    enable_raw_mode()?;
    execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    let _ = execute!(
        out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    terminal.clear().map_err(|e| anyhow::anyhow!("{e}"))?;

    match &status {
        Ok(s) if s.success() || s.code().is_some() => {}
        Ok(_) => app.message = Some("editor exited abnormally".into()),
        Err(e) => app.message = Some(format!("could not launch editor: {}", e)),
    }

    match &edit.kind {
        edit::EditKind::File { title, reopen_viewer } => {
            // The file may have changed on disk; refresh the panes and, if the
            // edit came from the viewer, re-open it on the (changed) file.
            app.reload_both();
            // If it was fetched from a remote pane, push the edit back up.
            app.reupload_remote(&edit.path);
            if *reopen_viewer {
                app.open_viewer_at(&edit.path, title, 0);
            }
        }
        edit::EditKind::BulkRename { dir, names } => {
            // Only apply a list the editor saved and exited zero from; `:cq`
            // (non-zero) is the escape hatch and cancels the whole batch.
            if matches!(&status, Ok(s) if s.success()) {
                app.finish_editor_rename(&edit.path, dir, names);
            } else {
                let _ = std::fs::remove_file(&edit.path);
                app.message =
                    Some(tr(app.lang, "bulk rename cancelled", "一括リネームを中止しました").into());
            }
        }
    }
    Ok(())
}

/// The first line of an error chain — the status line has one row, and the
/// `Caused by:` tail belongs in the log.
fn first_line(e: &anyhow::Error) -> String {
    e.to_string().lines().next().unwrap_or_default().to_string()
}

impl App {
    /// Advance everything that runs between keystrokes: background jobs
    /// landing their results, animations, the shell's own output, the input
    /// method following the mode. Returns whether any of it changed the
    /// screen.
    ///
    /// Split out of the event loop so the windowed front end can share it —
    /// there are two loops now (a terminal one that blocks on `event::poll`
    /// and a windowed one driven by winit's callbacks), and only the driving
    /// is different. Everything that happens per turn lives here.
    ///
    /// The one thing left behind is the pending edit: handing a real terminal
    /// to vim is something only the terminal build can do.
    pub(crate) fn tick_background(&mut self) -> bool {
        let mut redraw = false;
        // Repaint when any pane in the active shell tab produced new output.
        if self.shell.take_active_tab_dirty() {
            redraw = true;
        }
        // A heavy preview waiting for the cursor to settle needs a frame to
        // arrive once it has — otherwise it waits for the next keystroke,
        // which is exactly the one that moves off it again.
        if self.preview_wanted.is_some() || self.preview_decode.is_some() {
            redraw = true;
        }
        // While a pane is recording, keep the frame alive so its carmine
        // border can pulse — throttled to ~8 fps, which is plenty for a
        // 10-second cycle and stays cheap.
        if self.any_logging() && self.last_pulse.elapsed() >= Duration::from_millis(125) {
            self.last_pulse = Instant::now();
            redraw = true;
        }
        // A shell that is running, is on screen, and has drawn nothing: say so
        // in the log at widening intervals. Silence is the one state that
        // leaves no trace of itself, and "the log stops" reads the same as
        // "cian stopped" — so it is written down.
        if cian_core::log::enabled() {
            if let Some(s) = self.shell.active_session() {
                let secs = s.age().as_secs();
                if s.screen_is_blank() && matches!(secs, 5 | 15 | 30 | 60) && self.blank_said != secs
                {
                    self.blank_said = secs;
                    cian_core::log::log(&format!(
                        "shell: {secs}s in, still nothing on its screen",
                    ));
                }
            }
        }
        // Install the shell tab once its background spawn (see `ensure`) lands.
        if self.shell.poll_pending() {
            // ...and say so if what started was not what was asked for.
            if let Some(note) = self.shell.note.take() {
                self.message = Some(note);
            }
            redraw = true;
        }
        // Advance a running layout macro (splits, colours, commands) once the
        // shell is idle between spawns.
        if self.macro_run.is_some() && self.tick_macro() {
            redraw = true;
        }
        // Install a finished remote directory listing into the download browser.
        if self.remote_pane_ls.is_some() && self.poll_remote_pane_ls() {
            redraw = true;
        }
        if self.remote_view.is_some() && self.poll_remote_view() {
            redraw = true;
        }
        if self.remote_mut.is_some() && self.poll_remote_mut() {
            redraw = true;
        }
        if self.remote_ls.is_some() && self.poll_remote_ls() {
            redraw = true;
        }
        // Install the AI availability probe's result (unblocks the AI menu).
        if self.ai_probe.is_some() && self.poll_ai_probe() {
            redraw = true;
        }
        // Keep repainting while the startup splash spins.
        if self.is_starting_up() {
            redraw = true;
        }
        // A finished file/step count shows its report.
        if self.du_job.is_some() && self.poll_du() {
            redraw = true;
        }
        // The file finder fills in while it is being typed into.
        if self.file_scan.is_some() && self.poll_file_scan() {
            redraw = true;
        }
        if self.count_job.is_some() && self.poll_count() {
            redraw = true;
        }
        // Put the input method where this moment wants it — off while cian is
        // driven, on the moment it takes text. Compares one bool; only a
        // change costs anything.
        self.sync_ime();
        // A connection picked before the shell finished starting.
        if self.pending_shell_input.is_some() {
            self.flush_pending_shell_input();
            redraw = true;
        }
        // ssh asks for a password on its own schedule, so watch for the prompt
        // rather than sending blindly.
        if self.pending_auth.is_some() {
            redraw |= self.poll_pending_auth();
        }
        // A freshly-created split grows in from nothing.
        if let Some((tab, node)) = self.shell.just_split.take() {
            self.start_anim(AnimKind::Ratio {
                target: DividerTarget::ShellSplit { tab, node },
                from: 100,
                to: 50,
            });
        }
        // Drive any transition in flight, landing it when its time is up.
        if let Some(a) = self.anim {
            redraw = true;
            if a.done() {
                self.finish_anim();
            }
        }
        // Search results stream in while the walk continues.
        if self.find_job.is_some() {
            redraw |= self.poll_find_job();
        }
        // A directory comparison lands its whole result at once.
        if self.diff_job.is_some() {
            redraw |= self.poll_diff_job();
        }
        // Catch changes made by anything other than cian.
        if self.poll_external_changes() {
            redraw = true;
        }
        // A running file operation reports in over a channel.
        if self.op_job.is_some() {
            redraw |= self.poll_op_job();
        }
        // A pending AI reply lands over its own channel.
        if self.ai_job.is_some() {
            redraw |= self.poll_ai_job();
        }
        // While an AI reply is still in flight, keep repainting so the
        // "thinking" spinner actually spins (the poll above returns false until
        // the answer lands, which would otherwise let the loop go idle).
        if self.ai_job.is_some() {
            redraw = true;
        }
        // A running duplicate scan reports its groups when done.
        if self.dupes_job.is_some() {
            redraw |= self.poll_dupes_job();
        }
        // A fading flash needs frames of its own; clear it once it expires so
        // the loop can go back to sleep.
        if self.flash.is_some() {
            redraw = true;
            if !self.flash_active() {
                self.flash = None;
            }
        }
        // If the focused pane's shell has exited (e.g. the user typed `exit`),
        // close that pane; if its tab (and the whole panel) empties, return to
        // the files so we never strand the user typing into a dead shell.
        if self.focused == FocusedPane::Shell {
            let exited = self
                .shell
                .active_session_mut()
                .map(|s| !s.is_alive())
                .unwrap_or(false);
            // `anim_then.is_none()` guards against re-firing every tick while
            // the closing animation runs (the dead pane is still active until
            // it lands). The animated close shrinks the pane away and merges
            // its sibling back in, the same as Shift+F10 does.
            if exited && self.anim_then.is_none() {
                self.close_shell_pane_animated();
                self.message = Some("shell exited".into());
                redraw = true;
            }
        }
        redraw
    }
}

fn run_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            // A picture just stopped being shown: terminal graphics live
            // outside the cell buffer, so only a real clear removes them.
            // ratatui 0.30's backend error is an associated type with no
            // Send + Sync bound, so `?` into anyhow needs it flattened.
            let repaint = std::mem::take(&mut app.full_repaint);
            if std::mem::take(&mut app.full_clear) {
                terminal.clear().map_err(|e| anyhow::anyhow!("{e}"))?;
            } else if repaint {
                // Every cell painted again, with no blank moment in between:
                // resetting the buffer the next frame is compared against makes
                // the whole surface differ, so all of it is written. `clear`
                // would do this too, and would black the screen out first.
                terminal.swap_buffers();
            }
            terminal.draw(|f| draw(f, app)).map_err(|e| anyhow::anyhow!("{e}"))?;
            needs_redraw = false;
        }
        // Short timeout so live shell output is picked up promptly; we only
        // actually repaint when something changed (input, resize, or new
        // shell output), so the loop stays cheap when idle. While a transition
        // or flash is running we tick faster so the motion stays smooth.
        let tick = if app.anim.is_some()
            || app.flash.is_some()
            || app.op_job.is_some()
            || app.ai_job.is_some()
        {
            16
        } else {
            33
        };
        if event::poll(Duration::from_millis(tick))? {
            // Take everything the terminal already has before painting.
            //
            // A frame per keystroke is right when keystrokes arrive at human
            // speed. They do not always: a terminal that types a paste in
            // rather than bracketing it delivers thousands of key events at
            // once, and a repeat key delivers hundreds — and cian was drawing
            // a full frame for each, so a five-kilobyte paste took as many
            // renders as it had characters. Every event still runs, in order;
            // only the painting between them is dropped, because those frames
            // were never seen.
            let mut drained = 0usize;
            loop {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        // Input always wins over eye candy: land any transition
                        // immediately rather than making the user wait for it.
                        app.finish_anim();
                        // A key must never be able to end the session. Navigation
                        // can fail for ordinary reasons — a directory vanished, a
                        // path turned out not to be one — and the answer to that
                        // is a message, not an exit.
                        if let Err(e) = app.handle_key(key) {
                            app.message = Some(format!("✖ {}", first_line(&e)));
                            cian_core::log::log(&format!("key error: {e:#}"));
                        }
                        needs_redraw = true;
                    }
                    Event::Mouse(m) => {
                        app.handle_mouse(m);
                        needs_redraw = true;
                    }
                    // A terminal paste (Cmd/Ctrl+V, right-click, middle-click)
                    // arrives whole rather than as keystrokes, so it lands in the
                    // active field atomically and its newlines are stripped.
                    // A paste, or a file dropped onto the terminal window — which
                    // arrives the same way, since a terminal answers a drop by
                    // typing the path in. `accept_drop` takes it only when every
                    // item really is a file on disk.
                    Event::Paste(text) => {
                        if !app.accept_drop(&text) {
                            app.insert_into_active_text(&text);
                        }
                        needs_redraw = true;
                    }
                    Event::Resize(_, _) => needs_redraw = true,
                    _ => {}
                }
                drained += 1;
                // Stop for anything that hands the terminal to someone else,
                // and put a ceiling on one turn so a flood still paints.
                if app.should_quit || app.pending_edit.is_some() || drained >= 4096 {
                    break;
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        // An edit request suspends the TUI, runs the editor, and restores.
        // Ahead of `tick_background` rather than in the middle of it: it is
        // the one step that needs the terminal, and the background pollers
        // it displaced only read channels, so their order does not matter.
        if app.pending_edit.is_some() {
            suspend_and_edit(terminal, app)?;
            needs_redraw = true;
        }
        needs_redraw |= app.tick_background();
        if app.should_quit {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests;
