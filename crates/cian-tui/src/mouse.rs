//! Mouse handling: routing clicks/drags/scroll to the viewer, AI chat, review
//! popups, the context menu, panes and borders, plus the popup hit-zone and
//! row-cursor helpers. Split out of lib.rs as an `impl App` block.
use super::*;

impl App {
    // ------- Mouse -------
    /// One mouse event, and the same question the keyboard is asked: a click
    /// on a bookmark, a breadcrumb or a folder moves a pane too.
    pub(crate) fn handle_mouse(&mut self, ev: MouseEvent) {
        let before = self.nav_snapshot();
        self.handle_mouse_inner(ev);
        self.note_navigation(before);
    }

    /// Whether the pointer is over the viewer panel's own frame.
    ///
    /// Purely geometric — it does not ask whether the panel is docked, because
    /// one caller wants the answer for a floating one too. Written once because
    /// three places ask it and they have to agree: two carried a copy each and
    /// the third had none, which is how a right-click inside a docked panel
    /// ended up being answered by the pane behind it.
    fn pointer_over_panel(&self, col: u16, row: u16) -> bool {
        let hit = |r: Rect| hit_rect(r, col, row);
        // Split in two, the panel is both halves — `viewer_frame` only
        // describes the one the keyboard is on.
        hit(self.viewer_frame)
            || (self.viewer_split.is_some()
                && (hit(self.viewer_half_rects[0]) || hit(self.viewer_half_rects[1])))
    }

    fn handle_mouse_inner(&mut self, ev: MouseEvent) {
        let (col, row) = (ev.column, ev.row);

        // The view switcher is drawn last and answered first.
        //
        // It is three small rectangles in a corner, and in the classic view
        // they sit *on* the top border row — where a pane, a border to drag and
        // a scrollbar track all also want the click. Hit-testing it before any
        // of them is the same rule the screen already follows: whatever is
        // painted on top is what was clicked. Ordering it anywhere further down
        // meant it worked on one platform and not on another, which is what a
        // rule like this is for.
        if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
            if let Some(crate::GridButton::View(want)) = self.grid_button_at(col, row) {
                self.view_request = Some(want);
                return;
            }
        }

        // The grid covers the window and answers for all of it. First, because
        // everything below tests against rectangles the *list* layout left
        // behind — dividers, scrollbar tracks, tab labels — and those are not
        // erased when the grid takes over. A click on a tile was being eaten by
        // the ghost of a scrollbar.
        // …but not while a popup is open. The grid was answering for the whole
        // window whatever was drawn on top of it, so a click meant to dismiss a
        // menu went to the listing behind the menu instead — the menu stayed,
        // and the click did something else entirely. A docked editor panel is
        // the exception: that one is a surface beside the panes rather than a
        // dialog over them, and the window still belongs to everybody.
        let popup_owns_the_mouse =
            !matches!(self.popup, Popup::None) && self.viewer_dock.is_none();
        // …and the exception cuts both ways. "The window still belongs to
        // everybody" is the half about the panes; the other half is that inside
        // its own frame the panel is what the pointer is over. This asked only
        // the first, so it claimed clicks that landed *on* the panel and
        // returned before the panel's own handling further down ever ran: in
        // the Finder and icon skins a right-click inside the open file gave you
        // the pane's menu — and, because that menu overwrites the popup slot,
        // took the file with it — while the wheel scrolled the grid behind the
        // text the pointer was resting on.
        let on_the_panel = matches!(self.popup, Popup::Viewer { .. })
            && self.viewer_dock.is_some()
            && self.pointer_over_panel(col, row);
        if self.single_pane_view() && !popup_owns_the_mouse && !on_the_panel {
            if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
                // Either modifier means "add to the selection". A terminal
                // never sees Super at all; a window does, and on a Mac whose
                // Control and Command are swapped it is the one under the
                // finger of anyone reaching for Ctrl.
                let adding = ev.modifiers.intersects(
                    crossterm::event::KeyModifiers::CONTROL
                        | crossterm::event::KeyModifiers::SUPER,
                );
                if self.grid_click_mods(col, row, adding) {
                    return;
                }
            }
            // Right-click puts the cursor on what was pointed at and opens
            // cian's own menu — which already carries the OS actions (open,
            // open-with, reveal, properties) alongside cian's file commands.
            // Pointing first is what makes it a menu *about that file*: every
            // desktop moves the selection to whatever was right-clicked.
            // Right-clicking a bookmark opens the list they are managed in —
            // renamed, grouped, reordered, removed. Growing a second and smaller
            // way to do the same would only be a place for the two to disagree.
            if matches!(ev.kind, MouseEventKind::Down(MouseButton::Right))
                && col < crate::render::SIDEBAR_W + 1
                && self.sidebar_at(row).is_some()
            {
                self.start_shortcuts();
                return;
            }
            if matches!(ev.kind, MouseEventKind::Down(MouseButton::Right)) {
                if let Some(i) = self.grid_entry_at(col, row) {
                    if let Some(p) = self.active_pane_mut() {
                        p.cursor = i;
                    }
                    self.type_ahead.clear();
                }
                self.open_context_menu(col, row);
                return;
            }
            // The grid answers the wheel and its own scrollbar; everything else
            // is swallowed rather than let through to a layout that is not on
            // screen. The detail view has a real listing under the pointer, so
            // there everything the chrome did not claim carries on to it.
            if self.icon_view {
                self.grid_scroll_mouse(ev);
                return;
            }
        }

        // A drag in progress owns the mouse until the button comes back up,
        // even if the pointer strays outside the border's grab zone.
        if let Some(d) = self.drag {
            match ev.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.set_divider_ratio(d, d.ratio_at(col, row));
                    return;
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.drag = None;
                    return;
                }
                _ => {}
            }
        }

        // A scrollbar being dragged keeps the pointer until it is let go —
        // the hand wanders off the one-column track constantly, and a bar
        // that only worked while exactly on it would be a bar that does not
        // work.
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
        // A border is a border whatever is drawn beside it: a click on one is
        // a resize, not a click on a pane or on the panel.
        let on_divider = self.dividers.iter().any(|d| {
            let r = d.zone;
            hit_rect(r, col, row)
        });
        // A docked panel only owns the mouse inside its own frame. A click on
        // the listing beside it, or on the shell below, moves the focus there
        // — the panel is one surface among the window's, not a dialog over
        // them.
        if !on_divider && matches!(self.popup, Popup::Viewer { .. }) && self.viewer_dock.is_some() {
            let inside = self.pointer_over_panel(col, row);
            // The panes, for working out which one a click outside the panel
            // landed on. The panel's own frame is `pointer_over_panel`.
            let hit = |r: Rect| hit_rect(r, col, row);
            if matches!(ev.kind, MouseEventKind::Down(_)) {
                let to = if inside {
                    // Clicking the panel focuses it, the same way clicking a
                    // listing focuses that pane. Without this the click was
                    // swallowed by the panel's own handling, which only runs
                    // for the focused pane — so the panel could be clicked
                    // *away from* but never *to*.
                    self.viewer_dock
                } else if hit(self.layout_rects.left) {
                    Some(FocusedPane::Left)
                } else if hit(self.layout_rects.right) {
                    Some(FocusedPane::Right)
                } else if hit(self.layout_rects.shell) {
                    Some(FocusedPane::Shell)
                } else {
                    None
                };
                if let Some(to) = to {
                    if to != self.focused {
                        self.focus(to);
                        // A click on the panel goes on to do what it came for
                        // — place the caret, hit a tab, hit the ✕ — now that
                        // the panel is the focused surface.
                        if !inside {
                            return;
                        }
                    }
                }
            }
        }
        // In the viewer: a click places the cursor on that line, a drag selects
        // whole lines (line-wise visual), the wheel scrolls, and right-click
        // copies. Handled before the blanket popup guard below.
        // …and it only handles the mouse *inside its own frame* when it is
        // docked: outside it, the click belongs to the window — a border to
        // drag, a pane to focus.
        let inside_panel = self.pointer_over_panel(col, row);
        // The seam between two panes runs along the panel's own border, so a
        // click there is a resize even though it is "inside" the frame.
        // The wheel belongs to whatever the pointer is over, never to whatever
        // has the focus: scrolling to read something is not the same as
        // choosing where to type. Without this the panel took every wheel
        // event in the window, so a flick over the listing beside it moved
        // the *file's* cursor.
        let wheel = matches!(
            ev.kind,
            MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight
        );
        if matches!(self.popup, Popup::Viewer { .. })
            && (!wheel || inside_panel)
            && (self.viewer_dock.is_none()
                || (inside_panel && !on_divider && self.viewer_dock == Some(self.focused)))
        {
            // The tab strip lives in the top border. A title starts one column
            // inside the frame and opens with " ◂ ▸ ", which puts the arrows
            // at the third and fifth columns of the box.
            let frame = self.viewer_frame;
            // The ✕ in the corner. Since Esc no longer closes the file, this
            // is the way out that does not have to be known about.
            let x_rect = self.viewer_close_rect;
            if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left))
                && x_rect.width > 0
                && row == x_rect.y
                && col >= x_rect.x
                && col < x_rect.x + x_rect.width
            {
                if matches!(self.popup, Popup::Viewer { dirty: true, .. }) {
                    self.message = Some(
                        tr(
                            self.lang,
                            "unsaved changes — :w to save, :q! to discard",
                            "未保存の変更があります — :w で保存、:q! で破棄",
                        )
                        .into(),
                    );
                } else {
                    self.close_viewer_file();
                }
                return;
            }
            if self.viewer_tab_count() > 1
                && matches!(ev.kind, MouseEventKind::Down(MouseButton::Left))
                && row == frame.y
            {
                if col == frame.x + 2 {
                    self.viewer_switch_tab(false);
                    return;
                }
                if col == frame.x + 4 {
                    self.viewer_switch_tab(true);
                    return;
                }
                // …or the name of the file itself, which is what a tab strip
                // is for.
                if let Some((_, i)) = self
                    .viewer_tab_rects
                    .iter()
                    .copied()
                    .find(|(r, _)| col >= r.x && col < r.x + r.width)
                {
                    self.viewer_goto_tab(i);
                    return;
                }
            }
            // A click in the half that is not in focus crosses to it, rather
            // than moving a cursor in a file the keyboard is not pointed at.
            if self.viewer_split.is_some()
                && matches!(ev.kind, MouseEventKind::Down(MouseButton::Left))
            {
                let theirs = self.viewer_half_rects[1];
                if hit_rect(theirs, col, row)
                {
                    self.swap_viewer_split();
                    self.full_clear = true;
                    return;
                }
            }
            // A click in the outline column jumps to that entry — the reason
            // the column is worth its width.
            let ol = self.outline_rect;
            if ol.width > 0
                && matches!(ev.kind, MouseEventKind::Down(MouseButton::Left))
                && hit_rect(ol, col, row)
            {
                if let Popup::Viewer { shape, line, col: c, goal, visual, md_map, view, .. } = &mut self.popup {
                    let items = shape.as_deref().map(|s| s.items.as_slice()).unwrap_or(&[]);
                    let here = crate::render::src_line(md_map, *line);
                    let top = crate::render::outline_top(items, here, ol.height as usize);
                    let idx = top + (row - ol.y) as usize;
                    if let Some(item) = items.get(idx).cloned() {
                        *line = crate::render::disp_line(md_map, &view.lines, item.line);
                        *c = 0;
                        *goal = 0;
                        *visual = None;
                    }
                }
                return;
            }
            let body = self.viewer_rect;
            let body_h = (body.height as usize).max(1);
            // The clicked column, offset past the line-number gutter, so a click
            // lands on the character under the pointer (not just its line).
            let text_x = body.x + self.viewer_gutter;
            let ecol = col;
            // Closed folds mean screen rows and line numbers are not the same
            // thing: the rows drawn from `scroll` down, resolved once, so a
            // click over folded text lands on what is under the pointer.
            let rows: Vec<usize> = if let Popup::Viewer { view, scroll, shape, preview, .. } = &self.popup {
                let hid = shape
                    .as_deref()
                    .filter(|_| !*preview)
                    .map(|sh| sh.hidden(view.lines.len()))
                    .unwrap_or_default();
                (*scroll..view.lines.len())
                    .filter(|i| hid.is_empty() || !hid[*i])
                    .take(body_h)
                    .collect()
            } else {
                Vec::new()
            };
            let line_at = |row: u16, scroll: usize, n: usize| -> usize {
                let rel = row.saturating_sub(body.y) as usize;
                match rows.get(rel) {
                    Some(l) => *l,
                    None => rows.last().copied().unwrap_or((scroll + rel).min(n.saturating_sub(1))),
                }
            };
            // A click on the fold marker in the gutter opens or closes it,
            // rather than moving the cursor there — the marker is drawn to be
            // clicked, and a cursor move is what the text itself is for.
            // The gutter is [line number][fold marker][git change bar], so the
            // marker is two columns left of where the text starts.
            if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left))
                && self.viewer_gutter > 1
                && col + 2 == text_x
                && row >= body.y
                && ((row - body.y) as usize) < rows.len()
            {
                let l = line_at(row, 0, 0);
                self.toggle_viewer_fold(Some(l));
                return;
            }
            // A clicked column is not a character index: a tab is one buffer
            // character but several drawn columns, and a Japanese character is
            // one buffer character but two. Both have to be walked back
            // through the same widths the renderer used — counting every
            // character as one column put the cursor a character further left
            // for every wide one before it, which is most of a line of
            // Japanese.
            let hscroll = match &self.popup {
                Popup::Viewer { hscroll, .. } => *hscroll,
                _ => 0,
            };
            let col_at = |view: &cian_core::viewer::View, l: usize| -> usize {
                // The clicked cell is relative to the body; the line may have
                // been scrolled sideways underneath it.
                let rel = ecol.saturating_sub(text_x) as usize + hscroll;
                let Some(text) = view.lines.get(l) else { return 0 };
                let mut drawn = 0usize;
                for (j, ch) in text.chars().enumerate() {
                    let w = cian_core::textops::char_cols(ch, drawn);
                    if rel < drawn + w {
                        return j;
                    }
                    drawn += w;
                }
                vlen(view, l)
            };
            match ev.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Popup::Viewer { view, scroll, line, col, goal, visual, anchor, .. } =
                        &mut self.popup
                    {
                        let l = line_at(row, *scroll, view.lines.len());
                        let c = col_at(view, l);
                        *line = l;
                        *col = c;
                        *goal = c;
                        *anchor = (l, c);
                        *visual = None; // a bare click just moves the cursor
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    // Holding Alt while dragging makes a block (rectangular)
                    // selection; otherwise it is character-wise.
                    let mode = if ev.modifiers.contains(KeyModifiers::ALT) {
                        ViewVisual::Block
                    } else {
                        ViewVisual::Char
                    };
                    if let Popup::Viewer { view, scroll, line, col, goal, visual, .. } = &mut self.popup {
                        let l = line_at(row, *scroll, view.lines.len());
                        let c = col_at(view, l);
                        *line = l;
                        *col = c;
                        *goal = c;
                        *visual = Some(mode);
                    }
                }
                // The wheel moves the *view*, and takes the cursor only when
                // it would otherwise be left off screen — which is what a
                // wheel does everywhere else. It used to move the cursor and
                // let the view follow, so a flick past the end of the file
                // moved the insertion point without being asked to.
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                    if let Popup::Viewer { view, scroll, line, col, goal, .. } = &mut self.popup {
                        let n = view.lines.len();
                        let max_scroll = n.saturating_sub(body_h);
                        *scroll = if matches!(ev.kind, MouseEventKind::ScrollDown) {
                            (*scroll + 3).min(max_scroll)
                        } else {
                            scroll.saturating_sub(3)
                        };
                        *line = (*line).clamp(*scroll, (*scroll + body_h - 1).min(n.saturating_sub(1)));
                        *col = (*goal).min(vlen(view, *line));
                    }
                }
                // Sideways, for the terminals that report it — a shift-wheel
                // or a trackpad's second axis. The same three columns, and
                // the cursor comes along only if the view would leave it
                // behind.
                MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                    let body_w = (self.viewer_rect.width as usize)
                        .saturating_sub(self.viewer_gutter as usize)
                        .max(1);
                    if let Popup::Viewer { view, hscroll, scroll, line, col, goal, .. } =
                        &mut self.popup
                    {
                        // Bounded by the widest line *in view*, which is what
                        // the bar along the bottom describes. Bounding it by
                        // the cursor's own line would make the wheel dead
                        // whenever the cursor happened to be on a short one.
                        let widest = view
                            .lines
                            .iter()
                            .skip(*scroll)
                            .take(body_h)
                            .map(|l| cian_core::textops::col_span(l, usize::MAX).1)
                            .max()
                            .unwrap_or(0);
                        *hscroll = if matches!(ev.kind, MouseEventKind::ScrollRight) {
                            (*hscroll + 3).min(widest.saturating_sub(body_w))
                        } else {
                            hscroll.saturating_sub(3)
                        };
                        // Back to a character index, so the cursor lands on
                        // the character at that column rather than near it.
                        if let Some(l) = view.lines.get(*line) {
                            let (at, _) = cian_core::textops::col_span(l, *col);
                            if at < *hscroll || at >= *hscroll + body_w {
                                let want = *hscroll;
                                let mut c = 0usize;
                                let mut drawn = 0usize;
                                for (j, ch) in l.chars().enumerate() {
                                    if drawn >= want {
                                        c = j;
                                        break;
                                    }
                                    drawn += cian_core::textops::char_cols(ch, drawn);
                                    c = j;
                                }
                                *col = c;
                                *goal = c;
                            }
                        }
                    }
                }
                // Right-click opens the menu — the same gesture as in the
                // file panes. Copying moved into it, where it can be seen.
                MouseEventKind::Down(MouseButton::Right) => self.open_viewer_menu(col, row),
                _ => {}
            }
            return;
        }

        // In the AI chat, drag selects transcript lines and copies on release;
        // the wheel scrolls; right-click copies. Same feel as the viewer.
        if matches!(self.popup, Popup::AiChat { .. }) {
            // Off the chat altogether: closed, like any other popup. The
            // conversation is kept — closing a chat archives it — so this
            // loses nothing but the window.
            if self.click_dismissed_popup(ev) {
                return;
            }
            let body = self.ai_rect;
            let n = self.ai_lines.len();
            let scroll = self.ai_scroll;
            let line_at = |row: u16| -> usize {
                let rel = row.saturating_sub(body.y) as usize;
                (scroll + rel).min(n.saturating_sub(1))
            };
            let in_body = hit_rect(body, col, row);
            match ev.kind {
                MouseEventKind::Down(MouseButton::Left) if in_body && n > 0 => {
                    let l = line_at(row);
                    if let Popup::AiChat { sel, .. } = &mut self.popup {
                        *sel = Some((l, l));
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) if n > 0 => {
                    let l = line_at(row);
                    if let Popup::AiChat { sel: Some(s), .. } = &mut self.popup {
                        s.1 = l;
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    // A drag that actually spanned lines copies; a bare click clears.
                    let dragged = matches!(self.popup, Popup::AiChat { sel: Some((a, b)), .. } if a != b);
                    if dragged {
                        self.copy_ai_text();
                    } else if let Popup::AiChat { sel, .. } = &mut self.popup {
                        *sel = None;
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Popup::AiChat { scroll, .. } = &mut self.popup {
                        *scroll = scroll.saturating_add(3);
                    }
                }
                MouseEventKind::ScrollUp => {
                    if let Popup::AiChat { scroll, .. } = &mut self.popup {
                        *scroll = scroll.saturating_sub(3);
                    }
                }
                MouseEventKind::Down(MouseButton::Right) => self.copy_ai_text(),
                _ => {}
            }
            return;
        }

        // The four review lists all feel the same under the mouse: a click
        // toggles the row's checkbox and moves the cursor to it, the wheel
        // scrolls, and a click off the list closes it without carrying
        // anything out. Approval stays on Enter and the button.
        if matches!(self.popup, Popup::JunkReview { .. }) {
            if self.click_dismissed_popup(ev) {
                return;
            }
            let body = self.junk_rect;
            if let Popup::JunkReview { items, cursor, scroll } = &mut self.popup {
                review_list_mouse(items, cursor, scroll, body, ev);
            }
            return;
        }
        if matches!(self.popup, Popup::DupeReview { .. }) {
            if self.click_dismissed_popup(ev) {
                return;
            }
            let body = self.dupe_rect;
            if let Popup::DupeReview { items, cursor, scroll } = &mut self.popup {
                review_list_mouse(items, cursor, scroll, body, ev);
            }
            return;
        }
        if matches!(self.popup, Popup::StructureReview { .. }) {
            if self.click_dismissed_popup(ev) {
                return;
            }
            let body = self.struct_rect;
            if let Popup::StructureReview { items, cursor, scroll, .. } = &mut self.popup {
                review_list_mouse(items, cursor, scroll, body, ev);
            }
            return;
        }
        if matches!(self.popup, Popup::RenameReview { .. }) {
            if self.click_dismissed_popup(ev) {
                return;
            }
            let body = self.rename_rect;
            if let Popup::RenameReview { items, cursor, scroll, .. } = &mut self.popup {
                review_list_mouse(items, cursor, scroll, body, ev);
            }
            return;
        }

        // The context menu is mouse-navigable: hovering a row highlights it,
        // clicking it runs it. Handled before the blanket popup guard below.
        if matches!(self.popup, Popup::ContextMenu { .. }) {
            let m = self.menu_rect;
            let top = m.y + 1; // first row inside the border
            let in_cols = col >= m.x && col < m.x + m.width;
            if let Popup::ContextMenu { items, cursor, .. } = &mut self.popup {
                let n = items.len();
                let idx = row.saturating_sub(top) as usize;
                let on_row = in_cols && row >= top && idx < n;
                match ev.kind {
                    MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
                        if on_row {
                            *cursor = idx;
                        }
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        if on_row {
                            let item = items[idx];
                            let _ = self.run_menu_item(item);
                        } else {
                            // A click off the menu dismisses it entirely, as
                            // menus do — including any parent levels.
                            self.menu_stack.clear();
                            self.popup = Popup::None;
                        }
                    }
                    MouseEventKind::Down(MouseButton::Right) => {
                        // Right-click inside a submenu backs out one level.
                        self.menu_back();
                    }
                    _ => {}
                }
            }
            return;
        }

        // Every other popup — confirm dialogs and list pickers — is driven
        // through the hit zones the renderer registered, so it is fully
        // clickable. The wheel scrolls whatever is on screen.
        // A border drag belongs to the window, not to whatever is drawn in
        // it. With the panel docked in a pane, dragging the seam between the
        // panes — or the one above the shell — resizes them as ever.
        let panel_docked = matches!(self.popup, Popup::Viewer { .. }) && self.viewer_dock.is_some();
        let border_gesture = panel_docked
            && (self.drag.is_some()
                || (on_divider && matches!(ev.kind, MouseEventKind::Down(MouseButton::Left))));
        // A docked panel is one surface among the window's, not a dialog over
        // it: outside its own frame the mouse belongs to whatever is there.
        // This guard was swallowing every event that reached it — so with the
        // panel open and the focus on the listing beside it, the listing
        // could be neither clicked nor scrolled.
        let outside_panel = panel_docked && !inside_panel;
        if !matches!(self.popup, Popup::None) && !border_gesture && !outside_panel {
            if self.click_dismissed_popup(ev) {
                return;
            }
            let _ = self.handle_popup_mouse(ev);
            return;
        }

        let in_rect = |r: Rect| hit_rect(r, col, row);

        // Right-click focuses what was clicked, puts the cursor on the row
        // under the pointer, and opens the context menu there.
        if matches!(ev.kind, MouseEventKind::Down(MouseButton::Right)) {
            let target = if in_rect(self.layout_rects.left) {
                Some(FocusedPane::Left)
            } else if in_rect(self.layout_rects.right) {
                Some(FocusedPane::Right)
            } else if in_rect(self.layout_rects.shell) {
                Some(FocusedPane::Shell)
            } else {
                None
            };
            if let Some(t) = target {
                if self.focused != t {
                    self.focus(t);
                }
                match t {
                    // Act on the split pane under the pointer, not whichever
                    // happened to be active — otherwise a right-click on the
                    // left half colours the right one.
                    FocusedPane::Shell => self.select_shell_leaf_at(col, row),
                    _ => self.cursor_to_row(t, row),
                }
                self.open_context_menu(col, row);
            }
            return;
        }

        // Copied out, so the closure below borrows nothing of `self` — the
        // wheel needs to reach for the pane mutably right after asking which
        // pane it is over.
        let rects = self.layout_rects;
        let pane_at = move |col: u16, row: u16| -> Option<FocusedPane> {
            let hit = |r: Rect| hit_rect(r, col, row);
            if hit(rects.left) {
                Some(FocusedPane::Left)
            } else if hit(rects.right) {
                Some(FocusedPane::Right)
            } else if hit(rects.shell) {
                Some(FocusedPane::Shell)
            } else {
                None
            }
        };

        // A file drag in progress owns the mouse until release.
        if self.file_drag.is_some() {
            match ev.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    let over = pane_at(col, row);
                    let (from, anchor) =
                        self.file_drag.as_ref().map(|d| (d.from, d.anchor)).unwrap();
                    if let Some(d) = &mut self.file_drag {
                        d.moved = true;
                        d.over = over;
                    }
                    // Dragging within the origin pane just moves the cursor.
                    // It used to rubber-band-select rows, which fought the
                    // deliberate marking `Space` and visual mode already do,
                    // and made every slightly-shaky click reshuffle the marks.
                    let _ = anchor;
                    if over == Some(from) && from != FocusedPane::Shell {
                        self.cursor_to_row(from, row);
                    }
                    return;
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    let over = pane_at(col, row);
                    self.finish_file_drag(over, ev.modifiers);
                    return;
                }
                _ => {}
            }
        }

        // The mouse wheel scrolls whatever is under the pointer: a listing's
        // cursor, or the shell's output back through its scrollback.
        if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
            let up = matches!(ev.kind, MouseEventKind::ScrollUp);
            match pane_at(col, row) {
                Some(pane @ (FocusedPane::Left | FocusedPane::Right)) => {
                    self.focus(pane);
                    if let Some(p) = self.active_pane_mut() {
                        p.move_cursor(if up { -3 } else { 3 });
                    }
                }
                Some(FocusedPane::Shell) => {
                    // The pane under the pointer, not the active one — the
                    // shell can be split, and the wheel belongs to what it is
                    // over. Focus is left alone: scrolling to read something
                    // is not the same as choosing where to type.
                    self.select_shell_leaf_at(col, row);
                    if let Some(s) = self.shell.active_session() {
                        s.scroll_back(if up { 3 } else { -3 });
                    }
                }
                None => {}
            }
            return;
        }

        // A shell-pane selection in progress: extend on drag, copy on release.
        if self.shell_sel.is_some() {
            match ev.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    if let Some(sel) = &mut self.shell_sel {
                        sel.end = grid_pos(sel.inner, col, row);
                        sel.dragged = true;
                    }
                    return;
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    let dragged = self.shell_sel.map(|s| s.dragged).unwrap_or(false);
                    if dragged {
                        self.copy_shell_selection(); // copy-on-select; keep the highlight
                    } else {
                        self.shell_sel = None; // a bare click, not a selection
                    }
                    return;
                }
                _ => {}
            }
        }

        if !matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }

        // The ◀ / ▶ arrows at the head of the title, before anything else on
        // that row can claim the click.
        if let Some((pane, fwd, _)) = self.nav_rects.iter().copied().find(|(_, _, r)| in_rect(*r)) {
            self.focus(pane);
            if fwd {
                self.pane_go_forward();
            } else {
                self.pane_go_back();
            }
            return;
        }
        // A breadcrumb click navigates to that ancestor of the pane's cwd.
        // Checked before tab selection: these rects sit inside the active
        // tab's label, and the tab click would otherwise swallow them.
        if let Some((pane, strip, _)) =
            self.crumb_rects.iter().copied().find(|(_, _, r)| in_rect(*r))
        {
            self.focus(pane);
            let target = self.active_pane().map(|p| {
                let mut t = p.cwd.clone();
                for _ in 0..strip {
                    if let Some(parent) = t.parent() {
                        t = parent.to_path_buf();
                    }
                }
                t
            });
            if let (Some(t), Some(p)) = (target, self.active_pane_mut()) {
                if t != p.cwd {
                    let _ = p.jump_to(t);
                }
            }
            return;
        }
        // A column-header click sorts by that column (repeat = flip).
        if let Some((pane, key, _)) =
            self.sort_rects.iter().copied().find(|(_, _, r)| in_rect(*r))
        {
            self.focus(pane);
            self.apply_sort_key(key);
            return;
        }
        // Clicking a tab label switches to that tab. Checked before the border
        // drag, because the shell's tab bar sits on the files|shell seam row —
        // divider-first would swallow every shell-tab click as a drag.
        if let Some((pane, idx, _)) = self.tab_rects.iter().copied().find(|(_, _, r)| in_rect(*r)) {
            self.focus(pane);
            match pane {
                FocusedPane::Shell => self.shell.select(idx),
                _ => {
                    if let Some(t) = self.active_file_tabs_mut() {
                        t.select(idx);
                    }
                }
            }
            return;
        }

        // Grabbing a border (away from any tab label) starts a resize.
        if let Some(d) = self.dividers.iter().copied().find(|d| in_rect(d.zone)) {
            self.drag = Some(d);
            return;
        }

        match pane_at(col, row) {
            Some(FocusedPane::Shell) => {
                self.focus(FocusedPane::Shell);
                // Clicking a split should focus that split, as in any multiplexer.
                self.select_shell_leaf_at(col, row);
                // Begin a text selection anchored here, if the click landed on a
                // pane's terminal area. A plain drag then selects (no Shift), and
                // release copies.
                self.shell_sel = self
                    .shell_leaves
                    .iter()
                    .copied()
                    .find(|(_, _, _, inner)| hit_rect(*inner, col, row))
                    .map(|(tab, leaf, _, inner)| {
                        let a = grid_pos(inner, col, row);
                        ShellSel { tab, leaf, inner, anchor: a, end: a, dragged: false }
                    });
            }
            Some(pane) => {
                self.focus(pane);
                // Put the cursor on the row that was clicked.
                self.cursor_to_row(pane, row);
                // A second click on the same row in quick succession is a
                // double-click: enter a directory, or open a file with its OS
                // default program — the same as Enter / the open key.
                let now = Instant::now();
                let is_double = self
                    .last_click
                    .map(|(t, r)| r == row && now.duration_since(t) < DOUBLE_CLICK)
                    .unwrap_or(false);
                if is_double {
                    self.last_click = None;
                    let _ = self.activate_selected();
                    return;
                }
                self.last_click = Some((now, row));
                // Otherwise arm a drag from here; whether it becomes a drag or
                // stays a click is decided on release. The cursor was just put
                // on the clicked row, so that is the selection anchor.
                let anchor = self.active_pane().map(|p| p.cursor).unwrap_or(0);
                let paths = self.target_paths();
                if !paths.is_empty() {
                    self.file_drag = Some(FileDrag {
                        from: pane,
                        paths,
                        over: Some(pane),
                        moved: false,
                        anchor,
                    });
                }
            }
            None => {}
        }
    }

    /// The zone under the pointer, if any. Later zones win, so a small button
    /// drawn on top of a wider row is reachable.
    pub(crate) fn zone_at(&self, col: u16, row: u16) -> Option<ZoneKind> {
        self.popup_zones
            .iter()
            .rev()
            .find(|z| {
                let r = z.rect;
                hit_rect(r, col, row)
            })
            .map(|z| z.kind)
    }

    /// Point the active popup's list cursor at `i`. A no-op for popups that have
    /// no cursor (confirm dialogs, notices).
    pub(crate) fn set_popup_cursor(&mut self, i: usize) {
        match &mut self.popup {
            Popup::GrepReplace(plan) => plan.cursor = i,
            Popup::ContextMenu { cursor, .. }
            | Popup::ColorPicker { cursor, .. }
            | Popup::SortPicker { cursor, .. }
            | Popup::Macros { cursor, .. }
            | Popup::GitLog { cursor, .. }
            | Popup::EncodingPicker { cursor, .. }
            | Popup::DirCompare { cursor, .. }
            | Popup::Archive { cursor, .. }
            | Popup::DiskUsage { cursor, .. }
            | Popup::Palette { cursor, .. }
            | Popup::DestPicker { cursor, .. }
            | Popup::FindResults { cursor, .. }
            | Popup::SshHosts { cursor, .. }
            | Popup::SshUsers { cursor, .. }
            | Popup::Snippets { cursor, .. }
            | Popup::RemoteBrowser { cursor, .. }
            | Popup::LocalDest { cursor, .. }
            | Popup::History { cursor, .. }
            | Popup::Shortcuts { cursor, .. } => *cursor = i,
            _ => {}
        }
    }

    /// Drive the on-screen popup with the mouse: the wheel scrolls, a click on a
    /// registered zone replays the keystroke it stands for so all the existing
    /// popup key handling does the real work.
    /// A click clean outside the popup closes it. Returns whether it did.
    ///
    /// What every menu on every desktop does, and what cian's own context menu
    /// already did — but only that one, and only in the classic view, so in the
    /// desktop views a popup could be clicked *past* and stayed open behind
    /// whatever the click had done instead.
    ///
    /// It closes and does nothing else: the click is spent on the dismissal
    /// rather than passed through to the listing underneath. A dialog asking
    /// whether to overwrite must never be answered by a click aimed somewhere
    /// else — which is why this sends Esc, the answer that is always "no".
    ///
    /// The editor panel is not one of these. It is a place to be, not a
    /// question to answer, and it has a ✕ and `:q` for leaving.
    fn click_dismissed_popup(&mut self, ev: MouseEvent) -> bool {
        if !matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
            return false;
        }
        if matches!(self.popup, Popup::None | Popup::Viewer { .. }) {
            return false;
        }
        // Where the popup is, as the frame that drew it left it. No frame, no
        // opinion — better to leave the popup alone than to guess.
        let Some(ink) = crate::render::popup_ink() else { return false };
        let (col, row) = (ev.column, ev.row);
        let inside = hit_rect(ink, col, row);
        if inside {
            return false;
        }
        // The menu keeps a stack of the levels it was opened through; a click
        // outside dismisses all of them, not one.
        self.menu_stack.clear();
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let _ = self.handle_popup_key(esc);
        true
    }

    pub(crate) fn handle_popup_mouse(&mut self, ev: MouseEvent) -> Result<()> {
        let (col, row) = (ev.column, ev.row);
        let synth = |code| KeyEvent::new(code, KeyModifiers::NONE);
        match ev.kind {
            // The wheel moves the cursor / scroll of whatever is open; every
            // list and scroll popup accepts Down/Up.
            MouseEventKind::ScrollDown => return self.handle_popup_key(synth(KeyCode::Down)),
            MouseEventKind::ScrollUp => return self.handle_popup_key(synth(KeyCode::Up)),
            // Hovering (or dragging over) a row highlights it, as the menu does.
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(ZoneKind::SelectRow(i)) = self.zone_at(col, row) {
                    self.set_popup_cursor(i);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => match self.zone_at(col, row) {
                Some(ZoneKind::SelectRow(i)) => {
                    self.set_popup_cursor(i);
                    return self.handle_popup_key(synth(KeyCode::Enter));
                }
                Some(ZoneKind::Char(c)) => return self.handle_popup_key(synth(KeyCode::Char(c))),
                Some(ZoneKind::Enter) => return self.handle_popup_key(synth(KeyCode::Enter)),
                Some(ZoneKind::Esc) => return self.handle_popup_key(synth(KeyCode::Esc)),
                // A click in dead space inside the popup does nothing; a click
                // right outside it is ignored too, so a mis-aimed click never
                // silently confirms a destructive dialog.
                None => {}
            },
            _ => {}
        }
        Ok(())
    }

    /// Act on the selected entry as Enter would: enter a directory, or read a
    /// file in the viewer. Inside an archive the rows are members, and Enter
    /// navigates or views them; on an archive file, Enter goes in.
    ///
    /// Enter reads rather than launching. Looking at a file is what one does
    /// with it a hundred times a day and handing it to another program is what
    /// one does occasionally — and the viewer can be left with Esc, while an
    /// application that opens by accident has to be found and closed.
    /// Ctrl+Enter is the launch, and `x` where a terminal keeps Ctrl.
    pub(crate) fn activate_selected(&mut self) -> Result<()> {
        if self.in_archive() {
            self.archive_activate();
            return Ok(());
        }
        // A remote pane navigates over the network. Without this, a double
        // click on a remote directory fell through to `Pane::enter_selected`,
        // which reads the row's path *on this disk* — and a server's
        // `/var/log` is not a directory here, so the click did nothing.
        if self.active_pane().map(|p| p.is_remote()).unwrap_or(false) {
            self.remote_pane_enter();
            return Ok(());
        }
        let sel = self.active_pane().and_then(|p| p.selected()).map(|e| (e.is_dir, e.path.clone()));
        match sel {
            Some((true, _)) => {
                if let Some(p) = self.active_pane_mut() {
                    p.enter_selected()?;
                }
            }
            Some((false, path)) if cian_core::archive::is_archive(&path) => {
                self.enter_archive(path, String::new());
            }
            // Enter reads it *here*: the same viewer, docked in the pane
            // whose listing it replaces, with everything it can do. F3 and
            // Shift+Tab open the same file over the whole window instead.
            Some((false, _)) => {
                let here = self.focused;
                self.look_inside();
                if matches!(self.popup, Popup::Viewer { .. }) {
                    self.viewer_dock = Some(here);
                    self.full_clear = true;
                }
            }
            None => {}
        }
        Ok(())
    }

    /// Resolve a finished drag.
    ///
    /// Dropping onto the other file pane transfers; onto the shell it types
    /// the paths, which is the closest thing to dragging a file into a
    /// terminal. Anything else — including a press and release in place, which
    /// is just a click — does nothing.
    pub(crate) fn finish_file_drag(&mut self, over: Option<FocusedPane>, mods: KeyModifiers) {
        let Some(drag) = self.file_drag.take() else { return };
        if !drag.moved {
            return;
        }
        let Some(target) = over else { return };
        if target == drag.from {
            return;
        }
        match target {
            FocusedPane::Shell => {
                let quoted: Vec<String> = drag
                    .paths
                    .iter()
                    .map(|p| {
                        let s = p.display().to_string();
                        // Quote only when needed, so the common case stays
                        // something you would have typed yourself.
                        if s.contains(' ') { format!("\"{}\"", s) } else { s }
                    })
                    .collect();
                let text = quoted.join(" ");
                self.focus(FocusedPane::Shell);
                let cwd = self.shell_cwd();
                self.shell.ensure(&cwd);
                match self.shell.active_session_mut() {
                    Some(s) => s.write_input(text.as_bytes()),
                    None => self.pending_shell_input = Some(text),
                }
                self.message = Some(format!("{} path(s) → shell", drag.paths.len()));
            }
            dest_pane => {
                let dest = match dest_pane {
                    FocusedPane::Left => self.left.active_ref().cwd.clone(),
                    FocusedPane::Right => self.right.active_ref().cwd.clone(),
                    FocusedPane::Shell => return,
                };
                // Shift means move, matching what every other file manager
                // does with a modifier on a drag.
                let op = if mods.contains(KeyModifiers::SHIFT) {
                    PendingOp::Move
                } else {
                    PendingOp::Copy
                };
                self.open_popup(Popup::ConfirmTransfer { op, targets: drag.paths, dest });
            }
        }
    }

    // ------- Transitions -------

    pub(crate) fn anim_enabled(&self) -> bool {
        !self.anim_dur.is_zero()
    }

    /// Toggle full-window zoom of the focused surface, animating between the
    /// surface's pane rect and the whole layout area.
    pub(crate) fn toggle_zoom(&mut self) {
        // The full area is the union of everything currently laid out; derived
        // rather than stored so it stays right at any window size. While
        // zoomed this is just the focused surface, which already fills it.
        let full = union_rect(
            union_rect(self.layout_rects.left, self.layout_rects.right),
            self.layout_rects.shell,
        );
        if self.zoomed {
            // Shrink back into where the surface came from. Taken from
            // `zoom_return` because `layout_rects` now describes the zoomed
            // layout; reading the focused pane's rect here would give the full
            // area again, making the transition a no-op.
            let back = self.zoom_return.take();
            self.zoomed = false;
            if let Some(back) = back {
                // A resize while zoomed can leave the remembered rect outside
                // the window; snapping is better than flying in from nowhere.
                let fits = back.x + back.width <= full.x + full.width
                    && back.y + back.height <= full.y + full.height;
                if fits && back.width > 0 && full.width > 0 {
                    self.start_anim(AnimKind::Zoom { from: full, to: back });
                }
            }
        } else {
            let pane_rect = match self.focused {
                FocusedPane::Left => self.layout_rects.left,
                FocusedPane::Right => self.layout_rects.right,
                FocusedPane::Shell => self.layout_rects.shell,
            };
            self.zoomed = true;
            self.zoom_return = Some(pane_rect);
            if pane_rect.width > 0 && full.width > 0 {
                self.start_anim(AnimKind::Zoom { from: pane_rect, to: full });
            }
        }
    }

    /// Maximize the active shell pane, or restore it, animating the pane out of
    /// (or back into) its slot the way full-window zoom animates.
    pub(crate) fn toggle_pane_zoom_animated(&mut self) {
        // The shell panel's inner area (inside its border).
        let s = self.layout_rects.shell;
        let full = Rect::new(
            s.x.saturating_add(1),
            s.y.saturating_add(1),
            s.width.saturating_sub(2),
            s.height.saturating_sub(2),
        );
        if self.shell.zoom_pane {
            // Restoring: shrink back into the slot stashed on the way in.
            let back = self.pane_zoom_return.take();
            self.shell.zoom_pane = false;
            if let Some(back) = back {
                if back != full && back.width > 0 {
                    self.start_anim(AnimKind::PaneZoom { from: full, to: back });
                }
            }
        } else {
            // Maximizing: grow from the active pane's current slot.
            let slot = self.active_shell_leaf_rect();
            self.shell.zoom_pane = true;
            if let Some(slot) = slot {
                self.pane_zoom_return = Some(slot);
                if slot != full && slot.width > 0 {
                    self.start_anim(AnimKind::PaneZoom { from: slot, to: full });
                }
            }
        }
    }

    /// The on-screen rect of the active shell split pane, from the last frame's
    /// captured leaf rects.
    pub(crate) fn active_shell_leaf_rect(&self) -> Option<Rect> {
        let tab = self.shell.active;
        let leaf = self.shell.tabs.get(tab).map(|t| t.active)?;
        self.shell_leaves.iter().find(|(t, l, _, _)| *t == tab && *l == leaf).map(|(_, _, r, _)| *r)
    }

    pub(crate) fn start_anim(&mut self, kind: AnimKind) {
        if !self.anim_enabled() {
            return;
        }
        self.anim = Some(Anim { kind, start: Instant::now(), dur: self.anim_dur });
    }

    /// What the renderer should override this frame.
    pub(crate) fn anim_override(&self) -> AnimOverride {
        match self.anim {
            Some(a) => match a.kind {
                AnimKind::Ratio { target, from, to } => {
                    let t = a.progress();
                    let r = (from as f32 + (to as f32 - from as f32) * t).round() as u16;
                    AnimOverride { ratio: Some((target, r)), freeze_pty: true, show_splits: false }
                }
                AnimKind::Zoom { .. } => {
                    AnimOverride { ratio: None, freeze_pty: true, show_splits: false }
                }
                AnimKind::PaneZoom { .. } => {
                    AnimOverride { ratio: None, freeze_pty: true, show_splits: true }
                }
            },
            None => AnimOverride::default(),
        }
    }

    /// Land the current transition now, applying any deferred work. Called
    /// when the timer expires and whenever the user presses a key, so input is
    /// never held up waiting for an animation.
    pub(crate) fn finish_anim(&mut self) {
        if self.anim.take().is_none() {
            return;
        }
        if let Some(close) = self.anim_then.take() {
            self.apply_pending_close(close);
        }
    }

    /// Perform a close that was deferred until its shrink animation finished.
    pub(crate) fn apply_pending_close(&mut self, close: PendingClose) {
        match close {
            PendingClose::ShellPane => {
                let empty = self.shell.close_active_pane();
                if empty {
                    let back = self.last_file_pane;
                    self.focus(back);
                }
            }
        }
    }

    /// Begin closing the active shell split pane, shrinking it away first.
    /// Falls back to closing immediately when animation is off or the pane is
    /// the only one in its tab (nothing to shrink into).
    pub(crate) fn close_shell_pane_animated(&mut self) {
        let parent = self
            .shell
            .tabs
            .get(self.shell.active)
            .and_then(|t| t.parent_of(t.active));
        match (self.anim_enabled(), parent) {
            (true, Some((p, is_first))) => {
                let stored = match self
                    .shell
                    .tabs
                    .get(self.shell.active)
                    .and_then(|t| t.nodes.get(p))
                    .and_then(|n| n.as_ref())
                {
                    Some(Node::Split { ratio, .. }) => *ratio,
                    _ => 50,
                };
                // Drive the closing child's share to nothing.
                let to = if is_first { 0 } else { 100 };
                self.anim_then = Some(PendingClose::ShellPane);
                self.start_anim(AnimKind::Ratio {
                    target: DividerTarget::ShellSplit { tab: self.shell.active, node: p },
                    from: stored,
                    to,
                });
            }
            _ => self.apply_pending_close(PendingClose::ShellPane),
        }
    }

    /// Briefly highlight `pane` to show an operation landed there.
    pub(crate) fn flash(&mut self, pane: FocusedPane) {
        self.flash = Some((pane, Instant::now()));
    }

    /// How lit `pane` currently is, 1.0 right after a flash fading to 0.0.
    /// Returns 0.0 once the flash has expired.
    pub(crate) fn flash_level(&self, pane: FocusedPane) -> f32 {
        let Some((p, at)) = self.flash else { return 0.0 };
        if p != pane {
            return 0.0;
        }
        let e = at.elapsed().as_secs_f32();
        if e >= FLASH_SECS {
            0.0
        } else {
            1.0 - e / FLASH_SECS
        }
    }

    /// Whether a flash is still running and the UI should keep repainting.
    pub(crate) fn flash_active(&self) -> bool {
        self.flash.map(|(_, at)| at.elapsed().as_secs_f32() < FLASH_SECS).unwrap_or(false)
    }

    /// Which `pane_bg` slot a file pane uses.
    pub(crate) fn bg_slot(pane: FocusedPane) -> Option<usize> {
        match pane {
            FocusedPane::Left => Some(0),
            FocusedPane::Right => Some(1),
            // The shell's background lives on the split pane itself.
            FocusedPane::Shell => None,
        }
    }

    /// Make the shell split pane under the pointer the active one.
    ///
    /// Without this, clicking a pane focuses the panel but leaves the previous
    /// pane active, so anything acting on "the active pane" targets the wrong
    /// half of a split.
    /// Copy the current shell selection's text to the clipboard, reading it
    /// from the pane's terminal grid.
    /// Put the surface a scrollbar belongs to at the point on it that was
    /// grabbed. The thumb's own height is taken off the track first, so
    /// dragging it to the bottom lands on the last page rather than a page
    /// past it.
    pub(crate) fn scroll_to_fraction(&mut self, what: crate::ScrollWhat, col: u16, row: u16) {
        let Some(t) = self.scroll_tracks.iter().copied().find(|t| t.what == what) else { return };
        let vertical = t.rect.height > 1;
        let (at, span) = if vertical {
            (row.saturating_sub(t.rect.y) as usize, t.rect.height as usize)
        } else {
            (col.saturating_sub(t.rect.x) as usize, t.rect.width as usize)
        };
        let max = t.total.saturating_sub(t.shown);
        let pos = if span <= 1 { 0 } else { at.min(span - 1) * max / (span - 1) };
        match what {
            crate::ScrollWhat::Pane(id) => {
                self.focus(id);
                if let Some(p) = self.active_pane_mut() {
                    p.scroll = pos.min(max);
                    // The cursor comes with it: a listing whose cursor is off
                    // screen answers the next keypress somewhere invisible.
                    p.cursor = p.cursor.clamp(p.scroll, (p.scroll + t.shown).saturating_sub(1));
                    p.cursor = p.cursor.min(p.entries.len().saturating_sub(1));
                }
            }
            crate::ScrollWhat::ViewerRows => {
                if let Popup::Viewer { scroll, line, .. } = &mut self.popup {
                    *scroll = pos;
                    *line = (*line).clamp(*scroll, (*scroll + t.shown).saturating_sub(1));
                }
                self.clamp_viewer_hscroll();
            }
            crate::ScrollWhat::ViewerCols => {
                if let Popup::Viewer { hscroll, .. } = &mut self.popup {
                    *hscroll = pos;
                }
            }
        }
    }

    pub(crate) fn copy_shell_selection(&mut self) {
        let Some(sel) = self.shell_sel else { return };
        let Some(session) = self
            .shell
            .tabs
            .get(sel.tab)
            .and_then(|t| t.nodes.get(sel.leaf))
            .and_then(|n| n.as_ref())
            .and_then(|n| match n {
                Node::Leaf { session, .. } => Some(session),
                _ => None,
            })
        else {
            return;
        };
        // Order the two ends so start is before end in reading order.
        let (a, b) = (sel.anchor, sel.end);
        let (start, endp) = if (a.0, a.1) <= (b.0, b.1) { (a, b) } else { (b, a) };
        // `contents_between` stops *before* its end column, while the
        // highlight covers the cell the pointer is on — so the last character
        // of a selection was shown as taken and then not copied. One past it
        // is what the eye was promised.
        let text = match session.parser().lock() {
            Ok(p) => p.screen().contents_between(
                start.0,
                start.1,
                endp.0,
                endp.1.saturating_add(1),
            ),
            Err(_) => return,
        };
        let text = text.trim_end_matches(['\n', ' ']).to_string();
        if text.is_empty() {
            return;
        }
        if let Some(cb) = self.clipboard.as_mut() {
            let _ = cb.set_text(text);
        }
        self.message = Some(tr(self.lang, "copied", "コピーしました").into());
    }

    pub(crate) fn select_shell_leaf_at(&mut self, col: u16, row: u16) {
        let hit = self.shell_leaves.iter().copied().find(|(_, _, r, _)| hit_rect(*r, col, row));
        if let Some((tab, leaf, _, _)) = hit {
            self.shell.active = tab;
            if let Some(t) = self.shell.tabs.get_mut(tab) {
                t.active = leaf;
            }
        }
    }

    pub(crate) fn cursor_to_row(&mut self, pane: FocusedPane, row: u16) {
        let rect = match pane {
            FocusedPane::Left => self.layout_rects.left,
            FocusedPane::Right => self.layout_rects.right,
            FocusedPane::Shell => return,
        };
        // The list starts two rows in: the top border, then the column header.
        let Some(offset) = row.checked_sub(rect.y + 2) else { return };
        if offset >= rect.height.saturating_sub(3) {
            return;
        }
        let Some(p) = self.active_pane_mut() else { return };
        // The window the listing is actually showing — the pane keeps it, so
        // this is a lookup rather than a guess. It used to be derived from
        // the cursor with a formula that assumed the cursor was on the last
        // visible row, which is what made a click land somewhere else and
        // then scroll the file to the bottom of the pane.
        let idx = p.scroll + offset as usize;
        if idx < p.entries.len() {
            p.cursor = idx;
        }
    }
}
