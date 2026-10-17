//! A shell, in the engine.
//!
//! **Not in the window.** Electron's usual answer is `node-pty`: a native
//! module that wants a C++ toolchain and a rebuild against Electron's own ABI
//! — the same several gigabytes this project already refused once, for SFTP.
//! `cian-pty` is `portable-pty` and `vt100`, both plain Rust, and it is the
//! very emulator the terminal build reads its shell through. So there is one
//! emulator rather than two, and nothing to compile on the machine that runs
//! the GUI.
//!
//! The window sends keystrokes and draws a grid. It does not know what an
//! escape sequence is, which is right: interpreting them is a job with twenty
//! years of edge cases in it, and having a second answer to any of them is how
//! two front ends stop looking like one program.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cian_pty::PtySession;

use crate::wire::Out;

/// How often the screen is looked at.
///
/// A shell is the one thing here that changes without being asked, so it is
/// polled — but only the *dirty* flag is polled. Serialising a grid nobody
/// changed is the wasteful part, not the looking.
const TICK_MS: u64 = 30;

/// A password waiting for the prompt that will ask for it.
///
/// Held here rather than sent with the command because `ssh` reads the
/// password from the terminal, not from its arguments — see
/// `cian_core::auth`. The watcher thread below is already looking at every
/// frame, so it is the natural place to notice the question.
struct PendingAuth {
    secret: String,
    until: std::time::Instant,
}

/// Type the held password if this screen is asking for one.
///
/// Never logged, never echoed, never put in a message — it goes from the
/// config straight to the pty and is dropped. Submitted with a carriage
/// return, because `getpass` reads the line ended by CR and a bare `\n` may
/// not end it.
fn send_pending_auth(session: &mut PtySession, slot: &Arc<Mutex<Option<PendingAuth>>>) {
    let Ok(mut held) = slot.lock() else { return };
    let Some(pending) = held.as_ref() else { return };
    if std::time::Instant::now() > pending.until {
        *held = None;
        return;
    }
    let asking = match session.parser().lock() {
        Ok(p) => cian_core::auth::looks_like_password_prompt(&p.screen().contents()),
        Err(_) => false,
    };
    if !asking {
        return;
    }
    let Some(pending) = held.take() else { return };
    let mut bytes = pending.secret.into_bytes();
    bytes.push(b'\r');
    session.write_input(&bytes);
}

pub struct Shell {
    session: Arc<Mutex<PtySession>>,
    stop: Arc<AtomicBool>,
    /// Shared with the watcher thread, which is the one that sees the prompt.
    auth: Arc<Mutex<Option<PendingAuth>>>,
    /// Which shell this is, carried on every screen it sends.
    ///
    /// A hidden tab keeps running — that is the reason to have tabs — and it
    /// keeps producing screens. Without a name on them the window would draw
    /// whichever one moved last, and a build scrolling in tab two would keep
    /// stamping itself over tab one.
    pub id: u64,
    pub rows: u16,
    pub cols: u16,
    /// The command line this panel was started with.
    ///
    /// Kept so the AI can be told which shell it is writing for. `cmd.exe`
    /// batch and PowerShell share almost no syntax, and the suggested command
    /// is *placed at this prompt* — so getting it wrong produces a line that
    /// cannot run in the one place it is going to end up.
    pub program: String,
}

impl Shell {
    /// `program` is what to run — the caller resolves `cian.set_option("shell",
    /// …)` and falls back to the platform's own. **It used to be resolved
    /// here, as `default_shell()` and nothing else**, which meant the option
    /// worked in the terminal build and was silently ignored in this one:
    /// a setting that does nothing is worse than a setting that is missing,
    /// because there is no way to tell from the outside.
    pub fn start(
        id: u64,
        cwd: &std::path::Path,
        rows: u16,
        cols: u16,
        program: &str,
        out: Out,
    ) -> anyhow::Result<Self> {
        let shell = program.to_string();
        // `start` rather than `new`: it falls back to something that will run
        // and says which. A panel with cmd.exe in it and a line saying why
        // beats an empty panel every time — which is exactly what `new` alone
        // produced the first time this was tried.
        let (session, note) = PtySession::start(cwd, &shell, rows, cols)?;
        if let Some(note) = note {
            out.event("shellnote", serde_json::json!({ "note": note }));
        }
        let session = Arc::new(Mutex::new(session));
        let stop = Arc::new(AtomicBool::new(false));

        let auth: Arc<Mutex<Option<PendingAuth>>> = Arc::new(Mutex::new(None));
        let watch = Arc::clone(&session);
        let watch_stop = Arc::clone(&stop);
        let watch_auth = Arc::clone(&auth);
        std::thread::spawn(move || {
            let mut ticks: u32 = 0;
            while !watch_stop.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(TICK_MS));
                // Has the shell exited on its own — `exit`, Ctrl+D, a crash?
                // Nothing used to ask, so a shell the person closed by typing
                // `exit` left a frozen screen that looked alive. Asked twice a
                // second, which is often enough to feel immediate and rare
                // enough not to matter.
                ticks += 1;
                // The second `watch_stop` check is not belt and braces: this
                // thread is asleep for 30ms at a time, and `Drop for Shell`
                // sets the flag *and kills the shell*. So a deliberate close
                // makes `is_alive()` answer no on the very next tick, and the
                // pane the engine has already removed announces its own death
                // to a window that then asks for it to be closed again. Read
                // the flag once more, right where it matters.
                if ticks % 8 == 0
                    && !watch.lock().map(|mut s| s.is_alive()).unwrap_or(false)
                    && !watch_stop.load(Ordering::Relaxed)
                {
                    // A pane whose shell has ended, not a panel to take down:
                    // exactly what Shift+F10 does, which is why one word says
                    // both. The old event said "gone" and the window heard
                    // "the shell is over", so closing one split pane closed
                    // every other pane with it.
                    out.event("shellexit", serde_json::json!({ "id": id }));
                    return;
                }
                let dirty = watch.lock().map(|s| s.take_dirty()).unwrap_or(false);
                if !dirty {
                    continue;
                }
                let Ok(mut s) = watch.lock() else { continue };
                // Before the frame goes out, not after: if this screen is the
                // password prompt, the person should never see it sitting
                // there unanswered.
                send_pending_auth(&mut s, &watch_auth);
                if let Some(mut grid) = render(&s) {
                    grid["id"] = serde_json::json!(id);
                    out.event("shell", grid);
                }
            }
            // Stopped because the Shell was dropped — the engine already knows,
            // having done it. Saying so would be the engine telling the window
            // about the window's own request.
        });

        Ok(Shell { session, stop, auth, id, rows, cols, program: shell })
    }

    /// A handle a worker can hold: writing and waiting, without the Shell
    /// itself (which lives in the session and cannot cross a thread).
    pub fn handle(&self) -> Handle {
        Handle { session: Arc::clone(&self.session) }
    }

    /// Hold a password until this shell is asked for one.
    ///
    /// Armed for `AUTH_WINDOW` and then forgotten, whether or not it was ever
    /// wanted — a keyed host never asks, and a secret left armed is a secret
    /// that could be typed into whatever is on the screen twenty minutes
    /// later.
    pub fn arm_auth(&self, secret: String) {
        if let Ok(mut slot) = self.auth.lock() {
            *slot = Some(PendingAuth {
                secret,
                until: std::time::Instant::now() + cian_core::auth::AUTH_WINDOW,
            });
        }
    }

    pub fn write(&self, bytes: &[u8]) {
        if let Ok(mut s) = self.session.lock() {
            s.write_input(bytes);
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
        if let Ok(mut s) = self.session.lock() {
            s.resize(rows, cols);
        }
    }

    pub fn scroll(&self, lines: isize) {
        if let Ok(s) = self.session.lock() {
            s.scroll_back(lines);
        }
    }

    pub fn to_bottom(&self) {
        if let Ok(s) = self.session.lock() {
            s.scroll_to_bottom();
        }
    }

    /// The screen as it stands, for a window that has just opened the panel and
    /// has nothing to draw yet.
    pub fn screen(&self) -> Option<serde_json::Value> {
        self.session.lock().ok().and_then(|s| render(&s)).map(|mut g| {
            g["id"] = serde_json::json!(self.id);
            g
        })
    }

    /// The window title the shell set, if any. A remote login usually puts
    /// `user@host` here, which is the cheapest way to know where this panel
    /// actually is.
    pub fn title(&self) -> Option<String> {
        let t = self.session.lock().ok().map(|s| s.window_title())?;
        (!t.trim().is_empty()).then_some(t)
    }

    /// The visible screen as plain text, for the AI to read.
    pub fn contents(&self) -> Option<String> {
        self.session
            .lock()
            .ok()
            .and_then(|s| s.parser().lock().ok().map(|p| p.screen().contents()))
    }

    pub fn is_logging(&self) -> bool {
        self.session.lock().map(|s| s.is_logging()).unwrap_or(false)
    }

    pub fn log_path(&self) -> Option<std::path::PathBuf> {
        self.session.lock().ok().and_then(|s| s.log_path())
    }

    pub fn start_log(&self, at: &std::path::Path) -> anyhow::Result<()> {
        self.session
            .lock()
            .map_err(|_| anyhow::anyhow!("シェルに触れません"))?
            .start_log(at)
    }

    pub fn stop_log(&self) {
        if let Ok(s) = self.session.lock() {
            s.stop_log();
        }
    }

    pub fn alive(&self) -> bool {
        self.session.lock().map(|mut s| s.is_alive()).unwrap_or(false)
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut s) = self.session.lock() {
            s.kill_now();
        }
    }
}

/// What a macro's worker holds while it types into a shell it does not own.
pub struct Handle {
    session: Arc<Mutex<PtySession>>,
}

impl Handle {
    pub fn write(&self, bytes: &[u8]) {
        if let Ok(mut s) = self.session.lock() {
            s.write_input(bytes);
        }
    }

    /// Wait until `text` shows up on the screen, or `secs` pass.
    ///
    /// What makes a login macro possible: `sqlplus /nolog` is not ready when
    /// the process starts, it is ready when it says so. Case-insensitive,
    /// because a prompt's capitalisation is not something anyone should have
    /// to get right in a config file.
    pub fn wait_for(&self, text: &str, secs: f64) {
        let want = text.to_lowercase();
        let until = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
        while std::time::Instant::now() < until {
            let seen = self
                .session
                .lock()
                .ok()
                .and_then(|s| s.parser().lock().ok().map(|p| p.screen().contents()))
                .map(|c| c.to_lowercase().contains(&want))
                .unwrap_or(false);
            if seen {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// The screen, as runs of same-looking cells.
///
/// Per cell would be honest and enormous: eighty columns of separate objects,
/// thirty times a second. A run holds while the colours and the emphasis hold,
/// which on a real shell screen means a line is usually one or two of them.
fn render(session: &PtySession) -> Option<serde_json::Value> {
    let parser = session.parser().lock().ok()?;
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    let mut lines = Vec::with_capacity(rows as usize);

    // **折り返しの印。** 画面は桁で区切った升目なので、長い1行は何行にも
    // なって並ぶ ── どこが「続き」かを窓へ伝えないと、そこから拾った文字列は
    // 画面の桁で切られ、改行が挿し込まれた形になる（実機で測った、2026-10-17:
    // 600 字の1行が `[149,149,149,149,4]` で戻る）。`vt100` は行ごとにこれを
    // 持っているので、運ぶだけでいい。
    let mut wrapped = Vec::with_capacity(rows as usize);

    for row in 0..rows {
        wrapped.push(screen.row_wrapped(row));
        let mut runs: Vec<serde_json::Value> = Vec::new();
        let mut text = String::new();
        let mut style: Option<Style> = None;
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else { continue };
            // A wide character occupies two columns and reports its text in
            // the first; the second is a continuation and must not be drawn
            // again or every 全角 character doubles.
            if cell.is_wide_continuation() {
                continue;
            }
            let here = Style::of(cell);
            if style.as_ref() != Some(&here) {
                if let Some(was) = style.take() {
                    runs.push(was.json(&text));
                }
                text.clear();
                style = Some(here);
            }
            let c = cell.contents();
            text.push_str(if c.is_empty() { " " } else { c });
        }
        if let Some(was) = style {
            runs.push(was.json(&text));
        }
        lines.push(serde_json::Value::Array(runs));
    }

    let (cr, cc) = screen.cursor_position();
    Some(serde_json::json!({
        "rows": rows,
        "cols": cols,
        "cursor": { "row": cr, "col": cc },
        "hidden": screen.hide_cursor(),
        "scrollback": screen.scrollback(),
        "wrapped": wrapped,
        "title": session.window_title(),
        "lines": lines,
    }))
}

/// What a run of cells looks like.
#[derive(PartialEq, Eq)]
struct Style {
    fg: Option<String>,
    bg: Option<String>,
    bold: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl Style {
    fn of(cell: &vt100::Cell) -> Style {
        Style {
            fg: colour(cell.fgcolor()),
            bg: colour(cell.bgcolor()),
            bold: cell.bold(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        }
    }

    fn json(&self, text: &str) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        o.insert("t".into(), serde_json::Value::String(text.to_string()));
        if let Some(f) = &self.fg {
            o.insert("f".into(), serde_json::Value::String(f.clone()));
        }
        if let Some(b) = &self.bg {
            o.insert("b".into(), serde_json::Value::String(b.clone()));
        }
        for (k, v) in [
            ("bold", self.bold),
            ("it", self.italic),
            ("ul", self.underline),
            ("inv", self.inverse),
        ] {
            if v {
                o.insert(k.into(), serde_json::Value::Bool(true));
            }
        }
        serde_json::Value::Object(o)
    }
}

/// A vt100 colour, as something CSS can use.
///
/// The sixteen named ones stay named — they are the palette the window's theme
/// gets to choose, and freezing them to hex here would mean a shell that
/// ignores 白磁 and 陰翳 alike. Only the 256-colour and true-colour cases,
/// which a program picked deliberately, become fixed values.
fn colour(c: vt100::Color) -> Option<String> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) if i < 16 => Some(format!("c{i}")),
        vt100::Color::Idx(i) => {
            let (r, g, b) = xterm256(i);
            Some(format!("#{r:02x}{g:02x}{b:02x}"))
        }
        vt100::Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
    }
}

/// The xterm 256-colour cube and its grey ramp, which are arithmetic rather
/// than a table anybody needs to read.
fn xterm256(i: u8) -> (u8, u8, u8) {
    if i < 232 {
        let i = i - 16;
        let step = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
        (step(i / 36), step((i / 6) % 6), step(i % 6))
    } else {
        let v = 8 + (i - 232) * 10;
        (v, v, v)
    }
}

/// How a tab's shells are arranged.
///
/// A tree rather than a list, because that is what splitting *is*: every split
/// takes one pane and makes it two, and the pane it takes might itself be half
/// of an earlier split. A flat list of panes with one direction covers "two
/// side by side" and falls apart the moment somebody splits the right-hand one
/// downwards — which is the second thing anybody does.
pub enum Node {
    Leaf(u64),
    Split {
        down: bool,
        /// What fraction the first half keeps, 0.05–0.95.
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

impl Node {
    /// Replace the leaf `id` with a split holding it and `fresh`.
    pub fn split_at(&mut self, id: u64, fresh: u64, down: bool) -> bool {
        match self {
            Node::Leaf(mine) if *mine == id => {
                *self = Node::Split {
                    down,
                    ratio: 0.5,
                    a: Box::new(Node::Leaf(id)),
                    b: Box::new(Node::Leaf(fresh)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split_at(id, fresh, down) || b.split_at(id, fresh, down),
        }
    }

    /// Remove the leaf `id`, collapsing the split it was half of.
    ///
    /// Returns false when `id` is the only leaf: a tab with no panes is not a
    /// tab, and closing the last pane means closing the tab.
    pub fn close(&mut self, id: u64) -> bool {
        let replacement = match self {
            Node::Leaf(_) => return false,
            Node::Split { a, b, .. } => match (a.as_ref(), b.as_ref()) {
                (Node::Leaf(x), _) if *x == id => Some(std::mem::replace(b.as_mut(), Node::Leaf(0))),
                (_, Node::Leaf(y)) if *y == id => Some(std::mem::replace(a.as_mut(), Node::Leaf(0))),
                _ => None,
            },
        };
        if let Some(kept) = replacement {
            *self = kept;
            return true;
        }
        match self {
            Node::Split { a, b, .. } => a.close(id) || b.close(id),
            Node::Leaf(_) => false,
        }
    }

    /// Nudge the split that `id` sits under, in the given direction.
    ///
    /// The *nearest* split of the right orientation, walking out from the
    /// leaf: dragging a border means moving the one you can see, and the one
    /// you can see is the innermost that runs the right way.
    pub fn resize(&mut self, id: u64, wider: bool, down_axis: bool) -> bool {
        let Node::Split { down, ratio, a, b } = self else { return false };
        // Inner splits first: the border you can see is the innermost one.
        if a.resize(id, wider, down_axis) || b.resize(id, wider, down_axis) {
            return true;
        }
        if *down != down_axis {
            return false;
        }
        let mut mine = Vec::new();
        a.leaves(&mut mine);
        let mut theirs = Vec::new();
        b.leaves(&mut theirs);
        let first = if mine.contains(&id) {
            true
        } else if theirs.contains(&id) {
            false
        } else {
            return false;
        };
        // Which half the leaf is in decides which way "wider" goes: widening
        // the right-hand pane means giving the first half less, not more.
        let step = if first == wider { 0.05 } else { -0.05 };
        *ratio = (*ratio + step).clamp(0.1, 0.9);
        true
    }

    pub fn leaves(&self, out: &mut Vec<u64>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { a, b, .. } => {
                a.leaves(out);
                b.leaves(out);
            }
        }
    }

    /// Where each pane sits, as fractions of the panel: `(id, x, y, w, h)`.
    ///
    /// Worked out here rather than in the window because the tree is here.
    /// The window places boxes; it does not need to know what a split is.
    /// Where each divider sits, and which split it belongs to: `(a_leaf, down,
    /// x, y, w, h)` in the same fractions `places` uses. `a_leaf` is the first
    /// leaf of the split's left/top half, which is enough to name the split
    /// again when the window says where the divider was dragged to.
    ///
    /// Here for the same reason `places` is here: the window places boxes and
    /// does not need to know what a split is. cian-tui reaches this through
    /// `DividerTarget::ShellSplit`; a window has a pointer and needs the box.
    pub fn dividers(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        out: &mut Vec<(u64, bool, f32, f32, f32, f32)>,
    ) {
        let Node::Split { down, ratio, a, b } = self else { return };
        let r = ratio.clamp(0.05, 0.95);
        let mut first = Vec::new();
        a.leaves(&mut first);
        if let Some(id) = first.first() {
            if *down {
                out.push((*id, true, x, y + h * r, w, 0.0));
            } else {
                out.push((*id, false, x + w * r, y, 0.0, h));
            }
        }
        if *down {
            a.dividers(x, y, w, h * r, out);
            b.dividers(x, y + h * r, w, h * (1.0 - r), out);
        } else {
            a.dividers(x, y, w * r, h, out);
            b.dividers(x + w * r, y, w * (1.0 - r), h, out);
        }
    }

    /// Put the split holding `id` on this axis at `ratio` outright, rather
    /// than stepping it. A drag names a position, not a direction.
    pub fn set_ratio(&mut self, id: u64, down_axis: bool, to: f32) -> bool {
        let Node::Split { down, ratio, a, b } = self else { return false };
        if a.set_ratio(id, down_axis, to) || b.set_ratio(id, down_axis, to) {
            return true;
        }
        if *down != down_axis {
            return false;
        }
        let mut mine = Vec::new();
        a.leaves(&mut mine);
        if !mine.contains(&id) {
            return false;
        }
        *ratio = to.clamp(0.1, 0.9);
        true
    }

    pub fn places(&self, x: f32, y: f32, w: f32, h: f32, out: &mut Vec<(u64, f32, f32, f32, f32)>) {
        match self {
            Node::Leaf(id) => out.push((*id, x, y, w, h)),
            Node::Split { down, ratio, a, b } => {
                let r = ratio.clamp(0.05, 0.95);
                if *down {
                    a.places(x, y, w, h * r, out);
                    b.places(x, y + h * r, w, h * (1.0 - r), out);
                } else {
                    a.places(x, y, w * r, h, out);
                    b.places(x + w * r, y, w * (1.0 - r), h, out);
                }
            }
        }
    }
}

/// Which tab a pane-close applies to.
///
/// `leaves` is each tab's pane ids, in tab order. `named` is the pane the
/// request asked for — `Some` when a shell announced its own death,
/// `None` for Shift+F10, which means "the focused one".
///
/// **`None` for a named pane nobody owns.** That case is a stale event: the
/// shell has already been removed, usually by the very close that killed it,
/// and the announcement arrives afterwards. Treating it as "the focused tab"
/// — which is what the code did — means the dead pane is looked for in a tab
/// it was never part of, not found, and read as "this tab had one pane, so
/// closing it closes the tab". Pressing F10 on the third shell tab therefore
/// also closed the second, splits and all. Reported twice from Windows before
/// the race was found; it is in every close and lands about one time in eight
/// (the watcher checks liveness every eighth 30ms tick, and a deliberate
/// close is exactly what makes it answer no).
pub fn tab_for_close(leaves: &[Vec<u64>], named: Option<u64>, focused: usize) -> Option<usize> {
    match named {
        Some(id) => leaves.iter().position(|ids| ids.contains(&id)),
        None => (focused < leaves.len()).then_some(focused),
    }
}

#[cfg(test)]
mod close_tests {
    use super::*;

    #[test]
    fn a_named_pane_is_closed_in_the_tab_that_owns_it() {
        let tabs = vec![vec![1], vec![2, 3, 4], vec![5]];
        assert_eq!(tab_for_close(&tabs, Some(3), 0), Some(1));
        assert_eq!(tab_for_close(&tabs, Some(5), 0), Some(2));
    }

    #[test]
    fn a_named_pane_nobody_owns_closes_nothing() {
        // The whole bug: this used to answer `Some(focused)`.
        let tabs = vec![vec![1], vec![2, 3, 4]];
        assert_eq!(tab_for_close(&tabs, Some(99), 1), None);
        assert_eq!(tab_for_close(&tabs, Some(99), 0), None);
    }

    #[test]
    fn an_unnamed_close_means_the_focused_tab() {
        let tabs = vec![vec![1], vec![2, 3]];
        assert_eq!(tab_for_close(&tabs, None, 1), Some(1));
        // …and nothing at all when there is no such tab.
        assert_eq!(tab_for_close(&tabs, None, 7), None);
        assert_eq!(tab_for_close(&[], None, 0), None);
    }
}
