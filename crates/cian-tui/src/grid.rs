//! Steering the icon grid.
//!
//! The grid does not read like a list, so it is not driven like one. In every
//! desktop file manager a letter key means *go to the file that starts with
//! it*, and the arrows walk the grid — and that is what people arriving at an
//! icon view already know. So in this view, and only in this view, cian gives
//! the letters up.
//!
//! Repeating a letter walks the files that begin with it, one press per file,
//! wrapping at the end. Typing different letters builds a prefix instead, so
//! `re` finds `README.md` without stopping at `report.pdf` on the way. A pause
//! ends the word: the next letter starts a fresh search rather than extending
//! one the user has forgotten about.

use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::{hit_rect, App, FocusedPane, Mode, Popup};

/// How long a typed prefix stays live. Long enough to finish a word, short
/// enough that a letter pressed a moment later is obviously a new search.
const PATIENCE: Duration = Duration::from_millis(900);

impl App {
    /// Handle a key the grid claims for itself. Returns whether it did.
    ///
    /// Only plain letters and the arrows are taken. Everything else — `:`, `/`,
    /// Enter, Backspace, Space, the digits that pick a tab, every combination
    /// with a modifier — falls through to the keys cian has always had, so the
    /// grid is a different way of *moving*, not a different program.
    pub(crate) fn grid_key(&mut self, key: KeyEvent) -> bool {
        // Both desktop-shaped views, not just the grid.
        //
        // These two are driven with the mouse, and a letter in a view driven
        // with the mouse means *go to the file that starts with it* — that is
        // what every desktop file manager does with a letter, and what anyone
        // arriving at one expects. So in these views cian gives the letters up
        // wholesale: `d` goes to `docs/` rather than deleting, `t` goes to
        // `todo.md` rather than opening a tab. Every one of those commands is
        // still on `:`, on the menu, on an F-key, or under the mouse.
        //
        // The digits go the same way, for the same reason — they picked a tab,
        // and a tab is a thing you can see and click. F1/F2 still walk them.
        if !self.single_pane_view()
            || !matches!(self.popup, Popup::None)
            || self.mode != Mode::Normal
            || self.focused == FocusedPane::Shell
        {
            return false;
        }
        // A modifier means a command, not typing.
        if key.modifiers.intersects(
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
        ) {
            return false;
        }

        let cols = self.icon_cols.max(1);
        match key.code {
            // Only the grid walks in two dimensions; the detail view is a list,
            // and in a list the arrows mean what they have always meant.
            KeyCode::Up if self.icon_view => self.grid_move(-(cols as isize)),
            KeyCode::Down if self.icon_view => self.grid_move(cols as isize),
            KeyCode::Left if self.icon_view => self.grid_move(-1),
            KeyCode::Right if self.icon_view => self.grid_move(1),
            KeyCode::Char(c) if self.typing_moves(c) => self.type_ahead(c),
            _ => return false,
        }
        true
    }

    /// Does this key mean "go to the file that starts with it" in this view?
    ///
    /// The grid gives up every letter and digit: it is a wall of pictures, and
    /// nothing about it invites typing a command.
    ///
    /// The detail view is a listing, and someone who has used cian in a
    /// terminal still reads it as one — so it gives up a named set instead of
    /// the whole alphabet. These are the ones asked for, and they are the ones
    /// that hurt: single letters that do something to a file. What is left —
    /// `f` find, `n`/`N` next match, `e` encoding, `x` launch, `w` close tab —
    /// keeps working, and everything given up is still on `:`, on the menu, on
    /// an F-key or under the mouse.
    fn typing_moves(&self, c: char) -> bool {
        /// Asked for by name. `g`/`G` are vim's top and bottom, `A`/`P` the
        /// shifted pair of `a`/`p`; the rest are cian's one-letter commands.
        const GIVEN_UP: &[char] = &[
            'h', 'q', 'c', 'p', 'g', 'G', 'P', 'r', 'j', 'k', 'a', 'A', 's', 'u', 't', 'd', 'z',
            'v', 'm', 'b',
        ];
        if self.icon_view {
            return c.is_alphanumeric();
        }
        // The digits picked a tab. A tab is a thing you can see and click, and
        // F1/F2 still walk them — while `1` in a listing of numbered files is
        // worth having.
        c.is_ascii_digit() || GIVEN_UP.contains(&c)
    }

    /// Step the cursor by `by` entries, stopping at the ends rather than
    /// wrapping — a grid has corners, and walking off one should feel like it.
    pub(crate) fn grid_move(&mut self, by: isize) {
        let Some(pane) = self.active_pane_mut() else { return };
        let last = pane.entries.len().saturating_sub(1);
        let want = pane.cursor as isize + by;
        pane.cursor = want.clamp(0, last as isize) as usize;
        // A letter typed after moving starts a new search, not a continuation.
        self.type_ahead.clear();
    }

    /// Go to the file this letter names.
    /// The same jump, for a view that is not the grid — see the `q` arm in
    /// `keys.rs`.
    pub(crate) fn type_ahead_jump(&mut self, c: char) {
        self.type_ahead(c);
    }

    fn type_ahead(&mut self, c: char) {
        let now = Instant::now();
        if now.duration_since(self.type_ahead_at) > PATIENCE {
            self.type_ahead.clear();
        }
        self.type_ahead_at = now;

        // The same letter again walks the files beginning with it rather than
        // looking for a name with the letter twice — `jj` is how one asks for
        // the second `j`, not for `jjson`.
        let repeat = self.type_ahead.chars().count() == 1
            && self.type_ahead.chars().next().map(lower) == Some(lower(c));
        if !repeat {
            self.type_ahead.push(c);
        }
        let prefix: String = self.type_ahead.to_lowercase();

        let Some(pane) = self.active_pane() else { return };
        let names: Vec<String> = pane.entries.iter().map(|e| e.name.to_lowercase()).collect();
        let from = if repeat { pane.cursor + 1 } else { 0 };
        let total = names.len();

        // From `from`, all the way round, so a repeat wraps back to the first.
        let found = (0..total)
            .map(|i| (from + i) % total)
            .find(|&i| names[i].starts_with(&prefix));

        match found {
            Some(i) => {
                if let Some(p) = self.active_pane_mut() {
                    p.cursor = i;
                }
            }
            // Nothing starts with what has been typed. Rather than sit on a
            // dead prefix — where every further letter also fails — drop back
            // to just this letter and try again.
            None if prefix.chars().count() > 1 => {
                self.type_ahead.clear();
                self.type_ahead.push(c);
                let one = self.type_ahead.to_lowercase();
                if let Some(i) = names.iter().position(|n| n.starts_with(&one)) {
                    if let Some(p) = self.active_pane_mut() {
                        p.cursor = i;
                    }
                }
            }
            None => {}
        }
    }
}

/// Lowercase a char for comparison. Only the first of a multi-char lowering is
/// kept, which is enough: this compares single keystrokes.
fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The mouse, in the grid.
///
/// The list panes have their own hit-testing in [`crate::mouse`]; none of it
/// applies here, because the grid puts entries in two dimensions and cian's
/// panes have only ever had one. So the grid answers for its own rectangle.
impl App {
    /// Which entry is under this cell, if any.
    ///
    /// Only tiles that actually hold an entry answer — the empty space after
    /// the last file is not the last file, and clicking it should do nothing
    /// rather than jump the cursor to the end.
    pub(crate) fn grid_entry_at(&self, col: u16, row: u16) -> Option<usize> {
        let area = self.grid_area?;
        if !hit_rect(area, col, row) {
            return None;
        }
        let cols = self.icon_cols.max(1);
        let cx = ((col - area.x) / crate::render::TILE_W) as usize;
        let cy = ((row - area.y) / crate::render::TILE_H) as usize;
        if cx >= cols {
            return None;
        }
        let pane = self.active_pane()?;
        let per_page = cols * (area.height / crate::render::TILE_H).max(1) as usize;
        let start = pane.cursor.checked_div(per_page).map_or(0, |page| page * per_page);
        let i = start + cy * cols + cx;
        (i < pane.entries.len()).then_some(i)
    }

    /// Which place in the sidebar is on this row, if any.
    pub(crate) fn sidebar_at(&self, row: u16) -> Option<std::path::PathBuf> {
        self.sidebar_rows.iter().find(|(_, y)| *y == row).map(|(p, _)| p.clone())
    }

    /// Which toolbar button is under this cell, if any.
    pub(crate) fn grid_button_at(&self, col: u16, row: u16) -> Option<crate::GridButton> {
        self.grid_buttons
            .iter()
            .find(|(_, r)| hit_rect(*r, col, row))
            .map(|(b, _)| *b)
    }
}

/// What the grid does when it is clicked.
impl App {
    /// The wheel, and the grid's own scrollbar.
    ///
    /// The grid has no scroll offset of its own — which page is showing is
    /// worked out from where the cursor is — so both of these move the cursor
    /// and let the page follow. That is the same bargain the arrow keys make,
    /// and it is why the bar steps a page at a time rather than gliding.
    pub(crate) fn grid_scroll_mouse(&mut self, ev: MouseEvent) {
        let (col, row) = (ev.column, ev.row);
        // A bar being dragged keeps the pointer until it is let go: the hand
        // wanders off a one-column track constantly.
        if let Some(what) = self.scroll_drag {
            match ev.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.scroll_to_fraction(what, col, row);
                    return;
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.scroll_drag = None;
                    return;
                }
                _ => {}
            }
        }
        if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
            if let Some(t) = self.scroll_tracks.iter().copied().find(|t| hit_rect(t.rect, col, row)) {
                self.scroll_drag = Some(t.what);
                self.scroll_to_fraction(t.what, col, row);
                return;
            }
        }
        // One notch is one row of tiles, whichever way it went.
        let cols = self.icon_cols.max(1) as isize;
        match ev.kind {
            MouseEventKind::ScrollDown => self.grid_move(cols),
            MouseEventKind::ScrollUp => self.grid_move(-cols),
            _ => {}
        }
    }

    /// A single click: put the cursor on what was clicked, or press a button.
    /// Returns whether the click belonged to the grid.
    ///
    /// Held down, Ctrl (or Command — both, because on a Mac with the modifiers
    /// swapped there is no telling which key the user thinks of as Ctrl) adds
    /// to the selection instead of replacing it, which is what every desktop
    /// does. In cian's terms that is a mark, the same one `Space` sets, so a
    /// selection built with the mouse can be operated on with the keyboard.
    pub(crate) fn grid_click_mods(&mut self, col: u16, row: u16, adding: bool) -> bool {
        // Both desktop-shaped views, not just the grid: the detail view has the
        // same address bar, the same buttons and the same sidebar drawn down
        // its left — and none of them answered a click, because this whole
        // function began by asking whether the grid was showing. The places in
        // the sidebar are the reason a sidebar is there.
        if !self.single_pane_view() {
            return false;
        }
        // The address bar, before the buttons: it spans the width, so a click
        // anywhere along it is a click on it. A crumb goes to that ancestor;
        // anywhere else opens the prompt to type a path.
        if self.grid_address.is_some_and(|r| {
            row == r.y && col >= r.x && col < r.x + r.width
        }) {
            let crumb = self
                .grid_crumbs
                .iter()
                .find(|(_, r)| col >= r.x && col < r.x + r.width)
                .map(|(p, _)| p.clone());
            match crumb {
                Some(path) => {
                    if let Some(p) = self.active_pane_mut() {
                        p.marks.clear();
                        let _ = p.jump_to(path);
                    }
                    self.type_ahead.clear();
                }
                None => self.start_jump_path(),
            }
            return true;
        }
        if let Some(b) = self.grid_button_at(col, row) {
            self.grid_button(b);
            return true;
        }
        // A tab label on the top row. The grid answers for the whole window in
        // this view, so the strip it draws would otherwise be a strip that
        // cannot be clicked — while the same labels in the detail view can be.
        if let Some((pane, idx, _)) = self
            .tab_rects
            .iter()
            .copied()
            .find(|(_, _, r)| col >= r.x && col < r.x + r.width && row == r.y)
        {
            self.focus(pane);
            if let Some(t) = self.active_file_tabs_mut() {
                t.select(idx);
            }
            return true;
        }
        // The sidebar is one click to a place, which is the whole reason it
        // is there.
        if col < crate::render::SIDEBAR_W + 1 {
            // "＋ 追加" keeps where you are. The bookmark list `b` opens can do
            // this too, and nobody arriving at a sidebar knows that.
            if self.sidebar_add.is_some_and(|r| row == r.y) {
                self.start_shortcut_add(Vec::new(), false);
                return true;
            }
            if let Some(path) = self.sidebar_at(row) {
                if let Some(p) = self.active_pane_mut() {
                    p.marks.clear();
                    let _ = p.jump_to(path);
                }
                self.type_ahead.clear();
                return true;
            }
        }
        // Below here is the grid's own: tiles, and the empty space between
        // them. The detail view's rows are the listing's to answer.
        if !self.icon_view {
            return false;
        }
        if let Some(i) = self.grid_entry_at(col, row) {
            if let Some(p) = self.active_pane_mut() {
                // Ctrl+click *adds* to a selection, so there has to be one to
                // add to. The file already under the cursor is what the eye
                // says is selected — it is drawn selected — so the first
                // Ctrl+click makes that true rather than starting from nothing
                // and quietly dropping the file the user thought they had.
                if adding && p.marks.is_empty() {
                    let was = p.cursor;
                    if was != i {
                        p.set_mark_at(was);
                    }
                }
                p.cursor = i;
                if adding {
                    p.toggle_mark_at(i);
                }
            }
            // Clicking is pointing, and the next letter typed starts a new
            // search rather than continuing one from before the click.
            self.type_ahead.clear();
            return true;
        }
        // Inside the grid but on nothing. Swallowed, so a stray click on the
        // background does not fall through to the list panes underneath — and
        // it empties the selection, which is what clicking the empty part of a
        // window means everywhere else.
        let inside = self.grid_area.is_some_and(|a| hit_rect(a, col, row));
        if inside {
            if let Some(p) = self.active_pane_mut() {
                p.marks.clear();
            }
            self.type_ahead.clear();
        }
        inside
    }

    /// A toolbar button.
    fn grid_button(&mut self, which: crate::GridButton) {
        use crate::GridButton::*;
        match which {
            Back => {
                self.clear_marks();
                self.pane_go_back();
            }
            Forward => {
                self.clear_marks();
                self.pane_go_forward();
            }
            Up => {
                if let Some(p) = self.active_pane_mut() {
                    p.marks.clear();
                    let _ = p.go_parent();
                }
            }
            // Which view is showing is the front end's business — it owns two
            // of the three — so this only says which was asked for, and the
            // window notices on its next turn.
            View(want) => self.view_request = Some(want),
        }
    }
}

impl App {
    /// A plain click, with nothing held down. Used by the tests, which are
    /// about where a click lands rather than about what is held while it does.
    #[cfg(test)]
    pub(crate) fn grid_click(&mut self, col: u16, row: u16) -> bool {
        self.grid_click_mods(col, row, false)
    }
}

impl App {
    /// Drop the selection. Leaving a directory ends what was chosen in it:
    /// marks name paths, and carrying a set of them somewhere else is how a
    /// delete lands on something nobody was looking at.
    fn clear_marks(&mut self) {
        if let Some(p) = self.active_pane_mut() {
            p.marks.clear();
        }
    }
}

/// Dragging files with the mouse.
///
/// cian has never had this: a terminal cannot report a drag as anything but a
/// stream of motion events, and it cannot be a drag source for the desktop at
/// all. A window can do both, so the pieces live here — what was picked up, and
/// where letting go would put it — with the drawing left to whoever owns the
/// surface and the *doing* left to cian's existing confirmation.
impl App {
}
