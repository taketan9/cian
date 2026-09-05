//! The rendering layer: every `draw_*` function plus the colour/geometry
//! helpers they use. Split out of lib.rs. These take `&App` / `&mut App` and
//! never mutate domain state beyond stashing layout rects for the mouse code.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;
use tui_term::widget::PseudoTerminal;

use super::*;

/// The local model's colour. Every "AI - simple" window — the chat, the prompts
/// it asks first, and the review lists its answers become — wears this cyan, so
/// a glance says the answer came from the model configured in `cian.ai`.
const AI_SIMPLE: Color = Color::Rgb(0, 190, 205);
/// The carmine a remote pane wears, so a listing on a server never looks like
/// a listing on this disk.
const CRMAINE: Color = Color::Rgb(214, 45, 70);
/// cian's own cyan, and it never moves.
///
/// The bright stop of the icon's gradient (`packaging/icon.py` G1) — the two
/// colours he said he liked the look of. The frame round the pane holding the
/// keys wears this in the ordinary mode, in every one of the eighteen
/// palettes.
///
/// 2026-09-06:「どんなテーマでも同じ枠色にしてほしい。シアンっぽい色に
/// なっていたはずだ」。It was `theme().accent`, so the frame was a different
/// colour in each palette — and the one thing the frame says (**this side has
/// the keys**) is the same sentence in all of them. The four mode colours
/// beside it in `focus_badge_color` were already fixed for that exact reason;
/// this was the odd one out.
const CIAN: Color = Color::Rgb(22, 203, 225);

/// True when this popup belongs to the AI - simple family, and so wears
/// [`AI_SIMPLE`] rather than the theme accent.
fn is_ai_simple(popup: &Popup) -> bool {
    match popup {
        Popup::AiChat { skin, .. } => skin.simple,
        Popup::AiShellConfirm { .. }
        | Popup::CommitMessage { .. }
        | Popup::JunkReview { .. }
        | Popup::StructureReview { .. } => true,
        // `:renamepattern` and `:find` share their result lists with the AI; only the
        // AI side of each belongs to the family.
        Popup::RenameReview { by_ai, .. } | Popup::FindResults { by_ai, .. } => *by_ai,
        // The AI prompts; every other text input is a plain file operation.
        Popup::TextInput { kind, .. } => matches!(
            kind,
            InputKind::AiShellCmd
                | InputKind::AiShellRefine { .. }
                | InputKind::AiRename
                | InputKind::AiSearch
        ),
        _ => false,
    }
}

/// The frame colour for a popup: cyan for the AI - simple family, the theme's
/// own accent for everything else.
fn popup_accent(popup: &Popup) -> Color {
    // Fitted to the dialog it frames: the accent is chosen to *be* an accent
    // on the theme's page, and a dialog is a different surface — one that is
    // light on a light theme, since dialogs follow the theme now.
    let c = if is_ai_simple(popup) { AI_SIMPLE } else { theme().accent };
    text_tone(c, theme().popup_bg)
}

/// Normal three-surface layout: left/right file panes on top, shell below.
/// Apply a file pane's theme override (if any) to the active-theme global
/// before it draws, returning the palette to restore once it has. Per-pane
/// themes (#8) let the two columns wear different palettes; the swap is scoped
/// to that single `draw_file_pane` call so the shell and bars keep the app
/// theme. `side` is 0 = left, 1 = right.
/// Returns `Some(previous theme)` only when this pane actually has an override
/// and the global was swapped — the caller restores it afterward. `None` means
/// the pane follows the app theme and the global was left untouched (so a frame
/// with no per-pane themes does no theme writes at all).
fn push_pane_theme(app: &App, side: usize) -> Option<ResolvedTheme> {
    let t = app.pane_theme[side].as_deref().and_then(theme_preset)?;
    let prev = theme();
    set_theme(t);
    Some(prev)
}

fn draw_split(f: &mut Frame, main_area: Rect, app: &mut App, ov: AnimOverride) {
    app.ensure_git();
    let main_pct = ov.ratio_for(DividerTarget::Main, app.main_pct);
    let main_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(main_pct), Constraint::Percentage(100 - main_pct)])
        .split(main_area);
    let panes_area = main_split[0];
    let shell_area = main_split[1];

    let panes_pct = ov.ratio_for(DividerTarget::Panes, app.panes_pct);
    let panes_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(panes_pct), Constraint::Percentage(100 - panes_pct)])
        .split(panes_area);

    app.layout_rects = LayoutRects {
        left: panes_split[0],
        right: panes_split[1],
        shell: shell_area,
    };

    let mut leaves = Vec::new();
    let mut icon_slots = Vec::new();
    let mut tab_rects = Vec::new();
    let mut sort_rects = Vec::new();
    let mut crumb_rects = Vec::new();
    let mut nav_rects = Vec::new();
    let mut dividers = vec![
        Divider {
            zone: seam_zone(Direction::Vertical, panes_area, shell_area),
            parent: main_area,
            dir: Direction::Vertical,
            target: DividerTarget::Main,
        },
        Divider {
            zone: seam_zone(Direction::Horizontal, panes_split[0], panes_split[1]),
            parent: panes_area,
            dir: Direction::Horizontal,
            target: DividerTarget::Panes,
        },
    ];

    let visual_for_left = if app.focused == FocusedPane::Left { app.visual_anchor } else { None };
    let visual_for_right = if app.focused == FocusedPane::Right { app.visual_anchor } else { None };

    let (bg_l, bg_r) = (app.pane_bg[0], app.pane_bg[1]);
    let mut tracks: Vec<crate::ScrollTrack> = Vec::new();
    // The listings' scroll, settled once a frame from the geometry the last
    // one measured — the same arrangement the editor panel uses. Doing it
    // inside the pane renderer would mean borrowing the pane mutably while
    // its own title is still borrowed from it.
    let (fl_l, fl_r) = (app.flash_level(FocusedPane::Left), app.flash_level(FocusedPane::Right));
    let restore = push_pane_theme(app, 0);
    // Taken rather than borrowed: the panes are drawn with `&mut`, and a borrow
    // of `app.git` would keep the whole `app` borrowed alongside it. Put back a
    // few lines down — see [`App::take_git`].
    let (git_l, git_r) = (
        app.take_git(FocusedPane::Left),
        app.take_git(FocusedPane::Right),
    );
    crate::prof::timed(crate::prof::Phase::Panes, || {
    draw_file_pane(f, panes_split[0], &mut app.left, &mut tracks, app.focused == FocusedPane::Left, visual_for_left, app.mode, bg_l, fl_l, FocusedPane::Left, &mut tab_rects, git_l.as_ref(), app.lang, &mut sort_rects, &mut crumb_rects, &mut nav_rects, app.skin, app.native_icons, &mut icon_slots);
    });
    if let Some(prev) = restore { set_theme(prev); }
    let restore = push_pane_theme(app, 1);
    crate::prof::timed(crate::prof::Phase::Panes, || {
    draw_file_pane(f, panes_split[1], &mut app.right, &mut tracks, app.focused == FocusedPane::Right, visual_for_right, app.mode, bg_r, fl_r, FocusedPane::Right, &mut tab_rects, git_r.as_ref(), app.lang, &mut sort_rects, &mut crumb_rects, &mut nav_rects, app.skin, app.native_icons, &mut icon_slots);
    });
    if let Some(prev) = restore { set_theme(prev); }
    app.put_git(FocusedPane::Left, git_l);
    app.put_git(FocusedPane::Right, git_r);
    // With preview on and a file pane focused, the shell panel's area shows
    // the file under the cursor instead; the PTY runs on underneath, and
    // focusing the shell (Shift+J / click) gets its pixels back.
    app.scroll_tracks = std::mem::take(&mut tracks);
    // The switcher, on the top border row. The classic view has no toolbar to
    // hang it from and should not grow one — a row given to chrome is a row
    // taken from the files — so it sits in the frame itself, at the right-hand
    // end where nothing else is drawn.
    app.grid_buttons.clear();
    let th = theme();
    draw_view_switcher(f, Rect::new(main_area.x, main_area.y, main_area.width, 1), app, &th);
    let log_border = recording_pulse(app.started.elapsed());
    if app.preview_on && app.focused != FocusedPane::Shell {
        crate::prof::timed(crate::prof::Phase::Shell, || draw_preview_panel(f, shell_area, app));
    } else {
        // draw_shell sizes each pane's PTY to its computed sub-rect.
        crate::prof::timed(crate::prof::Phase::Shell, || {
            draw_shell(f, shell_area, &mut app.shell, app.focused == FocusedPane::Shell, &mut dividers, &mut leaves, ov, &mut tab_rects, log_border)
        });
    }
    app.dividers = dividers;
    app.shell_leaves = leaves;
    app.icon_slots.extend(icon_slots);
    app.tab_rects = tab_rects;
    app.sort_rects = sort_rects;
    app.crumb_rects = crumb_rects;
    app.nav_rects = nav_rects;
}

/// The focused surface drawn at an arbitrary rect, used as the floating layer
/// of a zoom transition. Deliberately does not touch `app.layout_rects`: the
/// backdrop already set those, and hit-testing should follow the resting
/// layout rather than a rect that is still moving.
fn draw_zoom_overlay(f: &mut Frame, rect: Rect, app: &mut App, ov: AnimOverride) {
    let mut sink = Vec::new();
    // A zoomed pane draws over the resting layout; its bar is the one the
    // mouse should find, so these replace what the backdrop recorded.
    let mut tracks: Vec<crate::ScrollTrack> = Vec::new();
    match app.focused {
        FocusedPane::Left => {
            let (bg, fl) = (app.pane_bg[0], app.flash_level(FocusedPane::Left));
            let va = app.visual_anchor;
            let restore = push_pane_theme(app, 0);
            let g = app.take_git(FocusedPane::Left);
            draw_file_pane(f, rect, &mut app.left, &mut tracks, true, va, app.mode, bg, fl, FocusedPane::Left, &mut Vec::new(), g.as_ref(), app.lang, &mut Vec::new(), &mut Vec::new(), &mut Vec::new(), app.skin, app.native_icons, &mut Vec::new());
            app.put_git(FocusedPane::Left, g);
            if let Some(prev) = restore { set_theme(prev); }
        }
        FocusedPane::Right => {
            let (bg, fl) = (app.pane_bg[1], app.flash_level(FocusedPane::Right));
            let va = app.visual_anchor;
            let restore = push_pane_theme(app, 1);
            let g = app.take_git(FocusedPane::Right);
            draw_file_pane(f, rect, &mut app.right, &mut tracks, true, va, app.mode, bg, fl, FocusedPane::Right, &mut Vec::new(), g.as_ref(), app.lang, &mut Vec::new(), &mut Vec::new(), &mut Vec::new(), app.skin, app.native_icons, &mut Vec::new());
            app.put_git(FocusedPane::Right, g);
            if let Some(prev) = restore { set_theme(prev); }
        }
        FocusedPane::Shell => {
            let log_border = recording_pulse(app.started.elapsed());
            draw_shell(f, rect, &mut app.shell, true, &mut sink, &mut Vec::new(), ov, &mut Vec::new(), log_border);
        }
    }
}

/// Float the active shell pane's terminal at `rect`, for the pane-zoom
/// transition. Just the one pane's screen, bordered, so it reads as that pane
/// growing rather than the whole panel.
fn draw_pane_zoom_overlay(f: &mut Frame, rect: Rect, app: &mut App) {
    if rect.width < 2 || rect.height < 2 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(accent_on_popup());
    let inner = rect.inner(Margin { vertical: 1, horizontal: 1 });
    f.render_widget(block, rect);

    let tab = app.shell.active;
    let leaf = app.shell.tabs.get(tab).map(|t| t.active);
    if let Some(leaf) = leaf {
        if let Some(Node::Leaf { session, bg }) =
            app.shell.tabs.get(tab).and_then(|t| t.nodes.get(leaf)).and_then(|n| n.as_ref())
        {
            if let Ok(parser) = session.parser().lock() {
                f.render_widget(PseudoTerminal::new(parser.screen()), inner);
            }
            if let Some(c) = bg {
                tint_default_cells(f, inner, *c);
            }
        }
    }
    if let Some(base) = theme().base_bg {
        tint_shell_base(f, inner, base, theme().file.plain);
    }
}

/// Zoomed layout: only the focused surface, filling the available area.
fn draw_zoomed(f: &mut Frame, area: Rect, app: &mut App, ov: AnimOverride) {
    let mut rects = LayoutRects::default();
    // Only the shell's internal splits are draggable while zoomed; the
    // main/panes borders are not on screen.
    let mut dividers = Vec::new();
    let mut leaves = Vec::new();
    let mut icon_slots = Vec::new();
    let mut tab_rects = Vec::new();
    let mut sort_rects = Vec::new();
    let mut crumb_rects = Vec::new();
    let mut nav_rects = Vec::new();
    let mut tracks: Vec<crate::ScrollTrack> = Vec::new();
    match app.focused {
        FocusedPane::Left => {
            rects.left = area;
            app.layout_rects = rects;
            let va = app.visual_anchor;
            let (bg, fl) = (app.pane_bg[0], app.flash_level(FocusedPane::Left));
            let restore = push_pane_theme(app, 0);
            let g = app.take_git(FocusedPane::Left);
            draw_file_pane(f, area, &mut app.left, &mut tracks, true, va, app.mode, bg, fl, FocusedPane::Left, &mut tab_rects, g.as_ref(), app.lang, &mut sort_rects, &mut crumb_rects, &mut nav_rects, app.skin, app.native_icons, &mut icon_slots);
            app.put_git(FocusedPane::Left, g);
            if let Some(prev) = restore { set_theme(prev); }
        }
        FocusedPane::Right => {
            rects.right = area;
            app.layout_rects = rects;
            let va = app.visual_anchor;
            let (bg, fl) = (app.pane_bg[1], app.flash_level(FocusedPane::Right));
            let restore = push_pane_theme(app, 1);
            let g = app.take_git(FocusedPane::Right);
            draw_file_pane(f, area, &mut app.right, &mut tracks, true, va, app.mode, bg, fl, FocusedPane::Right, &mut tab_rects, g.as_ref(), app.lang, &mut sort_rects, &mut crumb_rects, &mut nav_rects, app.skin, app.native_icons, &mut icon_slots);
            app.put_git(FocusedPane::Right, g);
            if let Some(prev) = restore { set_theme(prev); }
        }
        FocusedPane::Shell => {
            rects.shell = area;
            app.layout_rects = rects;
            let log_border = recording_pulse(app.started.elapsed());
            draw_shell(f, area, &mut app.shell, true, &mut dividers, &mut leaves, ov, &mut tab_rects, log_border);
        }
    }
    app.dividers = dividers;
    app.shell_leaves = leaves;
    app.icon_slots.extend(icon_slots);
    app.tab_rects = tab_rects;
    app.sort_rects = sort_rects;
    app.crumb_rects = crumb_rects;
    app.nav_rects = nav_rects;
}

pub(crate) fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // What the popup covers is worked out afresh every frame, by the drawing
    // itself. See [`clear_popup`].
    POPUP_INK.with(|c| c.set(None));
    // Which editor grammar is in force, for this frame. See [`notepad_keys`].
    NOTEPAD_KEYS.with(|c| c.set(app.notepad_keys()));

    // A popup that opened, closed, changed or scrolled: repaint everything.
    // See [`App::popup_shape`].
    let shape = (std::mem::discriminant(&app.popup), popup_scroll(&app.popup));
    if app.popup_shape != Some(shape) {
        app.popup_shape = Some(shape);
        // Painted again, not wiped. What this is for is overhanging ink, and
        // painting over it is enough to remove it — blanking the screen first
        // only added the flash. See [`App::full_repaint`].
        app.full_repaint = true;
    }
    // Where the pictures go is decided afresh every frame, so the list starts
    // empty every frame.
    //
    // It did not, and the cost was the whole windowed build feeling slow. The
    // split and zoomed layouts *replace* the list at the end of the frame, but
    // the detail view, the sidebar and the icon grid each `extend` it — so in
    // exactly the views the window opens in, thirty-odd slots were added per
    // frame and none ever removed. After ten seconds of sitting still the layer
    // was being handed six thousand quads to draw, each with its own bind group
    // and draw call, for a screen holding thirty icons. Half a core, gone, and
    // growing; the memory went the same way.
    app.icon_slots.clear();
    // …and so does the picture, which is one slot or none.
    app.image_slot = None;
    // A light theme paints the whole surface so gaps, the shell panel and the
    // bottom bars share one background rather than showing the terminal's own.
    if let Some(bg) = theme().base_bg {
        f.render_widget(Block::default().style(Style::default().bg(bg)), area);
    }
    // Command and filter modes add a prompt line above the status bar; the key
    // hints take another. A very short window drops the hints rather than the
    // listing.
    // The editor panel types on cian's own prompt line while it is docked in
    // a pane, so everything typed at cian is typed in the same place.
    let docked_prompt = app
        .viewer_dock
        .filter(|p| *p == app.focused)
        .and_then(|_| editor_prompt(&app.popup, app.lang));
    let prompt_line =
        matches!(app.mode, Mode::Command | Mode::Filter) || docked_prompt.is_some();
    let hint_line = app.show_key_hints && area.height >= 12;
    let bottom_lines = 1 + u16::from(prompt_line) + u16::from(hint_line);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(bottom_lines)])
        .split(area);
    let main_area = vertical[0];
    let bottom_area = vertical[1];

    let ov = app.anim_override();
    // A zoom transition draws the normal layout as a backdrop and floats the
    // zooming surface above it, so it visibly grows out of (or shrinks back
    // into) its own pane.
    if let Some(Anim { kind: AnimKind::Zoom { from, to }, .. }) = app.anim {
        let t = app.anim.map(|a| a.progress()).unwrap_or(1.0);
        draw_split(f, main_area, app, ov);
        let rect = lerp_rect(from, to, t);
        f.render_widget(Clear, rect);
        draw_zoom_overlay(f, rect, app, ov);
    } else if let Some(Anim { kind: AnimKind::PaneZoom { from, to }, .. }) = app.anim {
        // Backdrop keeps the shell's splits (ov.show_splits); the active pane
        // floats above them, growing out of or shrinking into its slot.
        let t = app.anim.map(|a| a.progress()).unwrap_or(1.0);
        draw_split(f, main_area, app, ov);
        let rect = lerp_rect(from, to, t);
        f.render_widget(Clear, rect);
        draw_pane_zoom_overlay(f, rect, app);
    } else if app.icon_view {
        // The icon view is one pane over the whole window, deliberately. A grid
        // of pictures wants width more than anything else, and two of them side
        // by side leave each too narrow to be worth looking at — which is why
        // no desktop file manager offers a two-pane icon view either.
        draw_icon_grid(f, main_area, app);
    } else if app.skin == Skin::Finder && !app.zoomed {
        draw_detail_view(f, main_area, app, ov);
    } else if app.zoomed {
        draw_zoomed(f, main_area, app, ov);
    } else {
        draw_split(f, main_area, app, ov);
    }

    // Nothing is drawn over a popup. The picture layer composites on top of
    // every cell, so an icon recorded for a row underneath would sit on the
    // dialog rather than behind it — and a file listing's worth of them looks
    // like the listing is still there.
    // Pictures under a dialog are dealt with after it has been laid out, at
    // the end of this function: where the context menu landed is not known
    // until it is drawn. See `hide_icons_under_popup`.
    let popup_open = !matches!(app.popup, Popup::None);

    // Reverse the cells of a shell text selection, over whatever was drawn.
    if let Some(sel) = app.shell_sel {
        highlight_shell_selection(f, &sel);
    }

    // Stack the bottom rows: [prompt] [hints] [status]. Each is claimed only if
    // the strip actually has room — a window can be short enough that Layout
    // hands back fewer rows than were asked for, and writing past the buffer
    // panics.
    let end = bottom_area.y.saturating_add(bottom_area.height);
    let mut row = bottom_area.y;
    let claim = |row: &mut u16| -> Option<Rect> {
        if *row >= end {
            return None;
        }
        let r = Rect::new(bottom_area.x, *row, bottom_area.width, 1);
        *row += 1;
        Some(r)
    };

    // Note: `claim` must only be called for rows that are actually drawn, so
    // each branch guards its flag *before* claiming.
    if prompt_line {
        if let Some(cmd_area) = claim(&mut row) {
            if let Some(text) = &docked_prompt {
                f.render_widget(
                    Paragraph::new(truncate(text, cmd_area.width as usize)).style(prompt_style()),
                    cmd_area,
                );
            } else if app.mode == Mode::Filter {
                let matched = app.active_pane().map(|p| p.entries.len()).unwrap_or(0);
                let total = app.active_pane().map(|p| p.all_entries.len()).unwrap_or(0);
                draw_prompt_line(
                    f,
                    cmd_area,
                    &format!("filter /{}_", app.filter_buffer),
                    &format!("{}/{} match  Enter=keep  Esc=clear", matched, total),
                );
            } else {
                draw_command_line(f, cmd_area, &app.command_buffer);
            }
        }
    }
    if hint_line {
        if let Some(r) = claim(&mut row) {
            draw_key_hints(f, r, app);
        }
    }
    if let Some(r) = claim(&mut row) {
        draw_status(f, r, app);
    }

    if app.op_job.is_some() && !app.op_bar_hidden {
        draw_op_progress(f, area, app);
    }
    // The directory comparison shows the same bar while it runs.
    if let Some(job) = &app.diff_job {
        draw_progress_bar(f, area, job.label, &job.latest, job.started, app.lang);
    }
    // The popups that draw themselves and are the last word on the frame — each
    // needs `&mut App`, to stash the geometry the mouse is measured against.
    //
    // They leave through one door on purpose. Each used to `return` where it
    // stood, which skipped the tidying at the foot of this function — and the
    // first thing that tidying does is take the file icons away from under the
    // popup. So in the window, where the icons are real pictures composited
    // over the cells, a listing's worth of them was drawn *on top of* the AI
    // chat. Which is exactly how it was reported.
    if draws_its_own_frame(f, area, app) {
        hide_icons_under_popup(app);
        return;
    }
    // Where the viewer goes: over everything when it was opened with F3 or
    // Shift+Tab, or in the pane whose listing it replaced when Enter opened
    // it. Everything below — the geometry the mouse is measured against and
    // the drawing itself — is told the same rectangle.
    // `viewer_return` counts too: a menu opened from the panel leaves the
    // file stashed behind it, and it is still docked in its pane — drawing it
    // over the whole window instead looked like the panel had maximised.
    let viewer_area = match app.viewer_dock {
        Some(p) if matches!(app.popup, Popup::Viewer { .. }) || app.viewer_return.is_some() => {
            app.layout_rects.for_pane(p)
        }
        _ => area,
    };
    if !matches!(app.popup, Popup::None) {
        // Remember where the context menu landed so a click can hit its rows.
        if let Popup::ContextMenu { items, at, .. } = &app.popup {
            app.menu_rect = context_menu_rect(items, *at, area, app.menu_lang);
        }
        // And the viewer's text body, so a drag maps to a line — plus the
        // line-number gutter width, so it maps to a char column too.
        if let Popup::Viewer { view, preview, blame, editing, shape, .. } = &app.popup {
            let inner_w = centered_rect(
                viewer_area.width.saturating_sub(4),
                viewer_area.height.saturating_sub(2),
                viewer_area,
            )
            .inner(Margin { vertical: 1, horizontal: 2 })
            .width;
            let ow = shape.as_deref().map_or(0, |s| outline_width(inner_w, s.shown, s.items.len()));
            // The ruler takes the first row of the body; without counting it
            // here a click lands one line above what was pointed at.
            let rr = u16::from(
                app.show_ruler && !*preview && view.kind == cian_core::viewer::ViewKind::Text,
            );
            let docked = app.viewer_dock.is_some();
            app.viewer_frame = viewer_frame_rect_docked(viewer_area, docked);
            app.viewer_rect = viewer_body_rect_docked(viewer_area, ow, rr, docked);
            app.outline_rect = Rect::new(
                app.viewer_rect.x.saturating_sub(ow),
                app.viewer_rect.y,
                ow.saturating_sub(1),
                app.viewer_rect.height,
            );
            app.viewer_gutter = if !blame.is_empty() && !*preview && !*editing {
                BLAME_W as u16
            } else if !*preview && view.kind == cian_core::viewer::ViewKind::Text {
                let fold_col = u16::from(shape.as_deref().is_some_and(|s| !s.items.is_empty()));
                (format!("{}", view.lines.len()).len().max(3) + 1) as u16 + fold_col
            } else {
                0
            };
        }
        let find_state = app
            .find_job
            .as_ref()
            .map(|j| (j.query.as_str(), j.root_label.as_str(), j.done, j.mode));
        let dests = app.dest_choices();
        let lang = app.lang;
        let menu_lang = app.menu_lang;
        let show_ws = app.show_ws;
        let ruler = app.show_ruler;
        app.popup_zones.clear();
        // Every open file's name, in order, with the one on screen back in its
        // place — the strip has to name them all, and the active one is not in
        // the list while it is being read.
        let names: Vec<String> = {
            let mut v: Vec<String> = app
                .viewer_tabs
                .iter()
                .map(|p| match p {
                    Popup::Viewer { title, .. } => title.clone(),
                    _ => String::new(),
                })
                .collect();
            if let Popup::Viewer { title, .. } = &app.popup {
                let at = app.viewer_tab_idx.min(v.len());
                v.insert(at, title.clone());
            }
            v
        };
        let mut tab_rects: Vec<(Rect, usize)> = Vec::new();
        let mut close_rect = Rect::new(0, 0, 0, 0);
        let mut vtracks: Vec<crate::ScrollTrack> = Vec::new();
        // A menu — or a chat, or the theme gallery — opened *from* the viewer
        // is drawn on top of it, not instead of it. The file is what the
        // question is about; losing sight of it while answering is the wrong
        // way round.
        if let Some(behind) = app.viewer_return.take() {
            let mut behind = behind;
            if let Some(other) = app.viewer_split.take() {
                let (first, second) = split_viewer_areas(viewer_area, app.viewer_split_lr);
                let (mine, theirs) = if app.viewer_split_focus {
                    (second, first)
                } else {
                    (first, second)
                };
                let mut other = other;
                let docked = app.viewer_dock.is_some();
                draw_viewer(f, theirs, &mut other, lang, (show_ws, ruler), (0, &[], &[]), docked, true, &mut Vec::new());
                draw_viewer(f, mine, &mut behind, lang, (show_ws, ruler), (0, &[], &[]), docked, true, &mut Vec::new());
                app.viewer_split = Some(other);
            } else {
                draw_viewer(
                    f,
                    viewer_area,
                    &mut behind,
                    lang,
                    (show_ws, ruler),
                    (0, &[], &[]),
                    app.viewer_dock.is_some(),
                    true,
                    &mut Vec::new(),
                );
            }
            app.viewer_return = Some(behind);
        }
        // A split viewer is two viewers. The half not in focus is drawn first
        // and dimmed, exactly as the unfocused file pane is, so which one the
        // keyboard is pointed at is never a guess.
        // Only while a viewer is what is on screen. A menu, a confirm dialog
        // or a chat is a different popup entirely, and letting the split
        // branch draw it meant drawing nothing at all — the dialog was there,
        // invisible, quietly taking the next Enter.
        if matches!(app.popup, Popup::Viewer { .. }) && app.viewer_split.is_some() {
            let other = app.viewer_split.take().expect("just checked");
            let full = area;
            // Within the panel: a docked panel splits inside the pane it is
            // docked in, not across the window it happens to sit on.
            let (first, second) = split_viewer_areas(viewer_area, app.viewer_split_lr);
            // Which half each file occupies is fixed; crossing over moves the
            // focus, not the files. Drawing the focused one always on the left
            // made the two look as though they had traded places.
            let (mine, theirs) = if app.viewer_split_focus {
                (second, first)
            } else {
                (first, second)
            };
            // Where each half ended up, so a click in the one the keyboard is
            // not on can cross to it.
            app.viewer_half_rects = [mine, theirs];
            let mut other = other;
            // Either buffer moved on: work the marks out again, so the
            // comparison keeps telling the truth while both are edited. This
            // is the whole reason for doing it in place rather than in a
            // window that would have gone stale the moment you typed.
            if app.viewer_diff.is_some() {
                let now = {
                    let f = |p: &Popup| match p {
                        Popup::Viewer { view, .. } => crate::content_key(&view.lines),
                        _ => 0,
                    };
                    (f(&app.popup), f(&other))
                };
                if app.viewer_diff.as_deref().is_some_and(|d| d.fp != now) {
                    app.viewer_split = Some(other);
                    app.recompute_viewer_diff();
                    other = app.viewer_split.take().expect("put back just above");
                }
            }
            let (dm, dt) = match app.viewer_diff.as_deref() {
                Some(d) => (d.mine.as_slice(), d.theirs.as_slice()),
                None => (&[][..], &[][..]),
            };
            draw_viewer(f, theirs, &mut other, lang, (show_ws, ruler), (0, &[], dt), false, true, &mut Vec::new());
            f.render_widget(
                Block::default().style(Style::default().fg(Color::Rgb(90, 90, 105))),
                theirs,
            );
            (app.viewer_tab_rects, app.viewer_close_rect) = draw_viewer(
                f,
                mine,
                &mut app.popup,
                lang,
                (show_ws, ruler),
                (app.viewer_tab_idx, &names, dm),
                false,
                true,
                &mut app.scroll_tracks,
            );
            app.viewer_split = Some(other);
            app.popup_zones.clear();
            // Everything the mouse needs, for the half the keyboard is on —
            // without this the clicks were being measured against a viewer
            // that filled the whole screen, which is not where anything was.
            let ow = if let Popup::Viewer { shape, .. } = &app.popup {
                let inner_w = viewer_frame_rect(mine)
                    .inner(Margin { vertical: 1, horizontal: 2 })
                    .width;
                shape.as_deref().map_or(0, |s| outline_width(inner_w, s.shown, s.items.len()))
            } else {
                0
            };
            let rr = if let Popup::Viewer { preview, view, .. } = &app.popup {
                u16::from(
                    app.show_ruler && !*preview && view.kind == cian_core::viewer::ViewKind::Text,
                )
            } else {
                0
            };
            app.viewer_frame = viewer_frame_rect(mine);
            app.viewer_rect = viewer_body_rect(mine, ow, rr);
            app.outline_rect = Rect::new(
                app.viewer_rect.x.saturating_sub(ow),
                app.viewer_rect.y,
                ow.saturating_sub(1),
                app.viewer_rect.height,
            );
            let _ = full;
            // Out through the same door as everything else: a split editor is
            // still a popup, and the icons still belong under it rather than
            // over it. See `draws_its_own_frame`.
            hide_icons_under_popup(app);
            return;
        }
        draw_popup(
            f,
            viewer_area,
            &mut app.popup,
            &app.config.ssh_hosts,
            &app.config.snippets,
            find_state,
            &dests,
            &mut app.popup_zones,
            lang,
            menu_lang,
            show_ws,
            ruler,
            app.viewer_tab_idx,
            &names,
            &mut tab_rects,
            &mut close_rect,
            app.viewer_dock.is_some(),
            app.viewer_dock.map(|d| d == app.focused).unwrap_or(true),
            &mut vtracks,
        );
        app.scroll_tracks.append(&mut vtracks);
        app.viewer_tab_rects = tab_rects;
        app.viewer_close_rect = close_rect;
    } else {
        app.popup_zones.clear();
    }
    if !matches!(app.popup, Popup::Viewer { .. }) {
        app.viewer_tab_rects.clear();
    }
    if app.viewer_split.is_none() {
        app.viewer_half_rects = [Rect::new(0, 0, 0, 0); 2];
    }

    // A brief "starting up" splash while the AI probe runs — non-blocking (it
    // never intercepts input, and yields the moment a popup opens), just so the
    // first couple of seconds don't feel dead.
    if matches!(app.popup, Popup::None) && app.is_starting_up() {
        draw_startup_splash(f, area, app.startup_at.elapsed().as_millis());
    }
    // Icons are drawn on top of every cell by the front end, so one recorded
    // for a row that a dialog now covers would sit *on* the dialog. Done here
    // rather than before the popup was drawn, because where the context menu
    // landed is only known once it has been laid out — and doing it early is
    // what made every icon in the listing disappear the moment the menu
    // opened, rather than just the handful behind it.
    if popup_open {
        hide_icons_under_popup(app);
    }

}

/// A centered, animated "starting up" card. Drawn over the UI; purely cosmetic.
/// One tile of the icon grid, in cells.
///
/// A cell is about twice as tall as it is wide, so a square picture four rows
/// high needs roughly eight columns. Fourteen leaves room for a name under it
/// without the names of neighbouring tiles running together.
pub(crate) const TILE_W: u16 = 14;
pub(crate) const TILE_H: u16 = 6;
/// Rows of the tile the picture occupies; the rest is the name.
const TILE_ICON_H: u16 = 4;

/// The chrome a desktop file manager wears: places down the left, buttons and
/// an address bar across the top. Returns the area left for the listing, or
/// `None` when the window is too small to be worth dressing.
///
/// Shared by both single-pane views. They differ only in what fills the space
/// underneath — tiles or rows — and a sidebar that appeared in one and not the
/// other would make switching between them feel like changing programs.
fn draw_desktop_chrome(
    f: &mut Frame,
    area: Rect,
    app: &mut App,
    th: &ResolvedTheme,
    bg: Option<Color>,
    min_w: u16,
) -> Option<Rect> {
    let mut inner = area.inner(Margin { vertical: 1, horizontal: 1 });
    if inner.width < min_w || inner.height < 5 {
        return None;
    }

    // The sidebar, if there is room for one beside a usable grid. A window too
    // narrow to hold both loses the sidebar rather than squeezing the files
    // into two columns — which is what Finder does when you drag it small.
    if inner.width >= SIDEBAR_W + min_w * 2 {
        let side = Rect::new(inner.x, inner.y, SIDEBAR_W, inner.height);
        crate::prof::timed(crate::prof::Phase::Sidebar, || draw_sidebar(f, side, app, th, bg));
        inner = Rect::new(
            inner.x + SIDEBAR_W,
            inner.y,
            inner.width - SIDEBAR_W,
            inner.height,
        );
    } else {
        app.sidebar_rows.clear();
    }

    // A toolbar, because the grid has no title row to hang the arrows on and
    // because someone who came for an icon view did not come to learn that
    // Backspace goes up. Each label's rect is remembered so a click can find it.
    let bar = Rect::new(inner.x, inner.y, inner.width, 1);
    let addr = Rect::new(inner.x, inner.y + 1, inner.width, 1);
    inner = Rect::new(inner.x, inner.y + 3, inner.width, inner.height.saturating_sub(3));

    let lit = Style::default().fg(text_tone(th.accent, bg.unwrap_or(th.popup_bg)));
    let mut spans = Vec::new();
    let mut x = bar.x;
    app.grid_buttons.clear();
    for (label, what) in [
        ("  ‹ 戻る  ", GridButton::Back),
        ("  › 進む  ", GridButton::Forward),
        ("  ↑ 上へ  ", GridButton::Up),
    ] {
        let w = crate::util::width(label) as u16;
        app.grid_buttons.push((what, Rect::new(x, bar.y, w, 1)));
        x += w;
        spans.push(Span::styled(label, lit));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), bar);
    // …and the view switcher at the other end of the same row.
    draw_view_switcher(f, bar, app, th);

    // The address bar. Clicking it opens the same "go to path" prompt `:` has
    // always had, seeded with where you are — so a typed path and Enter get you
    // there, which is the one thing everyone expects of the strip at the top.
    //
    // Drawn as a field rather than as a line of text. The first version was the
    // path in the theme's ordinary colours on the ordinary background, and it
    // read as a caption: correct, present, and impossible to recognise as
    // something you could click. A box, a folder in front of it and a hint at
    // the end is what makes it look like it takes typing.
    // Drawn as a breadcrumb, the way Explorer's is: the path broken into its
    // parts with chevrons between them, and every part a place you can click.
    // A path is a route rather than a string, and the bar that shows it should
    // let you step back along it — reading the whole line and retyping it to
    // go up two directories is what an address bar is supposed to save you.
    let cwd = app.active_pane().map(|p| p.cwd.clone()).unwrap_or_default();
    let field = Style::default().fg(text_tone(th.file.plain, th.status_bg)).bg(th.status_bg);
    let quiet = Style::default().fg(th.dim).bg(th.status_bg);
    let lead = Style::default().fg(th.file.directory).bg(th.status_bg);

    // Every ancestor, root first, with the name to show for each.
    let mut parts: Vec<(String, PathBuf)> = Vec::new();
    let mut acc = PathBuf::new();
    for c in cwd.components() {
        acc.push(c.as_os_str());
        let name = match c {
            std::path::Component::RootDir => "/".to_string(),
            other => other.as_os_str().to_string_lossy().into_owned(),
        };
        parts.push((name, acc.clone()));
    }

    let mut spans = vec![Span::raw(" "), Span::styled(" \u{f07b}  ", lead)];
    let mut x = addr.x + 4;
    let limit = addr.x + addr.width.saturating_sub(2);
    app.grid_crumbs.clear();
    // From the end backwards would be better on a very deep path; for now the
    // tail is simply cut, which is what the window's width forces anyway.
    for (i, (name, path)) in parts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" › ", quiet));
            x += 3;
        }
        let w = crate::util::width(name) as u16;
        if x + w >= limit {
            spans.push(Span::styled("…", quiet));
            break;
        }
        let last = i + 1 == parts.len();
        let style = if last { field.add_modifier(Modifier::BOLD) } else { field };
        spans.push(Span::styled(name.clone(), style));
        app.grid_crumbs.push((path.clone(), Rect::new(x, addr.y, w, 1)));
        x += w;
    }
    // The rest of the strip is the field itself: click it to type a path.
    if x < limit {
        spans.push(Span::styled(pad_to("", (limit - x) as usize), field));
    }
    app.grid_address = Some(addr);
    f.render_widget(Paragraph::new(Line::from(spans)), addr);

    Some(inner)
}

/// The three views, as three segments, with the one you are in filled in.
///
/// It was one button that said where you *are* and, pressed, went somewhere
/// else — to the classic view, whichever of the other two you were in. Which is
/// two questions a button should not raise: what does it do, and where will it
/// land. Three segments answer both by being three: the lit one is here, the
/// other two are one click away each, and nothing has to be cycled through.
///
/// Drawn right-aligned in `row`, and only if `row` is wide enough to hold it
/// with the buttons at the other end still readable.
///
/// Returns the rectangle it took, so a caller that shares the row knows what is
/// spoken for.
pub(crate) fn draw_view_switcher(
    f: &mut Frame,
    row: Rect,
    app: &mut App,
    th: &ResolvedTheme,
) -> Rect {
    let here = if app.icon_view {
        crate::ViewWanted::Icons
    } else if app.skin == Skin::Finder {
        crate::ViewWanted::Details
    } else {
        crate::ViewWanted::Classic
    };
    let segments = [
        (" ▤ 詳細 ", crate::ViewWanted::Details),
        (" ▥ クラシック ", crate::ViewWanted::Classic),
    ];
    let total: u16 = segments.iter().map(|(l, _)| crate::util::width(l) as u16).sum();
    // Half the row, at most: a switcher that pushes the address bar off the
    // screen is not an improvement on a switcher nobody can find.
    if row.width < total + 4 || total > row.width / 2 + total / 2 {
        return Rect::new(row.x + row.width, row.y, 0, 1);
    }
    // The control paints its own background rather than borrowing whatever is
    // behind it. In the classic view "behind it" is a border row, and dim text
    // on a border is the one thing the contrast test in `tests.rs` exists to
    // catch — it caught this.
    let surface = th.status_bg;
    let mut x = row.x + row.width - total;
    let taken = Rect::new(x, row.y, total, 1);
    let mut spans = Vec::new();
    for (label, want) in segments {
        let w = crate::util::width(label) as u16;
        app.grid_buttons.push((GridButton::View(want), Rect::new(x, row.y, w, 1)));
        x += w;
        // The one you are in is filled rather than merely brighter: a segmented
        // control says "here" with a block, and colour alone is a hint.
        let style = if want == here {
            Style::default()
                .fg(readable_on(th.selected_bg))
                .bg(th.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text_tone(th.dim, surface)).bg(surface)
        };
        spans.push(Span::styled(label, style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), taken);
    taken
}

/// How wide the grid's sidebar is, in cells.
pub(crate) const SIDEBAR_W: u16 = 22;

/// The places worth one click, down the left-hand side.
///
/// Two lists, the way every desktop file manager has them: the places the
/// system gives everyone, and the ones this user kept. cian has kept
/// bookmarks since long before it had a window — `shortcuts.lua`, the same
/// list `b` opens — so the sidebar shows those rather than inventing a second
/// set of favourites that would immediately disagree with the first.
fn draw_sidebar(
    f: &mut Frame,
    area: Rect,
    app: &mut App,
    th: &ResolvedTheme,
    bg: Option<Color>,
) {
    let surface = bg.unwrap_or(th.popup_bg);
    f.render_widget(
        Block::default().style(Style::default().bg(th.status_bg)),
        area,
    );

    let head = Style::default().fg(th.dim).add_modifier(Modifier::BOLD);
    let item = Style::default().fg(text_tone(th.file.plain, th.status_bg));
    // Where you are, as a filled row rather than a shade of text. A sidebar is
    // clicked at, and a click wants an answer you can see across the room —
    // coloured letters on a coloured panel are not one.
    let here = Style::default()
        .fg(text_tone(th.file.directory, th.selected_bg))
        .bg(th.selected_bg)
        .add_modifier(Modifier::BOLD);
    let cwd = app.active_pane().map(|p| p.cwd.clone()).unwrap_or_default();

    let mut lines: Vec<Line> = Vec::new();
    let mut rows: Vec<(PathBuf, u16)> = Vec::new();
    let mut y = area.y;

    let section = |lines: &mut Vec<Line>, y: &mut u16, title: &str| {
        lines.push(Line::from(Span::styled(format!(" {title}"), head)));
        *y += 1;
    };
    let native = app.native_icons;
    // Answers about the disk, at most once every few seconds — see
    // [`App::sidebar_dirs`]. Taken out of `app` for the length of the draw so
    // the closure below can read it while the rest of `app` is written to.
    //
    // Asked once per path and then never again. It was every three seconds,
    // which is fine arithmetic and a bad idea: ten questions to the OneDrive
    // sync engine is not a cost you can average away, it is a frame that takes
    // eighteen milliseconds while its neighbours take half of one — and a
    // hitch every three seconds is exactly what a program feels like when it
    // is described as slow. What is at stake is which icon a bookmark gets, so
    // a stale answer costs a wrong picture until the next start.
    let mut known: std::collections::HashMap<PathBuf, bool> =
        std::mem::take(&mut app.sidebar_dirs.1);
    let mut slots: Vec<crate::IconSlot> = Vec::new();
    let place = |lines: &mut Vec<Line>,
                 rows: &mut Vec<(PathBuf, u16)>,
                 slots: &mut Vec<crate::IconSlot>,
                 known: &mut std::collections::HashMap<PathBuf, bool>,
                 y: &mut u16,
                 icon: &str,
                 name: &str,
                 path: PathBuf| {
        if *y >= area.y + area.height {
            return;
        }
        let style = if path == cwd { here } else { item };
        // A picture where the front end can draw one, a glyph otherwise. The
        // glyphs are clipped in a window — their ink is wider than the cell —
        // and a sidebar of half-drawn symbols is worse than no symbols at all.
        let head = if native {
            slots.push(crate::IconSlot {
                x: area.x + 1,
                y: *y,
                w: 2,
                h: 1,
                path: path.clone(),
                is_dir: true,
                // A bookmark pointing somewhere that has gone still reads as a
                // place; asking the disk about it would answer "blank document".
                local: *known.entry(path.clone()).or_insert_with(|| path.is_dir()),
                glyph: icon.chars().next().map(|c| (c, rgb_of(th.file.directory))),
                prefer_glyph: false,
            });
            "   ".to_string()
        } else {
            format!("  {icon} ")
        };
        let label = fit(&format!("{head}{name}"), area.width as usize - 1);
        lines.push(Line::from(Span::styled(pad_to(&label, area.width as usize), style)));
        rows.push((path, *y));
        *y += 1;
    };

    app.sidebar_add = None;
    section(&mut lines, &mut y, "よく使う項目");
    for (icon, name, dir) in standard_places().iter().cloned() {
        place(&mut lines, &mut rows, &mut slots, &mut known, &mut y, icon, &name, dir);
    }

    // The user's own bookmarks. Groups are flattened to their leaves: a
    // sidebar is a list of places, and a place you have to open to reach is
    // not one.
    let mut saved: Vec<(String, PathBuf)> = Vec::new();
    collect_shortcuts(&app.shortcuts.entries, &mut saved);
    if !saved.is_empty() {
        lines.push(Line::from(""));
        y += 1;
        app.sidebar_add = Some(Rect::new(area.x, y, area.width, 1));
        lines.push(Line::from(vec![
            Span::styled(" お気に入り", head),
            Span::styled("        ＋ 追加", Style::default().fg(th.dim)),
        ]));
        y += 1;
        for (name, path) in saved {
            place(&mut lines, &mut rows, &mut slots, &mut known, &mut y, "\u{f07b}", &name, path);
        }
    }

    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(th.status_bg)),
        area,
    );
    let _ = surface;
    app.sidebar_rows = rows;
    app.sidebar_dirs.1 = known;
    // Handed back so the caller can add them to the frame's slots rather than
    // replacing them: the listing has its own.
    app.icon_slots.extend(slots);
}

/// `~/x` as an absolute path.
fn expand_home(raw: &str) -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match (raw.strip_prefix("~/"), home) {
        (Some(rest), Some(h)) => PathBuf::from(h).join(rest),
        _ if raw == "~" => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
        _ => PathBuf::from(raw),
    }
}

/// The places the system gives everyone, in the order Finder lists them.
pub(crate) fn standard_places() -> &'static [(&'static str, String, PathBuf)] {
    // Worked out once for the life of the process, because working it out is
    // not free and the sidebar is drawn on every frame.
    //
    // `known_dir` looks for each folder under the home directory *and* under
    // every OneDrive root, by English and Japanese name — up to ten paths
    // asked about per folder, forty for the four below. On this Mac that is
    // dozens of microseconds and invisible. On Windows, where the Desktop and
    // Documents usually *are* OneDrive's, every one of those questions goes
    // through the sync engine's filter driver, and a windowed cian spent
    // sixteen milliseconds a frame asking them — measured, in the window, by
    // someone who reported it as "もっさり". The answers cannot change while
    // cian runs.
    static PLACES: std::sync::OnceLock<Vec<(&'static str, String, PathBuf)>> =
        std::sync::OnceLock::new();
    PLACES.get_or_init(build_standard_places)
}

fn build_standard_places() -> Vec<(&'static str, String, PathBuf)> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let Some(home) = home else { return Vec::new() };
    let mut out = vec![("\u{f015}", "ホーム".to_string(), home.clone())];
    for (icon, label, english, japanese) in [
        ("\u{f0c7}", "デスクトップ", "Desktop", "デスクトップ"),
        ("\u{f019}", "ダウンロード", "Downloads", "ダウンロード"),
        ("\u{f02d}", "書類", "Documents", "ドキュメント"),
        ("\u{f03e}", "ピクチャ", "Pictures", "ピクチャ"),
    ] {
        // Only what is actually there: a sidebar entry that goes nowhere is
        // worse than one that is missing.
        if let Some(p) = crate::known_dir(&home, english, japanese) {
            out.push((icon, label.to_string(), p));
        }
    }
    out
}

/// Flatten the bookmark tree to the places in it.
///
/// Targets are expanded on the way out. A bookmark is written the way a person
/// writes a path — `~/Downloads` — and asking the system about a directory
/// literally called `~` gets the answer for a file that is not there.
fn collect_shortcuts(entries: &[crate::Shortcut], out: &mut Vec<(String, PathBuf)>) {
    for s in entries {
        if let Some(t) = &s.target {
            out.push((s.name.clone(), expand_home(t)));
        }
        if let Some(kids) = &s.children {
            collect_shortcuts(kids, out);
        }
    }
}

/// One pane as a detailed list, wearing the same chrome as the grid.
///
/// The two-pane layout is cian's whole shape, and this is the one place it is
/// set aside: a sidebar and an address bar want the width, and a person who
/// picked "details" from a view menu picked the thing Explorer calls details —
/// one folder, listed, with places down the side. Classic is one keystroke
/// away and still has both panes.
fn draw_detail_view(f: &mut Frame, area: Rect, app: &mut App, ov: AnimOverride) {
    let th = theme();
    let bg = th.base_bg;

    let Some(inner) = draw_desktop_chrome(f, area, app, &th, bg, 24) else {
        // Too narrow to dress: fall back to the layout that needs no room.
        draw_split(f, area, app, ov);
        return;
    };

    let mut tab_rects = Vec::new();
    let mut sort_rects = Vec::new();
    let mut crumb_rects = Vec::new();
    let mut nav_rects = Vec::new();
    let mut icon_slots = Vec::new();
    let mut tracks: Vec<crate::ScrollTrack> = Vec::new();
    let va = app.visual_anchor;
    let side = usize::from(app.focused == FocusedPane::Right);

    // Where the listing ended up, so a click can be turned back into a row.
    // Without this the rects still describe the two-pane layout and every click
    // lands on whatever row that geometry put under the pointer.
    let mut rects = crate::LayoutRects::default();
    if side == 1 {
        rects.right = inner;
    } else {
        rects.left = inner;
    }
    app.layout_rects = rects;
    let (pane_bg, fl) = (app.pane_bg[side], app.flash_level(app.focused));
    let restore = push_pane_theme(app, side);
    let g = app.take_git(app.focused);
    let which = if side == 1 { FocusedPane::Right } else { FocusedPane::Left };
    let tabs = if side == 1 { &mut app.right } else { &mut app.left };
    crate::prof::timed(crate::prof::Phase::Panes, || {
    draw_file_pane(
        f, inner, tabs, &mut tracks, true, va, app.mode, pane_bg, fl, which,
        &mut tab_rects, g.as_ref(), app.lang, &mut sort_rects, &mut crumb_rects,
        &mut nav_rects, app.skin, app.native_icons, &mut icon_slots,
    );
    });
    app.put_git(which, g);
    if let Some(prev) = restore {
        set_theme(prev);
    }
    app.icon_slots.extend(icon_slots);
    app.tab_rects = tab_rects;
    app.sort_rects = sort_rects;
    app.crumb_rects = crumb_rects;
    app.nav_rects = nav_rects;
    app.scroll_tracks = tracks;
    // The listing owns this rectangle, so a click in it is a click on a row —
    // the same question the grid answers with `grid_area`.
    app.grid_area = None;
}

/// The left pane as a grid of pictures.
///
/// The cells carry only the names and the selection; the pictures themselves
/// are drawn by whoever owns the surface, from the [`crate::IconSlot`]s pushed
/// here. Without a front end that can do that, this view is an empty grid —
/// which is why it is offered only in the window.
fn draw_icon_grid(f: &mut Frame, area: Rect, app: &mut App) {
    let th = theme();
    let bg = th.base_bg;
    let focus_bg = focus_badge_color(app.mode);

    // The top row carries the tabs, the way both other views carry them.
    //
    // It carried the directory instead, which the address bar below it already
    // says — so opening a second tab in this view changed nothing on screen at
    // all, and there was no way to tell which of them was showing, or that
    // there were two. Same strip, same numbers, same click targets as the
    // detail view, because it is the same tabs.
    let which_tabs = if app.focused == FocusedPane::Right
        || (app.focused == FocusedPane::Shell && app.last_file_pane == FocusedPane::Right)
    {
        FocusedPane::Right
    } else {
        FocusedPane::Left
    };
    let mut offsets = Vec::new();
    let title = {
        let tabs = if which_tabs == FocusedPane::Right { &app.right } else { &app.left };
        let focused = app.focused != FocusedPane::Shell;
        let (title, _) =
            tabs_title(tabs, focused, focus_bg, area.width.saturating_sub(2), &mut offsets);
        title
    };
    app.tab_rects.clear();
    for (i, off, w) in &offsets {
        app.tab_rects.push((which_tabs, *i, Rect::new(area.x + 1 + off, area.y, *w, 1)));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(Style::default().fg(focus_bg).add_modifier(Modifier::BOLD))
        .title(title);
    let block = match bg {
        Some(c) => block.style(Style::default().bg(c)),
        None => block,
    };
    f.render_widget(block, area);

    let Some(chrome) = draw_desktop_chrome(f, area, app, &th, bg, TILE_W) else { return };

    // The rightmost column belongs to the scrollbar, not to the tiles. Taken
    // before anything is measured, so the grid, the click map and the bar all
    // agree about where the tiles stop — the grid answers for every cell of
    // `grid_area`, and a bar drawn inside it would be a bar that cannot be
    // clicked.
    const BAR_W: u16 = 2;
    let bar = Rect::new(chrome.x + chrome.width.saturating_sub(BAR_W), chrome.y, BAR_W, chrome.height);
    let inner = Rect { width: chrome.width.saturating_sub(BAR_W), ..chrome };

    let cols = (inner.width / TILE_W).max(1) as usize;
    app.icon_cols = cols;
    app.grid_area = Some(inner);
    let rows = (inner.height / TILE_H).max(1) as usize;
    let per_page = cols * rows;

    // The grid keeps showing the last file pane while the focus is in the shell
    // panel below it — otherwise going to the shell emptied the whole window.
    let which = if app.focused == FocusedPane::Right
        || (app.focused == FocusedPane::Shell && app.last_file_pane == FocusedPane::Right)
    {
        FocusedPane::Right
    } else {
        FocusedPane::Left
    };
    let side = if which == FocusedPane::Right { &app.right } else { &app.left };
    let pane = side.active_ref();
    let synthetic = pane.is_synthetic();
    let total = pane.entries.len();
    // Scroll a page at a time, so the cursor's tile is always on screen and the
    // grid does not shuffle under the eye on every step.
    let page = pane.cursor.checked_div(per_page).unwrap_or(0);
    let start = page * per_page;
    let end = (start + per_page).min(total);

    let mut slots = Vec::new();
    for (n, e) in pane.entries[start..end].iter().enumerate() {
        let cx = inner.x + (n % cols) as u16 * TILE_W;
        let cy = inner.y + (n / cols) as u16 * TILE_H;
        let i = start + n;
        let selected = i == pane.cursor;
        let marked = pane.is_marked(i);

        // The picture sits centred in the tile's upper rows.
        let icon_w = TILE_ICON_H * 2;
        slots.push(crate::IconSlot {
            x: cx + (TILE_W - icon_w) / 2,
            y: cy,
            w: icon_w,
            h: TILE_ICON_H,
            path: e.path.clone(),
            is_dir: e.is_dir,
            local: !synthetic,
            // Only if the system has nothing — a grid of pictures is the point
            // of this view, and a tile with an empty square in it is not one.
            glyph: icon_for(e).chars().next().map(|c| (c, rgb_of(kind_for(e).color()))),
            prefer_glyph: false,
        });

        // The name, on the row under it, centred and cut to the tile.
        //
        // The highlight covers the name and not the tile. Painting the whole
        // tile width made a selected file's block run into its neighbour's
        // label, so two names looked like one — and it is not what a desktop
        // does either: there, the selection is the shape of the word.
        let name = fit(&e.name, TILE_W as usize - 2);
        let used = Span::raw(&name).width();
        let left = (TILE_W as usize - used) / 2;
        let right = TILE_W as usize - used - left;
        // The tile you are on wears the accent, the way the row does in the
        // detail view and the way every desktop marks a selection; a *marked*
        // tile keeps the theme's own tint, so the two states are told apart at
        // a glance instead of by counting.
        let tile_bg = if selected {
            th.accent
        } else {
            selection_on(bg.unwrap_or(th.popup_bg), th.selected_bg)
        };
        let mut style = Style::default().fg(text_tone(
            kind_for(e).color(),
            if selected || marked { tile_bg } else { bg.unwrap_or(th.popup_bg) },
        ));
        if selected || marked {
            style = style.bg(tile_bg).add_modifier(Modifier::BOLD);
            style = style.fg(text_tone(kind_for(e).color(), tile_bg));
        }
        let plain = Style::default().bg(bg.unwrap_or(th.popup_bg));
        let label_rect = Rect::new(cx, cy + TILE_ICON_H, TILE_W, 1);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ".repeat(left), plain),
                Span::styled(name, style),
                Span::styled(" ".repeat(right), plain),
            ])),
            label_rect,
        );
    }
    app.icon_slots.extend(slots);

    // How much grid there is, and where in it this page sits. The grid pages
    // rather than scrolls, so the thumb steps a page at a time — which is the
    // truth about how this view moves, and a bar that slid smoothly over a view
    // that jumps would be describing something else.
    // Whatever a previous frame's layout left behind is not on screen any more:
    // the grid owns the window, and this is the only track in it.
    app.scroll_tracks.clear();
    if total > per_page {
        app.scroll_tracks.push(crate::ScrollTrack {
            rect: bar,
            what: crate::ScrollWhat::Pane(which),
            total,
            shown: per_page,
        });
        let max = total.saturating_sub(per_page);
        for i in 0..BAR_W {
            f.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .thumb_symbol("█")
                    .thumb_style(
                        Style::default().fg(text_tone(th.accent, bg.unwrap_or(th.popup_bg))),
                    )
                    .track_symbol(Some("│"))
                    .track_style(Style::default().fg(th.border))
                    .begin_symbol(None)
                    .end_symbol(None),
                Rect::new(bar.x + i, bar.y, 1, bar.height),
                &mut ScrollbarState::new(max).position(start.min(max)),
            );
        }
    }
}

thread_local! {
    /// Where the popup on screen is, as of the last frame — the union of every
    /// rectangle a popup wiped before drawing itself.
    ///
    /// The mouse needs to know where "outside the popup" is, and no one owns
    /// that answer: two dozen popups each work out their own frame, and most of
    /// them are handed a `&mut Popup` rather than the whole of cian, so there is
    /// nowhere to put it on the way past. What they do all have in common is
    /// this: a popup begins by clearing the cells it is about to cover. So the
    /// clearing is what records it.
    ///
    /// Read by the mouse, which runs on the thread that draws — the loop is one
    /// thread in both front ends — between one frame and the next.
    static POPUP_INK: std::cell::Cell<Option<Rect>> = const { std::cell::Cell::new(None) };

    /// Whether the editor is answering to notepad keys, for the frame being
    /// drawn.
    ///
    /// Mirrored here rather than threaded down. The viewer is drawn four calls
    /// deep and not one of the layers in between owns an `App`, so carrying a
    /// bool through would mean widening four signatures to tell one badge what
    /// word to use. The theme and the popup's ink already reach the renderer
    /// this way, on the same thread and with the same lifetime — one frame.
    static NOTEPAD_KEYS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The editor grammar in force for the frame being drawn. See [`NOTEPAD_KEYS`].
fn notepad_keys() -> bool {
    NOTEPAD_KEYS.with(|c| c.get())
}

/// Wipe the cells a popup is about to draw over, and remember them as its own.
fn clear_popup(f: &mut Frame, rect: Rect) {
    f.render_widget(Clear, rect);
    POPUP_INK.with(|c| {
        c.set(Some(match c.get() {
            // A popup drawn in two pieces — a frame and a box inside it — is
            // one popup, and both pieces are inside it.
            Some(had) => had.union(rect),
            None => rect,
        }))
    });
}

/// The rectangle the popup on screen covers, if one is showing.
pub(crate) fn popup_ink() -> Option<Rect> {
    POPUP_INK.with(|c| c.get())
}

/// The popups that take the whole frame and end it. Returns whether one drew.
///
/// Each of these needs the application, not just its popup — they record where
/// they put things so a click can be turned back into a line, a row, a tile.
/// That is why they are not in [`draw_popup`] with the rest.
fn draws_its_own_frame(f: &mut Frame, area: Rect, app: &mut App) -> bool {
    match app.popup {
        Popup::AiChat { .. } => draw_ai_chat(f, area, app),
        Popup::AiHistory { .. } => draw_ai_history(f, area, app),
        Popup::Toggles { .. } => draw_toggles(f, area, app),
        Popup::OpQueue { .. } => draw_op_queue(f, area, app),
        // The image preview decodes to fit its box and caches by size.
        Popup::ImageView { .. } => draw_image(f, area, app),
        _ => {
            // The F3 image popup closed: drop its protocol state, and — for a
            // protocol whose pictures outlive the cells under them — wipe the
            // terminal once so it does not linger over what is now underneath.
            if app.img_proto.take().is_some() && app.needs_clear_after_image() {
                app.full_clear = true;
            }
            match app.popup {
                Popup::CommitMessage { .. } => draw_commit_message(f, area, app),
                Popup::JunkReview { .. } => draw_junk_review(f, area, app),
                Popup::DupeReview { .. } => draw_dupe_review(f, area, app),
                Popup::StructureReview { .. } => draw_structure_review(f, area, app),
                Popup::RenameReview { .. } => draw_rename_review(f, area, app),
                _ => return false,
            }
        }
    }
    true
}

/// How far the popup on top is scrolled, for the ones that scroll.
///
/// Part of the fingerprint that decides whether the surface needs wiping — a
/// manual scrolled by a line is as much of a change as a manual that just
/// opened, and leaves the same leftovers if the renderer is left to work it
/// out from the cells alone.
fn popup_scroll(popup: &Popup) -> usize {
    match popup {
        Popup::Manual { scroll, .. }
        | Popup::Report { scroll, .. }
        | Popup::Diff { scroll, .. }
        | Popup::DiskUsage { scroll, .. }
        | Popup::GitLog { scroll, .. }
        | Popup::FindResults { scroll, .. }
        | Popup::DirCompare { scroll, .. }
        | Popup::Archive { scroll, .. }
        | Popup::JunkReview { scroll, .. }
        | Popup::DupeReview { scroll, .. }
        | Popup::StructureReview { scroll, .. }
        | Popup::RenameReview { scroll, .. }
        | Popup::RemoteBrowser { scroll, .. }
        | Popup::AiChat { scroll, .. } => *scroll,
        _ => 0,
    }
}

/// Where a spinner is in its turn, `elapsed_ms` into it.
///
/// Braille — ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ — is what everything spins with, and it is the one
/// thing this cannot use. The window draws with exactly one font and no
/// fallback (deliberately: see cian-gui's `font.rs`), and not one of the
/// Japanese Nerd Fonts it looks for has the braille block in it — HackGen
/// Console NF, checked: 28,584 characters, U+2800 not among them. Ten frames
/// that are all the same missing glyph is a spinner that does not spin, which
/// is exactly how it was reported. The quarter-filled circles are in every font
/// on this machine — HackGen Console NF and Hack Nerd Font, both weights of
/// each — at the same advance as `m`, so they sit in one cell and read as one
/// thing turning.
///
/// Four frames rather than ten, at 120ms each: half a second to the turn, which
/// is a spinner rather than a flicker.
pub(crate) fn spinner_frame(elapsed_ms: u128) -> &'static str {
    const SPIN: [&str; 4] = ["◐", "◓", "◑", "◒"];
    SPIN[((elapsed_ms / 120) % SPIN.len() as u128) as usize]
}

fn draw_startup_splash(f: &mut Frame, area: Rect, elapsed_ms: u128) {
    let frame = spinner_frame(elapsed_ms);
    let w = 34u16.min(area.width);
    let h = 5u16.min(area.height);
    let rect = centered_rect(w, h, area);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(accent_on_popup())
        .style(Style::default().bg(theme().popup_bg).fg(readable_on(theme().popup_bg)));
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);
    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{}  ", frame),
                accent_on_popup(),
            ),
            Span::styled(
                "cian",
                accent_on_popup(),
            ),
            Span::styled("  starting up…", Style::default().fg(readable_on(theme().popup_bg))),
        ]),
        Line::from(Span::styled(
            "  checking AI helper (crmaine)…",
            Style::default().fg(muted_on(theme().popup_bg)).add_modifier(Modifier::ITALIC),
        )),
    ];
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

/// The viewer's text body rect, mirroring its renderer's geometry so a mouse
/// click maps to the right line.
/// The viewer's frame within `area` — the bordered box, whose top row carries
/// the title and the tab arrows.
pub(crate) fn viewer_frame_rect(area: Rect) -> Rect {
    centered_rect(area.width.saturating_sub(4), area.height.saturating_sub(2), area)
}

/// The frame, given whether the viewer is docked in a pane. A floating
/// viewer stands off the edges of the window; a docked one *is* the pane, so
/// it takes the rectangle exactly rather than drawing a second frame inside
/// the pane's own.
pub(crate) fn viewer_frame_rect_docked(area: Rect, docked: bool) -> Rect {
    if docked {
        area
    } else {
        viewer_frame_rect(area)
    }
}

fn viewer_body_rect(area: Rect, outline_w: u16, ruler_rows: u16) -> Rect {
    viewer_body_rect_docked(area, outline_w, ruler_rows, false)
}

fn viewer_body_rect_docked(area: Rect, outline_w: u16, ruler_rows: u16, docked: bool) -> Rect {
    let rect = viewer_frame_rect_docked(area, docked);
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    let body_h = inner.height.saturating_sub(1 + ruler_rows);
    Rect::new(inner.x + outline_w, inner.y + ruler_rows, inner.width - outline_w, body_h)
}

/// How wide the outline column is, or 0 when it is not showing.
///
/// Shared by the renderer and the mouse handler: a click has to land on the
/// entry that was drawn there, and two copies of this arithmetic would drift.
pub(crate) fn outline_width(inner_w: u16, show: bool, items: usize) -> u16 {
    // 90 rather than 60 since a file can be docked in a pane: half a window
    // is wide enough to *fit* the column and not wide enough to afford it —
    // it was taking a third of the text away. `:outline` still asks for it,
    // and the full-window viewer is past the bar in any ordinary terminal.
    if show && items > 0 && inner_w >= 90 { 28u16.min(inner_w / 3) } else { 0 }
}

/// The source line a viewer line stands for.
///
/// In source mode they are the same number. In the Markdown preview they are
/// not, and everything that reads the *file* (the outline) has to be told
/// which of the two it is holding before it compares it with anything that
/// reads the *screen* (the cursor).
pub(crate) fn src_line(md_map: &[usize], line: usize) -> usize {
    md_map.get(line).copied().unwrap_or(line)
}

/// The viewer line showing source line `src` — the trip back.
///
/// A rendered block often opens with a blank line for spacing, and that blank
/// belongs to the same source line as the heading under it. Landing on the
/// blank is landing one line short of what was asked for, so the first line
/// with something on it wins.
pub(crate) fn disp_line(md_map: &[usize], lines: &[String], src: usize) -> usize {
    if md_map.is_empty() {
        return src;
    }
    let first = md_map.iter().position(|s| *s >= src).unwrap_or(md_map.len().saturating_sub(1));
    let mut i = first;
    while i + 1 < md_map.len()
        && md_map[i + 1] == md_map[first]
        && lines.get(i).is_some_and(|l| l.trim().is_empty())
    {
        i += 1;
    }
    i
}

/// The first outline entry drawn, given where the cursor is and how many rows
/// there is room for.
pub(crate) fn outline_top(items: &[cian_core::outline::Item], line: usize, h: usize) -> usize {
    match items.iter().rposition(|i| i.line <= line) {
        Some(i) if h > 0 && i >= h => i + 1 - h,
        _ => 0,
    }
}

/// The rect the context menu occupies, from its anchor and item count. Shared
/// by the renderer and the mouse handler so a click lands where the row is
/// drawn.
/// What to call a sort key on screen.
///
/// One place, because there were three and only one of them was translated:
/// the column heading said 日時, the picker said `date`, and the message after
/// sorting said `sorted by date (ascending)` in a Japanese window. The
/// [`cian_core::SortKey::label`] behind those last two is the wire name — it
/// goes over the pipe to the windowed build — and a wire name is not a label.
pub(crate) fn sort_label(key: cian_core::SortKey, lang: Lang) -> &'static str {
    use cian_core::SortKey;
    match key {
        SortKey::Name => tr(lang, "Name", "名前"),
        SortKey::Size => tr(lang, "Size", "サイズ"),
        SortKey::Modified => tr(lang, "Date", "日時"),
        SortKey::Extension => tr(lang, "Extension", "拡張子"),
    }
}

/// Split a menu label into (name, hint), where the hint is a trailing
/// `(…)`-style key/command annotation preceded by two spaces (e.g.
/// `"Rename by pattern…  (:renamepattern)"` → name and hint). No hint
/// yields an empty second element.
pub(crate) fn menu_label_parts(label: &str) -> (&str, &str) {
    if label.ends_with(')') {
        if let Some(pos) = label.rfind("  (") {
            return (label[..pos].trim_end(), &label[pos + 2..]);
        }
    }
    (label, "")
}

/// The widest name and widest hint across a menu's items — so names left-align
/// and hints right-align in a common column.
fn menu_dims(items: &[MenuItem], lang: Lang) -> (usize, usize) {
    let mut name_w = 0;
    let mut hint_w = 0;
    for i in items {
        let (n, h) = menu_label_parts(i.label(lang));
        name_w = name_w.max(width(n));
        hint_w = hint_w.max(width(h));
    }
    (name_w.max(6), hint_w)
}

/// The text-input field, with the cursor shown as a highlighted character
/// (reverse video) so moving it never shifts the text around it. A password is
/// masked; a cursor at the end highlights a trailing space (a block cursor).
///
/// One row per typed line, because the AI prompts take a newline now
/// (Shift+Enter): "what do you want the command to do" is a sentence or three,
/// not a filename. Only the first row wears the `>`; the rest are indented by
/// one so the whole field reads as a block. Wrapping the *long* rows is the
/// paragraph's job, and the box is sized for it above.
fn caret_lines(buffer: &str, cursor: usize, secret: bool, selected: bool) -> Vec<Line<'static>> {
    let shown: String = if secret { "•".repeat(buffer.chars().count()) } else { buffer.to_string() };
    let lead = |i: usize| Span::raw(if i == 0 { ">" } else { " " });
    // Select-all: the whole value reversed out, so "the next key replaces this"
    // is visible rather than something you have to remember having pressed.
    if selected && !shown.is_empty() {
        let hl = Style::default().fg(readable_on(theme().accent)).bg(theme().accent);
        return shown
            .split('\n')
            .enumerate()
            .map(|(i, seg)| Line::from(vec![lead(i), Span::styled(seg.to_string(), hl)]))
            .collect();
    }
    let cur = cursor.min(shown.chars().count());
    let mut out = Vec::new();
    // Chars consumed by the rows already emitted, counting the `\n` between
    // them, so the caret's index into the whole buffer can be resolved to a
    // row and an offset within it.
    let mut seen = 0usize;
    let mut placed = false;
    for (i, seg) in shown.split('\n').enumerate() {
        let chars: Vec<char> = seg.chars().collect();
        // The first row that could contain it does. `<=` rather than `<` so a
        // caret sitting at the end of a row is drawn there, on the space after
        // the last character, rather than falling between two rows and
        // vanishing.
        if !placed && cur <= seen + chars.len() {
            placed = true;
            let at_i = cur - seen;
            let before: String = chars[..at_i].iter().collect();
            let at: String =
                chars.get(at_i).map(|c| c.to_string()).unwrap_or_else(|| " ".to_string());
            let after: String =
                chars.get(at_i + 1..).map(|s| s.iter().collect()).unwrap_or_default();
            out.push(Line::from(vec![
                lead(i),
                Span::raw(before),
                Span::styled(
                    at,
                    Style::default().fg(readable_on(theme().accent)).bg(theme().accent),
                ),
                Span::raw(after),
            ]));
        } else {
            out.push(Line::from(vec![lead(i), Span::raw(seg.to_string())]));
        }
        seen += chars.len() + 1;
    }
    out
}


fn context_menu_rect(items: &[MenuItem], at: (u16, u16), area: Rect, lang: Lang) -> Rect {
    // marker(2) + name + gap(2, if any hint) + hint + right gutter(2) + borders(2).
    let (name_w, hint_w) = menu_dims(items, lang);
    let hint_col = if hint_w > 0 { hint_w + 2 } else { 0 };
    let w = (2 + name_w + hint_col + 2 + 2) as u16;
    let h = items.len() as u16 + 2;
    let x = at.0.min(area.width.saturating_sub(w));
    let y = at.1.min(area.height.saturating_sub(h));
    Rect::new(x, y, w.min(area.width), h.min(area.height))
}

/// Build a tab strip. Active tab uses full path; inactive tabs use just the
/// directory name. If the labels overflow `max_width`, the rest collapse into
/// a `+N` marker so the active tab stays visible.
fn tabs_title<'a>(
    tabs: &'a PaneTabs,
    focused: bool,
    focus_bg: Color,
    max_width: u16,
    // Filled with (tab index, column offset from the title's start, width) for
    // each visible tab, so a click can be mapped back to a tab.
    offsets: &mut Vec<(usize, u16, u16)>,
) -> (Line<'a>, String) {
    fn label_for(i: usize, tab: &Pane, is_active: bool) -> String {
        // A remote pane shows "⇅ user@host:/path" so it reads as a server.
        if let Some((host, path)) = tab.remote_view() {
            return format!(" {} ⇅ {}:{} ", i + 1, host, path);
        }
        // Inside an archive: "⊞ report.zip/sub/" so the pane reads as a
        // place inside a file, not a directory.
        if let Some((arc, sub)) = tab.archive_view() {
            let name = arc.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            return format!(" {} ⊞ {}/{} ", i + 1, name, sub);
        }
        // A flat / search listing names the view (e.g. "⌥ branch", "⌥ grep: x")
        // rather than a directory, so it is obvious the pane is not a folder and
        // that `b` / Esc leaves it.
        if let Some(lbl) = tab.flat_label() {
            // …and says how, because "the pane is not a folder any more" is
            // easy to notice and "Esc puts it back" is not.
            return format!(" {} ⌥ {}  ⏎Esc/⇦ ", i + 1, lbl);
        }
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
    // Display cells, not characters: a Japanese directory name is two cells a
    // character, and the char count both overflowed the layout budget and
    // misplaced the click map.
    let width_of = |s: &str| width(s) as u16;

    // The two history arrows eat four cells at the head of the title, so the
    // tabs get that much less to lay out in. Forgetting this is how the long
    // path started being clipped at the right edge again.
    const NAV_W: u16 = 4;
    let max_width = max_width.saturating_sub(NAV_W);
    // First, lay out tabs starting from the active one outward so it never gets cut.
    let active = tabs.active.min(tabs.tabs.len().saturating_sub(1));
    let total = tabs.tabs.len();
    let mut shown: Vec<usize> = vec![active];
    // A long path is shortened from the middle — its tail is the part that
    // identifies it, and clipping at the border loses exactly that end.
    let active_label = truncate_middle(
        &label_for(active, &tabs.tabs[active], true),
        max_width.saturating_sub(2) as usize,
    );
    let mut used: u16 = width_of(&active_label);
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
    // Track the running column offset so each tab's on-screen span is known.
    let mut col: u16 = 1; // the leading space below
    spans.push(Span::raw(" "));
    // Browser arrows, before the tabs: lit when there is somewhere to go.
    // Their rects are pushed by the caller, which knows the pane's origin.
    {
        let active = &tabs.tabs[tabs.active.min(tabs.tabs.len().saturating_sub(1))];
        let lit = accent_on_popup();
        let out = Style::default().fg(theme().dim);
        spans.push(Span::styled(
            "◀",
            if active.history.is_empty() { out } else { lit },
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            "▶",
            if active.forward.is_empty() { out } else { lit },
        ));
        spans.push(Span::raw(" "));
        col += NAV_W;
    }
    if hidden_left > 0 {
        let s = format!("+{} ", hidden_left);
        col += width_of(&s);
        spans.push(Span::styled(s, Style::default().fg(dim_text(surface()))));
    }
    for (pos, &i) in shown.iter().enumerate() {
        let is_active = i == active;
        let style = if is_active {
            if focused {
                Style::default().fg(readable_on(focus_bg)).bg(focus_bg).add_modifier(Modifier::BOLD)
            } else {
                // Active but unfocused: an accent-tinted bar so it stays legible
                // whatever the pane background is (DarkGray vanished on some).
                Style::default().fg(readable_on(theme().border)).bg(theme().border).add_modifier(Modifier::BOLD)
            }
        } else {
            // Inactive tabs: a readable mid grey from the theme, not DarkGray,
            // which was the same tone as some backgrounds.
            Style::default().fg(dim_text(surface())).add_modifier(Modifier::BOLD)
        };
        let label = if is_active {
            active_label.clone()
        } else {
            label_for(i, &tabs.tabs[i], is_active)
        };
        let w = width_of(&label);
        offsets.push((i, col, w));
        col += w;
        spans.push(Span::styled(label, style));
        if pos + 1 < shown.len() {
            spans.push(Span::styled("│", Style::default().fg(theme().dim)));
            col += 1;
        }
    }
    if hidden_right > 0 {
        spans.push(Span::styled(
            format!(" +{}", hidden_right),
            Style::default().fg(dim_text(surface())),
        ));
    }
    spans.push(Span::raw(" "));
    (Line::from(spans), active_label)
}

/// Pick a Nerd Font glyph based on the entry name/extension.
pub(crate) fn icon_for(entry: &cian_core::Entry) -> &'static str {
    // Without a Nerd Font, drop the icons entirely — directory colour still
    // marks folders, and no glyph mojibakes on a plain terminal.
    if !crate::theme::nerd_fonts() {
        return "";
    }
    // The synthetic `..` row gets an up-level arrow so it reads as navigation,
    // not as a folder that happens to be called "..".
    if entry.is_parent {
        return "\u{f062}"; // arrow-up
    }
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

/// Broad kinds of file, used to color the listing.
///
/// Deliberately coarse: the point is that a glance separates "code" from
/// "archive" from "image", not that every extension gets its own hue. Too many
/// colors read as noise rather than structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Directory,
    Code,
    Config,
    Document,
    Image,
    Media,
    Archive,
    Executable,
    /// Dotfiles and other things that are usually background noise.
    Muted,
    Plain,
}

impl FileKind {
    fn color(self) -> Color {
        // From the active theme's palette, so a light theme recolors the whole
        // set at once rather than fighting these fixed values.
        let p = theme().file;
        match self {
            FileKind::Directory => p.directory,
            FileKind::Code => p.code,
            FileKind::Config => p.config,
            FileKind::Document => p.document,
            FileKind::Image => p.image,
            FileKind::Media => p.media,
            FileKind::Archive => p.archive,
            FileKind::Executable => p.executable,
            FileKind::Muted => p.muted,
            FileKind::Plain => p.plain,
        }
    }

    fn bold(self) -> bool {
        matches!(self, FileKind::Directory | FileKind::Executable)
    }
}

/// Classify an entry for coloring. Mirrors the categories [`icon_for`] draws
/// from, so a file's icon and its color always agree.
fn kind_for(entry: &cian_core::Entry) -> FileKind {
    if entry.is_dir {
        return FileKind::Directory;
    }
    // Dotfiles recede: they are rarely the thing being looked for.
    if entry.name.starts_with('.') {
        return FileKind::Muted;
    }
    let ext = std::path::Path::new(&entry.name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "rs" | "py" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "go" | "c" | "h" | "cpp"
        | "cc" | "cxx" | "hpp" | "java" | "rb" | "php" | "lua" | "swift" | "kt" | "kts"
        | "vue" | "svelte" | "html" | "htm" | "css" | "scss" | "sass" | "less" => FileKind::Code,
        "toml" | "ini" | "conf" | "cfg" | "yaml" | "yml" | "json" | "jsonc" | "xml" | "env" => {
            FileKind::Config
        }
        "md" | "markdown" | "txt" | "log" | "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt"
        | "pptx" | "rtf" | "csv" | "tsv" => FileKind::Document,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" | "ico" | "tif" | "tiff" => {
            FileKind::Image
        }
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "mp4" | "mov" | "mkv" | "avi" | "webm"
        | "wmv" => FileKind::Media,
        "zip" | "tar" | "gz" | "7z" | "rar" | "bz2" | "xz" | "zst" | "tgz" => FileKind::Archive,
        "exe" | "msi" | "bat" | "cmd" | "ps1" | "sh" | "bash" | "zsh" | "fish" | "app"
        | "dll" | "so" | "dylib" => FileKind::Executable,
        _ => FileKind::Plain,
    }
}

fn shell_tabs_title<'a>(
    tabs: &'a ShellPane,
    focused: bool,
    offsets: &mut Vec<(usize, u16, u16)>,
) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut col: u16 = 1; // the leading space below
    spans.push(Span::raw(" "));
    for i in 0..tabs.count().max(1) {
        // Its name where it has one. A strip of `shell 1`..`shell 4` is a
        // strip you have to open every tab to read.
        let label = match tabs.tab_name(i) {
            Some(n) if !n.is_empty() => format!(" {n} "),
            _ => format!(" shell {} ", i + 1),
        };
        let style = if i == tabs.active {
            if focused {
                Style::default().fg(readable_on(theme().accent)).bg(theme().accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(readable_on(theme().selected_bg))
                    .bg(theme().selected_bg)
            }
        } else {
            // Measured against the surface it sits on, like everything else.
            // `Color::Gray` is the terminal's palette entry 8, which a
            // Solarized profile maps to the *background* — the label was
            // drawn, clickable, and invisible.
            Style::default().fg(muted_on(surface()))
        };
        let w = label.chars().count() as u16;
        offsets.push((i, col, w));
        col += w;
        spans.push(Span::styled(label, style));
        if i + 1 < tabs.count() {
            spans.push(Span::styled("│", Style::default().fg(theme().border)));
            col += 1;
        }
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

#[allow(clippy::too_many_arguments)]
fn draw_file_pane(
    f: &mut Frame,
    area: Rect,
    tabs: &mut PaneTabs,
    tracks: &mut Vec<crate::ScrollTrack>,
    focused: bool,
    visual_anchor: Option<usize>,
    mode: Mode,
    bg: Option<Color>,
    flash: f32,
    pane_id: FocusedPane,
    tab_rects: &mut Vec<(FocusedPane, usize, Rect)>,
    git: Option<&cian_core::git::RepoStatus>,
    lang: Lang,
    sort_rects: &mut Vec<(FocusedPane, cian_core::SortKey, Rect)>,
    crumb_rects: &mut Vec<(FocusedPane, usize, Rect)>,
    nav_rects: &mut Vec<(FocusedPane, bool, Rect)>,
    skin: Skin,
    native_icons: bool,
    icon_slots: &mut Vec<crate::IconSlot>,
) {
    // Read the active theme once — `theme()` now takes a lock, and the row loop
    // below would otherwise hit it thousands of times per frame.
    let th = theme();
    let finder = skin == Skin::Finder;
    // The focused pane announces itself with a coloured tab. In the desktop
    // look that colour is the loudest thing on a near-white screen, and it is
    // announcing the wrong thing — the path matters, the fact that this pane
    // has the focus is already said by the selection. So it becomes chrome, and
    // the tab reads as a breadcrumb: dark text on a light chip.
    let focus_bg = if finder { th.status_bg } else { focus_badge_color(mode) };
    let bg = bg.or(th.base_bg);
    let mut border_style = if focused {
        Style::default().fg(focus_bg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.border)
    };
    // An operation that just landed here lights the border, fading out.
    if flash > 0.0 {
        border_style = Style::default().fg(fade(th.accent, flash)).add_modifier(Modifier::BOLD);
    }
    // A remote (SFTP) pane wears a carmine frame, so "this is a server, not the
    // local disk" is unmistakable regardless of focus.
    if tabs.active_ref().is_remote() {
        border_style = Style::default().fg(CRMAINE).add_modifier(Modifier::BOLD);
    }
    // The window the listing will show, settled before anything reads it — see
    // the note by `start` below.
    {
        let list_h = area.height.saturating_sub(3) as usize;
        let p = tabs.active_mut();
        p.scroll = clamp_list_scroll(p.scroll, p.cursor, list_h, p.entries.len());
    }
    let max_title_w = area.width.saturating_sub(2);
    let mut offsets = Vec::new();
    let (title, active_title) = tabs_title(tabs, focused, focus_bg, max_title_w, &mut offsets);
    // The two history arrows sit at columns 1 and 3 of the title.
    nav_rects.push((pane_id, false, Rect::new(area.x + 2, area.y, 1, 1)));
    nav_rects.push((pane_id, true, Rect::new(area.x + 4, area.y, 1, 1)));
    // The title is drawn on the top border row, one cell in from the corner.
    for (i, off, w) in &offsets {
        tab_rects.push((pane_id, *i, Rect::new(area.x + 1 + off, area.y, *w, 1)));
    }
    // Finder draws no box. The pane still needs its title row and its one-cell
    // side gutters — the layout below is written in terms of them — so it keeps
    // the top and the sides and loses the strokes: a border whose colour is the
    // background it sits on is a border nobody can see. What separates the two
    // panes is then the gutter itself, which is how a desktop file manager does
    // it too.
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(if finder {
            Style::default().fg(bg.unwrap_or(th.popup_bg))
        } else {
            border_style
        })
        .title(title);
    if let Some(c) = bg {
        block = block.style(Style::default().bg(c));
    }

    let pane = tabs.active_ref();
    // A remote or in-archive listing has paths that mean nothing to this disk.
    let synthetic = pane.is_synthetic();
    let visual_range = visual_anchor.map(|a| {
        if a <= pane.cursor { (a, pane.cursor) } else { (pane.cursor, a) }
    });

    // Columns are dropped progressively on narrow panes so the name always
    // keeps a usable amount of room.
    let inner_w = area.width.saturating_sub(2);
    let show_time = inner_w >= 52;
    let show_size = inner_w >= 34;
    // A git badge column (badge + space) only when the pane sits in a repo.
    let git_w: u16 = if git.is_some() { 2 } else { 0 };
    // Likewise a ☁ column, only where a sync client actually put placeholders:
    // an ordinary folder never pays a cell for it.
    let cloud_w: u16 = if pane.has_cloud() { 2 } else { 0 };
    let meta_w = if show_time { SIZE_COL_W + TIME_COL_W + 2 } else if show_size { SIZE_COL_W + 1 } else { 0 };
    // 2 mark + icon + 2 spaces
    let name_w = inner_w.saturating_sub(meta_w + 5 + git_w + cloud_w) as usize;

    // Only the rows the viewport can actually show are touched: per-frame work
    // is O(visible), not O(entries), which is the difference between a snappy
    // and a sluggish pane on a directory with thousands of files.
    let total = pane.entries.len();
    // Borders top+bottom, plus the column-header row under the top border.
    let list_h = area.height.saturating_sub(3) as usize;
    // The window follows the cursor only when the cursor would leave it. It
    // used to be derived from the cursor with a formula that put the cursor
    // on the *last* visible row, so clicking a file — or jumping to one —
    // scrolled it to the bottom of the pane.
    // Written back, not just computed.
    //
    // The window the listing shows is `scroll`, and the mouse turns a click
    // into a row with `scroll + offset`. The classic view kept the two in step
    // by assigning it before drawing; the detail view did not, so its `scroll`
    // stayed at zero however far the listing had been walked — and a click
    // twenty rows down the *screen* selected the twentieth file in the
    // *listing*, which then dragged the view back to the top. One owner, at
    // the one place that knows both the pane and the height it is drawn in.
    let start = pane.scroll;
    let end = start.saturating_add(list_h).min(total);
    let mark_style = Style::default().fg(th.mark_fg).add_modifier(Modifier::BOLD);
    // Per row, because the cursor's row has a tint of its own and a dim grey
    // that reads on the page can vanish on it.
    // The size and date columns, dimmed against whatever they land on — the
    // page, or the selection, which in the desktop views is the accent.
    let sel_bg_for_meta = if finder {
        th.accent
    } else {
        selection_on(bg.unwrap_or(th.popup_bg), th.selected_bg)
    };
    let meta_on = |selected: bool| {
        Style::default()
            .fg(dim_text(if selected { sel_bg_for_meta } else { bg.unwrap_or(th.popup_bg) }))
    };

    // Where the listing starts, in absolute cells: one in for the border. The
    // row is `y`, which the painter below already has.
    let list_x = area.x + 1;

    // An unfocused pane recedes so the focused one reads as the active surface.
    let mut list_style = if focused {
        Style::default()
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };
    if let Some(c) = bg {
        list_style = list_style.bg(c);
    }
    // The frame and the rows render separately so the column-header row can
    // sit between them: block on the full area, header on the first inner
    // row, the list below it.
    f.render_widget(block, area);
    let inner = area.inner(Margin { vertical: 1, horizontal: 1 });
    if inner.height > 0 {
        let header = Rect::new(inner.x, inner.y, inner.width, 1);
        draw_pane_header(
            f, header, pane, git_w, cloud_w, name_w, show_size, show_time, list_style, pane_id,
            lang, sort_rects,
        );
    }
    let list_area =
        Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(1));

    // The rows are painted straight into the buffer, rather than built as
    // widgets and handed to `List`.
    //
    // This is the innermost loop cian has, and the widget road to it is paved
    // with allocation: a `String` per column, a `Span` per string, a `Vec` of
    // them per `Line`, a `ListItem` around that, and a `Vec` of those — then
    // every one of those strings segmented into graphemes on the way to the
    // cells. Measured at 200x60 with two full panes, that was about two thirds
    // of a frame. Painting cells directly keeps every one of the decisions
    // above — the same columns, colours, banding and selection — and does none
    // of the packaging. See [`put`] for the one that matters: a file listing is
    // overwhelmingly ASCII, and ASCII needs no segmenter.
    // What "this one" looks like.
    //
    // The theme's `selected_bg` is a tint — right for the classic view, where
    // the cursor row sits among borders and the pane's own focus colour says
    // which side is live. In the desktop views there is one pane and no
    // border, and on a dark theme that tint is a shade away from the page: the
    // selection was reported as hard to find on dark, and merely findable on
    // light. So there the selection is the accent itself, filled, the way
    // every desktop file manager marks the row you are on.
    let sel_bg = if finder {
        th.accent
    } else {
        selection_on(bg.unwrap_or(th.popup_bg), th.selected_bg)
    };
    let selected_style = Style::default().bg(sel_bg).add_modifier(Modifier::BOLD);
    let buf = f.buffer_mut();
    // The whole area first, so the rows below the last file carry the pane's
    // background rather than whatever was behind it.
    buf.set_style(list_area, list_style);
    for (vi, e) in pane.entries[start..end].iter().enumerate() {
        let y = list_area.y + vi as u16;
        if y >= list_area.bottom() {
            break;
        }
        let i = start + vi; // absolute index for marks / visual range / git
        let selected_row = i == pane.cursor;
        let marked = pane.is_marked(i);
        let in_visual = visual_range.map(|(a, b)| i >= a && i <= b).unwrap_or(false);

        // The row's own background: banded, in visual range, or selected.
        // Banding uses the absolute index so the stripes stay put while the
        // list scrolls under them.
        let row = Rect::new(list_area.x, y, list_area.width, 1);
        if selected_row {
            buf.set_style(row, selected_style);
        } else if in_visual {
            buf.set_style(row, Style::default().bg(th.visual_bg));
        } else if finder && i % 2 == 1 {
            buf.set_style(row, Style::default().bg(th.popup_bg));
        }

        let kind = kind_for(e);
        // Fitted to the row it lands on: the same colour reads differently on
        // the page and on the selection, and a light theme's palette is close
        // enough to its own selection tint to disappear into it.
        let kind_color = text_tone(kind.color(), if selected_row { sel_bg } else { bg.unwrap_or(th.popup_bg) });
        let mut name_style = Style::default().fg(kind_color);
        if kind.bold() {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        // The icon carries the same color so the row reads as one unit.
        let icon_style = Style::default().fg(kind_color);
        let meta_style = meta_on(selected_row);

        let end_x = row.right();
        let mut x = row.x;
        if git.is_some() {
            let (badge, color) = git
                .and_then(|g| g.mark_for(&e.path))
                .map(|m| (m.badge(), git_mark_color(m)))
                .unwrap_or(("", Color::Reset));
            // Padded without formatting: the badge is one character or none.
            let badge: &str = match badge {
                "" => "  ",
                "M" => "M ",
                "A" => "A ",
                "D" => "D ",
                "R" => "R ",
                "?" => "? ",
                "!" => "! ",
                _ => "* ",
            };
            put(buf, x, y, end_x, badge, Style::default().fg(color).add_modifier(Modifier::BOLD));
            x = (x + git_w).min(end_x);
        }
        if cloud_w > 0 {
            // Listed but not downloaded; a space keeps local files aligned.
            put(
                buf,
                x,
                y,
                end_x,
                if e.cloud { cloud_mark() } else { "  " },
                Style::default().fg(Color::Rgb(130, 175, 210)),
            );
            x = (x + cloud_w).min(end_x);
        }
        put(buf, x, y, end_x, if marked { "● " } else { "  " }, mark_style);
        x = (x + 2).min(end_x);

        // The icon is either a glyph cian draws, or two blank cells and a note
        // saying "a picture goes here" — see [`crate::IconSlot`]. Two cells
        // rather than one because a cell is about twice as tall as it is wide,
        // so two of them are roughly the square an icon wants.
        if native_icons {
            // Carried whatever the skin. In the classic view it *is* the icon;
            // in the detail view it is what gets drawn when the system has no
            // icon of its own to offer.
            let glyph = icon_for(e).chars().next().map(|c| (c, rgb_of(kind_color)));
            icon_slots.push(crate::IconSlot {
                x: list_x + git_w + cloud_w + 2,
                y,
                w: 2,
                h: 1,
                path: e.path.clone(),
                is_dir: e.is_dir,
                local: !synthetic,
                glyph,
                prefer_glyph: skin != Skin::Finder,
            });
        } else {
            put(buf, x, y, end_x, icon_for(e), icon_style);
        }
        x = (x + 3).min(end_x);

        // The name is written where it starts and the column is stepped over
        // whole, so a short name needs no padding written after it — the row's
        // background is already there.
        let name_end = (x + name_w as u16).min(end_x);
        put(buf, x, y, name_end, truncate_to(&e.name, name_w).as_ref(), name_style);
        x = name_end;

        if show_size {
            // Directories have no meaningful byte count; the `..` row shows none.
            let s: std::borrow::Cow<str> = if e.is_parent {
                std::borrow::Cow::Borrowed("")
            } else if e.is_dir {
                std::borrow::Cow::Borrowed("—")
            } else {
                std::borrow::Cow::Owned(cian_core::human_size(e.len))
            };
            // Right-aligned by starting it where it should end, rather than by
            // building a padded string to write.
            let w = crate::util::width(&s) as u16;
            let col = SIZE_COL_W;
            let at = x + 1 + col.saturating_sub(w);
            put(buf, at, y, (x + 1 + col).min(end_x), &s, meta_style);
            x = (x + 1 + col).min(end_x);
        }
        if show_time {
            // Formatting a date costs about a microsecond — chrono resolves the
            // local zone for each one — and a listing shows the same handful of
            // timestamps on every frame it is on screen. Memoised, it costs a
            // hash lookup. See [`cian_core::format_time_cached`].
            let t: std::borrow::Cow<str> = if e.is_parent {
                std::borrow::Cow::Borrowed("")
            } else {
                e.modified
                    .map(|m| std::borrow::Cow::Owned(cian_core::format_time_cached(m)))
                    .unwrap_or(std::borrow::Cow::Borrowed("-"))
            };
            put(buf, x + 1, y, end_x, &t, meta_style);
        }
    }

    // The scrollbar sits on the pane's right border and takes its style from
    // it, which is right when there is a border.
    let scroll_style =
        if finder { Style::default().fg(th.border) } else { border_style };
    draw_list_scrollbar(f, area, pane.entries.len(), pane.cursor, pane.scroll, focused, finder, scroll_style, pane_id, tracks);

    // The active tab's path segments are click targets (a breadcrumb): the
    // rects live on the title row and are resolved before tab selection.
    if let Some((ix, tab_col, _)) =
        offsets.iter().copied().find(|(i, _, _)| *i == tabs.active)
    {
        // Labels start one cell in from the corner, like the tab rects above.
        push_breadcrumb_rects(&active_title, ix, area, tab_col + 1, pane, pane_id, crumb_rects);
    }
}

/// Drop the icons a dialog is standing on, and only those.
///
/// The context menu records where it landed, so the listing behind it keeps
/// every icon the menu is not covering. Any other dialog could be anywhere, so
/// all of them go — an icon floating over a confirmation box is worse than an
/// icon missing for as long as the box is up.
fn hide_icons_under_popup(app: &mut App) {
    let over = app.menu_rect;
    let is_menu = matches!(app.popup, Popup::ContextMenu { .. });
    if !is_menu || over.width == 0 || over.height == 0 {
        app.icon_slots.clear();
        return;
    }
    app.icon_slots.retain(|s| {
        let (x0, x1) = (s.x, s.x + s.w);
        let (y0, y1) = (s.y, s.y + s.h);
        x1 <= over.x || x0 >= over.right() || y1 <= over.y || y0 >= over.bottom()
    });
}

/// Write one run of text into the buffer at `(x, y)`, clipped at `end_x`.
///
/// The fast path is the whole point. A file listing is overwhelmingly ASCII,
/// and for ASCII every byte is one character in one cell — no grapheme
/// clusters to find, no widths to look up, no combining marks to join to what
/// came before. `Buffer::set_stringn` cannot know that and segments every
/// string it is given; here it is asked only about the rows that need it.
///
/// Control characters are drawn as a space rather than sent to the cell: a
/// filename may contain one, and a terminal handed a `\t` moves the cursor.
fn put(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, end_x: u16, text: &str, style: Style) -> u16 {
    if x >= end_x || text.is_empty() {
        return x;
    }
    if text.is_ascii() {
        let mut x = x;
        for &b in text.as_bytes() {
            if x >= end_x {
                break;
            }
            let c = if (0x20..0x7f).contains(&b) { b as char } else { ' ' };
            let cell = &mut buf[(x, y)];
            cell.set_char(c);
            cell.set_style(style);
            x += 1;
        }
        return x;
    }
    buf.set_stringn(x, y, text, (end_x - x) as usize, style).0
}

/// [`crate::util::truncate`], without the allocation when nothing is cut.
///
/// Most names fit the column they are given, and the old shape copied every one
/// of them into a fresh `String` to say so.
fn truncate_to(s: &str, w: usize) -> std::borrow::Cow<'_, str> {
    if crate::util::width(s) <= w {
        std::borrow::Cow::Borrowed(s)
    } else {
        std::borrow::Cow::Owned(crate::util::truncate(s, w))
    }
}

/// The column-header row: `Name`, `Size`, `Date` over their columns, the
/// active sort key carrying a direction arrow. Each label's rect is pushed to
/// `sort_rects` so a click sorts by that column (repeat flips the direction,
/// as column headers behave everywhere else — `apply_sort_key` does the flip).
#[allow(clippy::too_many_arguments)]
fn draw_pane_header(
    f: &mut Frame,
    header: Rect,
    pane: &Pane,
    git_w: u16,
    cloud_w: u16,
    name_w: usize,
    show_size: bool,
    show_time: bool,
    base: Style,
    pane_id: FocusedPane,
    lang: Lang,
    sort_rects: &mut Vec<(FocusedPane, cian_core::SortKey, Rect)>,
) {
    use cian_core::SortKey;
    let style = base.fg(dim_text(surface()));
    let label = |key: SortKey| -> String {
        let name = if key == SortKey::Extension { "" } else { sort_label(key, lang) };
        if pane.sort.key == key {
            format!("{} {}", name, if pane.sort.reverse { "▼" } else { "▲" })
        } else {
            name.to_string()
        }
    };
    // Mirror the row layout: git badge, mark, icon columns, then the fields.
    // The icon column exists only with Nerd Fonts on (one cell + two spaces;
    // two spaces alone otherwise).
    let prefix = 4 + usize::from(nerd_fonts()) + git_w as usize + cloud_w as usize;
    let name_lbl = label(SortKey::Name);
    let size_lbl = label(SortKey::Size);
    let time_lbl = label(SortKey::Modified);
    let mut text = format!("{}{}", " ".repeat(prefix), pad_to(&name_lbl, name_w));
    if show_size {
        text.push_str(&pad_left(&size_lbl, SIZE_COL_W as usize + 1));
    }
    if show_time {
        text.push_str(&format!(" {}", time_lbl));
    }
    f.render_widget(Paragraph::new(text).style(style), header);

    // Click zones, in the same geometry the text was laid out in.
    let x = header.x + prefix as u16;
    sort_rects.push((pane_id, SortKey::Name, Rect::new(x, header.y, width(&name_lbl) as u16, 1)));
    if show_size {
        let sx = header.x + (prefix + name_w) as u16;
        sort_rects.push((pane_id, SortKey::Size, Rect::new(sx, header.y, SIZE_COL_W + 1, 1)));
    }
    if show_time {
        let tx = header.x + (prefix + name_w + SIZE_COL_W as usize + 2) as u16;
        sort_rects.push((
            pane_id,
            SortKey::Modified,
            Rect::new(tx, header.y, width(&time_lbl) as u16, 1),
        ));
    }
}

/// Map the displayed path segments of the active tab to click rects, counted
/// from the path's end. Counting from the end keeps the mapping exact even
/// when the head was middle-truncated: the tail of `truncate_middle` is
/// verbatim, so everything right of the `…` is trustworthy — and the segment
/// holding the `…` itself is ambiguous, so it gets no rect.
fn push_breadcrumb_rects(
    label: &str,
    active_ix: usize,
    area: Rect,
    tab_col: u16,
    pane: &Pane,
    pane_id: FocusedPane,
    crumb_rects: &mut Vec<(FocusedPane, usize, Rect)>,
) {
    // Only a plain directory listing has a browsable path.
    if pane.remote_view().is_some() || pane.flat_label().is_some() || pane.archive_view().is_some() {
        return;
    }
    // The label opens with " N " (the tab number) — that part is a tab click,
    // not a path segment, so parsing starts after it.
    let prefix = format!(" {} ", active_ix + 1);
    let Some(path_part) = label.strip_prefix(&prefix) else { return };
    let mut col = width(&prefix); // display cells from the label start
    let mut seg_start = col;
    let mut segs: Vec<(usize, usize, bool)> = Vec::new(); // (start, end, clean)
    let mut clean = true; // no `…` seen inside this segment
    for ch in path_part.chars() {
        let w = width(&ch.to_string());
        if ch == '/' || ch == '\\' {
            if col > seg_start {
                segs.push((seg_start, col, clean));
            }
            clean = true;
            seg_start = col + w;
        } else if ch == '…' {
            clean = false;
        }
        col += w;
    }
    if col > seg_start {
        segs.push((seg_start, col, clean));
    }
    // The label's trailing " " rides along in the last segment; harmless.
    let n = segs.len();
    for (i, (s, e, clean)) in segs.into_iter().enumerate() {
        if !clean {
            continue;
        }
        // Segments count up from the end: the last is the cwd itself (0 to
        // strip), the one before it 1, and so on.
        let strip = n - 1 - i;
        let x = area.x + tab_col + s as u16;
        crumb_rects.push((pane_id, strip, Rect::new(x, area.y, (e - s) as u16, 1)));
    }
}

/// The colour of a git status badge.
fn git_mark_color(m: cian_core::git::GitMark) -> Color {
    use cian_core::git::GitMark::*;
    match m {
        Staged => Color::Rgb(130, 225, 150),   // green
        Modified => Color::Rgb(240, 210, 120),  // yellow
        Untracked => Color::Rgb(130, 170, 210), // blue-grey
        Conflict => Color::Rgb(255, 130, 135),  // red
        DirDirty => Color::Rgb(180, 165, 110),  // muted yellow
    }
}

/// Fixed widths so the columns line up between the two panes.
const SIZE_COL_W: u16 = 5;
const TIME_COL_W: u16 = 16;

/// Draw a scrollbar on a pane's right border when the listing overflows.
/// Where the listing's window starts: `scroll`, moved the least amount that
/// puts `cursor` inside it.
pub(crate) fn clamp_list_scroll(
    scroll: usize,
    cursor: usize,
    list_h: usize,
    total: usize,
) -> usize {
    if list_h == 0 {
        return cursor;
    }
    let max = total.saturating_sub(list_h);
    let mut s = scroll.min(max);
    if cursor < s {
        s = cursor;
    } else if cursor >= s + list_h {
        s = cursor + 1 - list_h;
    }
    s.min(max)
}

#[allow(clippy::too_many_arguments)]
fn draw_list_scrollbar(
    f: &mut Frame,
    area: Rect,
    total: usize,
    cursor: usize,
    scroll: usize,
    focused: bool,
    finder: bool,
    border: Style,
    pane_id: FocusedPane,
    tracks: &mut Vec<crate::ScrollTrack>,
) {
    // Two rows in: the top border and the column-header row.
    let view_h = area.height.saturating_sub(3);
    if view_h == 0 || total <= view_h as usize {
        return;
    }
    // Two columns in the desktop views, one in the classic.
    //
    // The classic view's bar *is* the pane's border, and a two-cell border
    // would be a different frame. The desktop views have no border there and
    // are driven with the mouse, where a one-cell bar is a thing you aim at
    // rather than a thing you grab — which is what "I can't see a scrollbar"
    // turned out to mean once it was drawn solidly.
    let thick: u16 = if finder { 2 } else { 1 };
    let track = Rect::new(area.x + area.width.saturating_sub(thick), area.y + 2, thick, view_h);
    tracks.push(crate::ScrollTrack {
        rect: track,
        what: crate::ScrollWhat::Pane(pane_id),
        total,
        shown: view_h as usize,
    });
    // The bar's range is what can actually scroll — the content less the
    // window showing it. Given the whole content instead, the thumb stops
    // short of the end by exactly one window: on a file twice the height of
    // the pane it reached the middle and no further.
    let max = total.saturating_sub(view_h as usize);
    let at = clamp_list_scroll(scroll, cursor, view_h as usize, total);
    // With a border, the bar sits *on* it, so the track has to be the border:
    // same glyph, same style. Drawing it in its own dimmer color made the right
    // edge look broken — bright where the thumb was, faded elsewhere, while the
    // other three sides stayed the border color.
    //
    // The desktop views have no border to sit on, and the quiet version of this
    // — a grey │ thumb on a grey │ track — was reported as no scrollbar at all,
    // which is fair: the two differ by a shade. There, the thumb is a solid
    // block, in the focus colour when the pane is the one being driven. That is
    // the classic view's own emphasis (it reverses the border out, which paints
    // a solid bar) said in the way a borderless pane can say it.
    let (thumb_symbol, thumb, track_symbol, track_style) = match (finder, focused) {
        (false, true) => ("│", border.add_modifier(Modifier::REVERSED), "│", border),
        (false, false) => ("│", Style::default().fg(Color::Rgb(120, 120, 145)), "│", border),
        (true, true) => (
            "█",
            Style::default()
                .fg(text_tone(theme().accent, theme().base_bg.unwrap_or(theme().popup_bg))),
            "│",
            border,
        ),
        (true, false) => ("█", Style::default().fg(Color::Rgb(120, 120, 145)), "│", border),
    };
    // One widget per column: ratatui's scrollbar is one cell wide by
    // definition, and two of them side by side are one bar you can hit.
    for i in 0..thick {
        let mut state = ScrollbarState::new(max).position(at);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_symbol(thumb_symbol)
                .thumb_style(thumb)
                .track_symbol(Some(track_symbol))
                .track_style(track_style)
                .begin_symbol(None)
                .end_symbol(None),
            Rect::new(track.x + i, track.y, 1, track.height),
            &mut state,
        );
    }
}

/// How far back through the scrollback this pane is looking: a bar down its
/// right border, and a badge saying so.
///
/// The badge matters more than the bar. Output scrolling past is the normal
/// state of a shell, and a panel that has quietly stopped following it —
/// while a build carries on underneath — is a panel that looks hung.
fn draw_shell_scrollback(
    f: &mut Frame,
    area: Rect,
    inner: Rect,
    s: &PtySession,
    focused: bool,
) {
    let at = s.scrollback_pos();
    if at == 0 || inner.height == 0 {
        return;
    }
    let view = inner.height as usize;
    let total = at + view;
    f.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("┃")
            .thumb_style(Style::default().fg(if focused { theme().accent } else { theme().border }))
            .track_symbol(Some("│"))
            .track_style(Style::default().fg(theme().border))
            .begin_symbol(None)
            .end_symbol(None),
        Rect::new(area.x + area.width.saturating_sub(1), inner.y, 1, inner.height),
        // Counting from the bottom, so the thumb is where the eye expects it:
        // at the foot of the track while looking at live output.
        &mut ScrollbarState::new(total).position(total.saturating_sub(at + view / 2)),
    );
    let badge = format!(" ↑ {at} ");
    let bw = badge.chars().count() as u16;
    if bw < inner.width {
        f.render_widget(
            Paragraph::new(badge).style(
                Style::default()
                    .fg(readable_on(theme().accent))
                    .bg(theme().accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Rect::new(inner.x + inner.width - bw, inner.y + inner.height - 1, bw, 1),
        );
    }
}

/// Draw the shell panel, then apply its background tint.
///
/// The tint has to be a post-pass. The PTY widget writes an explicit `Reset`
/// background into every cell the shell left uncolored, which would clobber
/// any background set on the block underneath. Recoloring only the cells
/// that are still `Reset` tints the panel while leaving alone every color
/// the shell chose for itself (ls colors, a vim theme, and so on).
#[allow(clippy::too_many_arguments)]
fn draw_shell(
    f: &mut Frame,
    area: Rect,
    shell: &mut ShellPane,
    focused: bool,
    dividers: &mut Vec<Divider>,
    leaves: &mut Vec<(usize, usize, Rect, Rect)>,
    ov: AnimOverride,
    tab_rects: &mut Vec<(FocusedPane, usize, Rect)>,
    log_border: Color,
) {
    draw_shell_inner(f, area, shell, focused, dividers, leaves, ov, tab_rects, log_border);
}

/// Repaint every still-uncolored cell in `area` with `bg`.
pub(crate) fn tint_default_cells(f: &mut Frame, area: Rect, bg: Color) {
    let buf = f.buffer_mut();
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if cell.bg == Color::Reset {
                    cell.set_bg(bg);
                }
            }
        }
    }
}

/// Like [`tint_default_cells`], but also recolors the *foreground* of cells the
/// shell left at the terminal default. On a light theme the shell's own default
/// text is otherwise a pale terminal color on the pale base — the letters you
/// type look washed out. Colors the shell chose for itself are left alone.
fn tint_shell_base(f: &mut Frame, area: Rect, bg: Color, fg: Color) {
    let buf = f.buffer_mut();
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if cell.bg == Color::Reset {
                    cell.set_bg(bg);
                }
                if cell.fg == Color::Reset {
                    cell.set_fg(fg);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_shell_inner(
    f: &mut Frame,
    area: Rect,
    shell: &mut ShellPane,
    focused: bool,
    dividers: &mut Vec<Divider>,
    leaves: &mut Vec<(usize, usize, Rect, Rect)>,
    ov: AnimOverride,
    tab_rects: &mut Vec<(FocusedPane, usize, Rect)>,
    log_border: Color,
) {
    // The panel border turns to the pulsing carmine when the pane it frames
    // (a lone leaf, or a maximized one) is recording.
    let panel_logs = shell
        .active_tab()
        .map(|t| {
            let single = t.leaves().len() == 1;
            (single || shell.zoom_pane)
                && matches!(
                    t.nodes.get(t.active).and_then(|n| n.as_ref()),
                    Some(Node::Leaf { session, .. }) if session.is_logging()
                )
        })
        .unwrap_or(false);
    let border_style = if panel_logs {
        Style::default().fg(log_border).add_modifier(Modifier::BOLD)
    } else if focused {
        accent_on_popup()
    } else {
        Style::default().fg(theme().border)
    };
    let mut offsets = Vec::new();
    let title = shell_tabs_title(shell, focused, &mut offsets);
    for (i, off, w) in offsets {
        tab_rects.push((FocusedPane::Shell, i, Rect::new(area.x + 1 + off, area.y, w, 1)));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(border_style)
        .title(title);
    let inner = area.inner(Margin { vertical: 1, horizontal: 1 });
    f.render_widget(block, area);

    // Remember the inner size for sizing newly-spawned panes.
    shell.rows = inner.height.max(1);
    shell.cols = inner.width.max(1);

    let active = shell.active;
    if shell.tabs.get(active).is_none() {
        let body = if let Some(err) = &shell.error {
            // The command that was tried, next to why it did not work: a shell
            // that will not start is usually a shell that is not where it was
            // said to be, and the answer is in the name.
            format!("shell failed to start: {}\n  tried: {}", err, shell.shell_cmd)
        } else if shell.is_starting() {
            "starting shell…".to_string()
        } else {
            "shell pane — focus here (Shift+J / click / :shell) to start a shell. \
             Esc returns to the files."
                .to_string()
        };
        f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inner);
        return;
    }

    // A shell that has started and not yet said anything. The panel would
    // otherwise be blank, which is what "the shell does not work" looks like
    // when what is happening is a profile script taking its time.
    if let Some(s) = shell.active_session() {
        // Running, and with nothing on its screen. After a few seconds that is
        // no longer "starting up", and the panel should say what it is instead
        // of showing an empty rectangle — which is indistinguishable from cian
        // being broken, and was reported as exactly that.
        const PATIENCE: std::time::Duration = std::time::Duration::from_secs(3);
        if s.screen_is_blank() && s.age() >= PATIENCE {
            let secs = s.age().as_secs();
            let waiting = format!(
                "{} は {secs} 秒前に起動しましたが、まだ何も表示していません。\n\n\
                 プロファイルの読み込みで止まっていることがあります。\
                 とくにホームが OneDrive にある環境では時間がかかります。\n\
                 切り分け: init.lua に次を書いて起動し直すと、\
                 プロファイルが原因かどうか分かります。\n\n\
                 {}",
                shell.shell_cmd,
                r#"    cian.set_option("shell", "powershell.exe -NoLogo -NoProfile")"#,
            );
            f.render_widget(
                Paragraph::new(waiting)
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(theme().dim)),
                inner,
            );
            return;
        }
    }

    // Shift+F12: show only the active leaf, filling the panel. Suppressed
    // while a pane-zoom transition runs, so the splits show as the backdrop
    // the pane grows out of.
    if shell.zoom_pane && !ov.show_splits {
        let leaf = shell.tabs[active].active;
        if let Some(tab) = shell.tabs.get_mut(active) {
            if let Some(Node::Leaf { session: s, .. }) = tab.nodes.get_mut(leaf).and_then(|n| n.as_mut()) {
                s.resize(inner.height.max(1), inner.width.max(1));
            }
        }
        if let Some(Node::Leaf { session: s, bg }) = shell.tabs[active].nodes.get(leaf).and_then(|n| n.as_ref()) {
            if let Ok(parser) = s.parser().lock() {
                f.render_widget(PseudoTerminal::new(parser.screen()), inner);
            }
            if let Some(c) = bg {
                tint_default_cells(f, inner, *c);
            }
            draw_shell_scrollback(f, area, inner, s, focused);
        }
        // A maximized pane hides its siblings; say how many, so it is clear
        // this is one of several and not the whole tab.
        let (pos, total) = shell.active_pane_position();
        if total > 1 {
            let badge = format!(" ▣ pane {}/{}  ({} hidden) ", pos, total, total - 1);
            let bw = badge.chars().count() as u16;
            if bw < inner.width {
                let at = Rect::new(inner.x + inner.width - bw, inner.y, bw, 1);
                f.render_widget(
                    Paragraph::new(badge).style(
                        Style::default()
                            .fg(readable_on(theme().accent))
                            .bg(theme().accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    at,
                );
            }
        }
        if let Some(bg) = theme().base_bg {
            tint_shell_base(f, inner, bg, theme().file.plain);
        }
        return;
    }

    let root = shell.tabs[active].root;
    // While a transition runs the PTYs keep their old size; the real resize
    // happens on the frame after it lands.
    if !ov.freeze_pty {
        if let Some(tab) = shell.tabs.get_mut(active) {
            resize_node(tab, active, root, inner, false, ov);
        }
    }
    let broadcast = shell.is_broadcasting();
    let sync_members = shell.sync_members.clone();
    let tab = &shell.tabs[active];
    render_node(f, tab, active, root, inner, tab.active, focused, false, dividers, leaves, ov, log_border, broadcast, &sync_members);
    // Fill any cell the shell left at the terminal default with the theme's
    // base, so a light theme's shell panel matches the rest.
    if let Some(bg) = theme().base_bg {
        tint_shell_base(f, inner, bg, theme().file.plain);
    }
}

/// Recursively size each leaf's PTY to its rect. `bordered` is true for leaves
/// inside a split (which draw a 1-cell border), false for a lone root leaf.
fn resize_node(tab: &mut ShellTab, tab_idx: usize, i: usize, area: Rect, bordered: bool, ov: AnimOverride) {
    let split = match tab.nodes.get(i).and_then(|n| n.as_ref()) {
        Some(Node::Split { dir, first, second, ratio }) => Some((*dir, *first, *second, *ratio)),
        Some(Node::Leaf { .. }) => None,
        None => return,
    };
    match split {
        None => {
            let (h, w) = if bordered {
                (area.height.saturating_sub(2).max(1), area.width.saturating_sub(2).max(1))
            } else {
                (area.height.max(1), area.width.max(1))
            };
            if let Some(Node::Leaf { session: s, .. }) = tab.nodes[i].as_mut() {
                s.resize(h, w);
            }
        }
        Some((dir, first, second, ratio)) => {
            let r = ov.ratio_for(DividerTarget::ShellSplit { tab: tab_idx, node: i }, ratio);
            let rects = split_rects(dir, area, r);
            resize_node(tab, tab_idx, first, rects.0, true, ov);
            resize_node(tab, tab_idx, second, rects.1, true, ov);
        }
    }
}

/// Recursively render the split tree. Leaves inside a split get a border (the
/// active one highlighted); a lone root leaf fills its area without one.
#[allow(clippy::too_many_arguments)]
fn render_node(
    f: &mut Frame,
    tab: &ShellTab,
    tab_idx: usize,
    i: usize,
    area: Rect,
    active_leaf: usize,
    focused: bool,
    bordered: bool,
    dividers: &mut Vec<Divider>,
    leaves: &mut Vec<(usize, usize, Rect, Rect)>,
    ov: AnimOverride,
    log_border: Color,
    broadcast: bool,
    sync_members: &std::collections::BTreeSet<usize>,
) {
    match tab.nodes.get(i).and_then(|n| n.as_ref()) {
        Some(Node::Leaf { session, bg }) => {
            let target = if bordered {
                let is_active = focused && i == active_leaf;
                // A pane is a live sync target when broadcast is on AND either the
                // member set is empty (all panes) or it lists this leaf.
                let sync_here = broadcast && (sync_members.is_empty() || sync_members.contains(&i));
                // Broadcast/synchronize is the loudest state (input hits every
                // pane), so it wins the border colour — a bright amber with a
                // `⇄` badge on each pane it targets.
                let bs = if sync_here {
                    Style::default().fg(Color::Rgb(255, 176, 32)).add_modifier(Modifier::BOLD)
                } else if session.is_logging() {
                    Style::default().fg(log_border).add_modifier(Modifier::BOLD)
                } else if is_active {
                    accent_on_popup()
                } else {
                    Style::default().fg(dim_text(surface()))
                };
                let mut blk = Block::default().borders(Borders::ALL)
        .border_type(border_type()).border_style(bs);
                if sync_here {
                    // Show the group size (n/total) only when it is a real subset.
                    let title = if sync_members.is_empty() {
                        " ⇄ SYNC ".to_string()
                    } else {
                        let all = tab.leaves();
                        let live = all.iter().filter(|l| sync_members.contains(l)).count();
                        format!(" ⇄ SYNC {}/{} ", live, all.len())
                    };
                    blk = blk.title(title);
                }
                let pinner = area.inner(Margin { vertical: 1, horizontal: 1 });
                f.render_widget(blk, area);
                pinner
            } else {
                area
            };
            // (tab, leaf, outer area for focus, inner PTY area for selection).
            leaves.push((tab_idx, i, area, target));
            if let Ok(parser) = session.parser().lock() {
                f.render_widget(PseudoTerminal::new(parser.screen()), target);
            }
            // Tint after the PTY has drawn: it writes an explicit Reset
            // background into every cell the shell left uncolored, which would
            // otherwise clobber anything set underneath.
            if let Some(c) = bg {
                tint_default_cells(f, area, *c);
            }
            // Each half of a split says where it is looking, independently:
            // the wheel scrolls the one under the pointer, so two of them can
            // be at different places in their own output.
            draw_shell_scrollback(f, area, target, session, focused);
        }
        Some(Node::Split { dir, first, second, ratio }) => {
            let target = DividerTarget::ShellSplit { tab: tab_idx, node: i };
            let rects = split_rects(*dir, area, ov.ratio_for(target, *ratio));
            let d = match dir {
                SplitDir::LeftRight => Direction::Horizontal,
                SplitDir::TopBottom => Direction::Vertical,
            };
            dividers.push(Divider {
                zone: seam_zone(d, rects.0, rects.1),
                parent: area,
                dir: d,
                target,
            });
            render_node(f, tab, tab_idx, *first, rects.0, active_leaf, focused, true, dividers, leaves, ov, log_border, broadcast, sync_members);
            render_node(f, tab, tab_idx, *second, rects.1, active_leaf, focused, true, dividers, leaves, ov, log_border, broadcast, sync_members);
        }
        None => {}
    }
}

/// Split a rect along `dir`, giving `ratio` percent of it to the first child.
fn split_rects(dir: SplitDir, area: Rect, ratio: u16) -> (Rect, Rect) {
    let direction = match dir {
        SplitDir::LeftRight => Direction::Horizontal,
        SplitDir::TopBottom => Direction::Vertical,
    };
    let first = ratio.min(100);
    let rects = Layout::default()
        .direction(direction)
        .constraints([Constraint::Percentage(first), Constraint::Percentage(100 - first)])
        .split(area);
    (rects[0], rects[1])
}

/// The band of cells that counts as grabbing the border between `a` and `b`.
/// The two rects are adjacent, so the seam is the last row/column of `a` plus
/// the first of `b` — two cells, which is a comfortable grab target.
fn seam_zone(dir: Direction, a: Rect, b: Rect) -> Rect {
    match dir {
        Direction::Horizontal => Rect {
            x: a.x + a.width.saturating_sub(1),
            y: a.y,
            width: 2.min(b.x + b.width - (a.x + a.width.saturating_sub(1))),
            height: a.height,
        },
        Direction::Vertical => Rect {
            x: a.x,
            y: a.y + a.height.saturating_sub(1),
            width: a.width,
            height: 2.min(b.y + b.height - (a.y + a.height.saturating_sub(1))),
        },
    }
}

/// A prompt line with a right-aligned hint, used by filter mode.
/// The one style a typed prompt wears, wherever it is typed: a dark bar with
/// white on it. The file panes' `:` and `/` have always looked like this, and
/// the viewer's now do too — a prompt that changed colour depending on which
/// half of cian raised it was the same prompt pretending to be two.
pub(crate) fn prompt_style() -> Style {
    Style::default()
        .bg(theme().popup_bg)
        .fg(readable_on(theme().popup_bg))
        .add_modifier(Modifier::BOLD)
}

fn draw_prompt_line(f: &mut Frame, area: Rect, left: &str, right: &str) {
    let style = prompt_style();
    f.render_widget(Paragraph::new(left).style(style), area);
    let w = right.chars().count() as u16 + 1;
    if area.width > w {
        let hint = Rect::new(area.x + area.width - w, area.y, w, 1);
        f.render_widget(
            Paragraph::new(right).style(
                style.fg(muted_on(theme().popup_bg)).remove_modifier(Modifier::BOLD),
            ),
            hint,
        );
    }
}

fn draw_command_line(f: &mut Frame, area: Rect, buf: &str) {
    let text = format!(":{}", buf);
    let p = Paragraph::new(text).style(prompt_style());
    f.render_widget(p, area);
}

/// Blend `c` toward white by `t` (0 = unchanged, 1 = fully lit). Used for the
/// operation flash, which fades a border back to its resting color.
/// The recording-border color at time `elapsed`: carmine that pulses between a
/// deep and a bright shade on a ~10-second cycle, so a logging pane reads as
/// "● recording" without ever disappearing.
fn recording_pulse(elapsed: std::time::Duration) -> Color {
    let period = 10.0_f32;
    let phase = (elapsed.as_secs_f32() % period) / period;
    // Smooth 0→1→0 over the cycle (cosine), never reaching either extreme.
    let level = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * phase).cos();
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * level) as u8;
    // Deep carmine → bright carmine.
    Color::Rgb(lerp(120, 214), lerp(0, 45), lerp(20, 70))
}

fn fade(c: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (r, g, b) = match c {
        Color::Rgb(r, g, b) => (r, g, b),
        // Named colors have no components to blend; approximate with a light
        // neutral so the flash still reads.
        _ => (200, 220, 255),
    };
    let mix = |v: u8| (v as f32 + (255.0 - v as f32) * t) as u8;
    Color::Rgb(mix(r), mix(g), mix(b))
}

/// Colour a status message by what kind of news it carries, so a failure never
/// wears the same clothes as a success. Classified from the text itself —
/// messages come from a hundred call sites and most already start with a glyph
/// (✔/⚠) or contain an unambiguous failure word; the rest stay accent-neutral.
pub(crate) fn message_color(msg: &str) -> Color {
    const GOOD: Color = Color::Rgb(110, 200, 130);
    const WARN: Color = Color::Rgb(235, 200, 100);
    const BAD: Color = Color::Rgb(235, 110, 110);
    if msg.starts_with('✔') || msg.starts_with("saved") || msg.starts_with("copied")
        || msg.starts_with("renamed") || msg.starts_with("created")
    {
        return GOOD;
    }
    if msg.starts_with('⚠') || msg.contains("cancelled") || msg.contains("中止")
        || msg.contains("unsaved") || msg.contains("未保存")
    {
        return WARN;
    }
    let lower = msg.to_lowercase();
    if lower.contains("fail") || lower.contains("error") || lower.contains("cannot")
        || lower.contains("not found") || lower.contains("denied")
        || msg.contains("できません") || msg.contains("失敗") || msg.contains("ありません")
    {
        return BAD;
    }
    theme().accent
}

fn focus_badge_color(mode: Mode) -> Color {
    match mode {
        Mode::Normal => CIAN,
        Mode::Visual => Color::Rgb(255, 140, 0),
        Mode::Search => Color::Rgb(80, 200, 120),
        Mode::Command => Color::Rgb(200, 100, 200),
        Mode::Filter => Color::Rgb(80, 200, 120),
        // **The shell is a surface too.** It used to be gold; 2026-09-06:
        // 「シェルパネルに焦点があたったときは、金じゃなくシアンにして
        // ほしい。アクティブパネルはシアンにしたい」。The frame answers
        // "which surface has the keys", and an answer that changes colour
        // depending on *which* surface is one more thing to learn. Green and
        // purple stay: those say what you are *doing*, not where you are.
        Mode::Shell => CIAN,
    }
}

/// The keys worth advertising in the current context.
///
/// Deliberately short and mode-specific: a bar listing everything is wallpaper
/// that stops being read. `?` is always last so the full manual is reachable
/// from whatever state the user is stuck in.
pub(crate) fn key_hints(app: &App) -> Vec<(&'static str, &'static str)> {
    // Pick the English or Japanese label; the key column is the same either way.
    let ja = app.lang == Lang::Ja;
    let d = move |en: &'static str, jp: &'static str| -> &'static str {
        if ja {
            jp
        } else {
            en
        }
    };
    // A file docked in this pane: the hints are the file's, because the keys
    // are. The full-screen viewer carries its own bar inside its frame; a
    // docked one is only as wide as the pane, so its hints go here where
    // there is room for them.
    if matches!(app.popup, Popup::Viewer { .. }) && app.viewer_dock == Some(app.focused) {
        let editing = matches!(app.popup, Popup::Viewer { editing: true, .. });
        // Notepad style first: half of what the editor otherwise advertises
        // does not exist there. There is no mode for Esc to leave and no
        // command line for `:q` to be typed at, so those two hints would be
        // instructions that fail.
        if app.notepad_keys() {
            return vec![
                ("Ctrl+S", d("save", "保存")),
                ("Shift+←→", d("select", "選択")),
                ("Ctrl+C / V", d("copy / paste", "コピー / 貼り付け")),
                ("Ctrl+F", d("search", "検索")),
                ("Esc", d("close", "閉じる")),
                // Not `T`: that is a character in this grammar and a vi motion
                // in the other, so it only reaches the switch from a listing.
                // The panel's own menu is what is reachable from inside it —
                // by right-click too, which no terminal can take away.
                ("S-Enter", d("menu — editor keys", "メニュー — キー操作切替")),
            ];
        }
        // `T` on both of vim's rows. Someone who opens a file, types a
        // sentence and watches it not appear is not going to find the switch
        // by guessing that a *file manager* menu holds it — and that person is
        // exactly who it was added for. It costs one column to say so, and no
        // heuristic has to work out whether they are lost.
        // Only what works *while typing*. `:` and `?` are characters here —
        // the editor takes every key that is not an F-key or Shift+Enter — so
        // `:q`, `:notepad` and `?` were three hints that typed themselves into
        // the file. The `T` on this row was found and fixed; its neighbours,
        // which are wrong for the same reason, were not re-read at the time.
        return if editing {
            vec![
                ("Ctrl+S", d("save", "保存")),
                ("Esc", d("leave the editor", "編集終了")),
                ("S-Enter", d("menu", "メニュー")),
            ]
        } else {
            vec![
                ("/", d("search", "検索")),
                ("i", d("edit", "編集")),
                ("v", d("select", "選択")),
                ("y", d("copy", "コピー")),
                ("d c y", d("+ motion", "＋モーション")),
                ("Tab", d("the other pane", "反対ペインへ")),
                (":q", d("close", "閉じる")),
                (":notepad", d("notepad keys", "メモ帳ふうに")),
                ("?", d("keys", "キー一覧")),
            ]
        };
    }
    if app.focused == FocusedPane::Shell {
        let mut v = vec![("Esc", d("files", "ファイル"))];
        // How to copy out of a shell, which nothing on screen said. Shown while
        // a selection is up, when it is the question being asked; the rest of
        // the time the row has more useful things on it.
        if app.shell_ctrl_c_copies() {
            v.push(("^C", d("copy the selection", "選択をコピー")));
        } else {
            v.push((d("drag", "ドラッグ"), d("select = copy", "選択でコピー")));
        }
        // When the last output looks like an error, nudge toward asking Carmine
        // to explain it — the action lives at the top of the shell menu
        // (Shift+Enter), which works everywhere a modifier-combo might not.
        if app.shell_error_detected() {
            v.push(("⚠ S-Enter", d("explain error", "エラーを説明")));
        }
        // Moving between split panes only exists once there is a split, and it
        // is the hint most worth showing then — the key is easy to forget and
        // there is nothing on screen otherwise to suggest it.
        if app.shell.active_pane_count() > 1 {
            v.push(("S-F1/S-F2", d("prev/next pane", "前/次のペイン")));
        }
        v.extend([
            // F1..F8 jump straight to tab N; naming F1/F2 stands in for the row.
            ("F1/F2", d("tab 1/2", "タブ1/2")),
            ("F9", d("new tab", "新規タブ")),
            ("F10", d("close tab", "タブを閉じる")),
            // Named per key rather than as a pair. "S-F8/F9" read as
            // "Shift+F8 or F9" — with plain F9 (new tab) sitting right beside
            // it — and gave no clue which key gave which orientation.
            ("S-F8", d("v-split", "左右分割")),
            ("S-F9", d("h-split", "上下分割")),
            ("S-F10", d("close split", "分割を閉じる")),
            ("F12", d("zoom", "ズーム")),
            // No `? help` here: in the shell `?` is a literal character that
            // goes to the running program, so advertising it would be a lie.
            // Shift+Enter opens the menu, which leads to the manual.
            ("S-Enter", d("menu", "メニュー")),
        ]);
        return v;
    }
    match app.mode {
        Mode::Visual => vec![
            ("j/k", d("extend", "伸ばす")),
            ("a", d("all", "全選択")),
            ("gg/G", d("top/bottom", "先頭/末尾")),
            ("Enter", d("confirm", "確定")),
            ("Esc", d("cancel", "取消")),
        ],
        Mode::Filter => vec![
            ("type", d("narrow", "絞込")),
            ("Enter", d("keep", "適用")),
            ("Esc", d("clear", "解除")),
        ],
        Mode::Command => vec![("Enter", d("run", "実行")), ("Esc", d("cancel", "取消"))],
        // A flat / search listing is a mode of its own: the one thing that must
        // be obvious is how to get out of it, then that marks and file ops work
        // on the results just like a normal listing.
        // Inside an archive the keys mean archive things — and there is
        // nothing else on screen to say so, which is exactly when the bar
        // earns its row.
        _ if app.active_pane().map(|p| p.archive_view().is_some()).unwrap_or(false) => {
            let mut v = vec![
                ("Enter/l", d("in", "入る")),
                // Backspace, not `-/h`: `-` is bound to nothing at all unless
                // a keymap says so, and `h` opens the directory history.
                ("Bksp", d("out", "戻る")),
                ("F3", d("view member", "メンバー閲覧")),
                ("Space", d("mark", "マーク")),
                ("c", d("extract →", "展開 →")),
            ];
            // The write half exists for zip only; saying so beats a key that
            // answers "read-only for now".
            let zip = app
                .active_pane()
                .and_then(|p| p.archive_view())
                .map(|(a, _)| {
                    a.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("jar"))
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if zip {
                v.extend([("r", d("rename", "リネーム")), ("d", d("delete", "削除"))]);
            } else {
                v.push(("", d("(this archive cannot be written)", "（このアーカイブは書き換えられません）")));
            }
            v.push(("?", d("help", "ヘルプ")));
            v
        }
        // A narrowed listing says how to widen it again, in the bar rather than
        // in the manual: the state is easy to get into and used to be hard to
        // notice you were in.
        _ if app.active_pane().map(|p| !p.filter.is_empty()).unwrap_or(false) => vec![
            ("⇦/Esc", d("clear filter", "絞込解除")),
            ("Space", d("mark", "マーク")),
            ("/", d("filter", "絞込")),
            ("Enter", d("open", "開く")),
            ("F3", d("view", "閲覧")),
            ("M", d("menu", "メニュー")),
            ("?", d("help", "ヘルプ")),
        ],
        _ if app.active_pane().map(|p| p.is_flat()).unwrap_or(false) => vec![
            ("b/Esc", d("leave", "戻る")),
            ("Space", d("mark", "マーク")),
            ("/", d("filter", "絞込")),
            ("Enter", d("open", "開く")),
            ("F3", d("view", "閲覧")),
            ("?", d("help", "ヘルプ")),
        ],
        // Ordered by how often each is reached for: a narrow window drops
        // from the end, and `? help` is reserved separately. Kept short on
        // purpose — a bar listing everything becomes wallpaper, and the
        // manual is one keystroke away.
        _ => vec![
            // Switching focus between the two file panes and the shell is the
            // core two-pane move, so it leads the bar.
            ("←→", d("panes", "ペイン")),
            ("S-J", d("shell", "シェル")),
            ("Space", d("mark", "マーク")),
            ("/", d("filter", "絞込")),
            (",", d("sort", "並替")),
            ("S-F", d("find", "検索")),
            ("C-F", d("grep", "grep")),
            ("b", d("branch", "ブランチ")),
            ("F3", d("view", "閲覧")),
            ("M", d("menu", "メニュー")),
            // The tab F-keys, which are otherwise invisible: F1/F2 step tabs,
            // F9 opens one, F10 closes one.
            ("F1/F2", d("prev/next tab", "前/次タブ")),
            ("F9", d("new tab", "新規タブ")),
            ("F10", d("close tab", "タブを閉じる")),
            // Last, so it is the first to drop on a narrow window: comparing
            // two files is the rarest of these by some distance.
            ("=", d("diff", "差分")),
            ("?", d("help", "ヘルプ")),
        ],
    }
}

fn draw_key_hints(f: &mut Frame, area: Rect, app: &App) {
    let key_style = Style::default()
        // The accent, fitted to the bar: on a light theme the accent and the
        // status bar are two shades of the same idea.
        .fg(text_tone(theme().accent, theme().status_bg))
        .bg(theme().status_bg)
        .add_modifier(Modifier::BOLD);
    // The description is quieter than its key, on whatever the status bar is
    // painted — the fixed grey it used to be was picked against a dark bar.
    let desc_style = Style::default().fg(muted_on(theme().status_bg)).bg(theme().status_bg);
    let gap = Span::styled("   ", desc_style);

    let hints = key_hints(app);
    // +4 for the space between key and label plus the trailing gap. Display
    // width, not char count, so wide (CJK) labels don't overflow the row.
    let width_of = |(k, d): &(&str, &str)| width(k) as u16 + width(d) as u16 + 4;

    // The last hint is always `? help`. It is the way out of not knowing any
    // of the others, so it must never be the entry that a narrow window drops
    // — reserve its width and truncate the middle instead.
    let (body, tail) = hints.split_at(hints.len().saturating_sub(1));
    let reserved: u16 = tail.iter().map(width_of).sum();

    let mut spans = vec![Span::styled(" ", desc_style)];
    let mut used = 1u16;
    for h in body {
        let w = width_of(h);
        if used + w + reserved > area.width {
            break;
        }
        used += w;
        spans.push(Span::styled(h.0, key_style));
        spans.push(Span::styled(format!(" {}", h.1), desc_style));
        spans.push(gap.clone());
    }
    for h in tail {
        if used + width_of(h) <= area.width {
            spans.push(Span::styled(h.0, key_style));
            spans.push(Span::styled(format!(" {}", h.1), desc_style));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme().status_bg)),
        area,
    );
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
        Style::default().fg(muted_on(theme().status_bg)).bg(theme().status_bg),
    );
    let pad = Span::styled(" ", Style::default().bg(theme().status_bg));
    // Every chip's colour is fitted to the bar it is drawn on. The colours
    // here mean something — amber for a filling disk, green for a clean tree
    // — and were picked against a dark bar; on a light one they were their
    // own background with words in it.
    let chip = |label: String, fg: Color| {
        Span::styled(
            label,
            Style::default()
                .fg(text_tone(fg, theme().status_bg))
                .bg(theme().status_bg)
                .add_modifier(Modifier::BOLD),
        )
    };

    let ja = app.lang == Lang::Ja;
    let items_chip = if ja {
        format!("{} 件", item_count)
    } else {
        format!("{} items", item_count)
    };
    let marks_chip = if ja {
        format!("マーク {}", mark_count)
    } else {
        format!("marks {}", mark_count)
    };
    // The badge names the mode once it leaves Normal (helix-style): the pane
    // letter alone said *where* keys go but not *what* they currently mean.
    let mode_word = match app.mode {
        Mode::Normal | Mode::Shell => "",
        Mode::Visual => " VISUAL",
        Mode::Search => " SEARCH",
        Mode::Command => " CMD",
        Mode::Filter => " FILTER",
    };
    // With the editor panel in focus, the badge is the *file's* mode — READ,
    // EDIT, COMMAND, VISUAL — because that is what the next key will mean.
    // It used to live on the panel's own frame; docked in a pane there is no
    // room for it there, and this is where the window already reports what is
    // going on.
    let notepad_keys = app.notepad_keys();
    let docked_editor = app
        .viewer_dock
        .filter(|p| *p == app.focused)
        .and_then(|_| editor_mode_of(&app.popup, notepad_keys))
        .map(editor_mode);
    let (badge_text, badge_bg) = match docked_editor {
        Some((word, colour)) => (format!(" {focus_label} {word} "), colour),
        None => (format!(" {}{} ", focus_label, mode_word), badge_bg),
    };
    let mut spans: Vec<Span> = vec![
        Span::styled(
            badge_text,
            Style::default().fg(readable_on(badge_bg)).bg(badge_bg).add_modifier(Modifier::BOLD),
        ),
        pad.clone(),
    ];
    if docked_editor.is_some() {
        if let Some((line, col)) = editor_position(&app.popup) {
            spans.push(chip(format!("{line}:{col}"), readable_on(theme().status_bg)));
            spans.push(dim_sep.clone());
        }
    }
    spans.extend([
        chip(items_chip, readable_on(theme().status_bg)),
        dim_sep.clone(),
        chip(
            marks_chip,
            if mark_count > 0 { theme().mark_fg } else { muted_on(theme().status_bg) },
        ),
    ]);

    // The whole name of the file under the cursor.
    //
    // Both desktop views cut a long name to the width they have — the detail
    // view to its name column, the grid to its tile — and a cut name is not a
    // name: `2026-04_...port.xlsx` could be any of six files. The bar has the
    // width, and the cursor is the one row anyone needs it for.
    if let Some(name) = app
        .active_pane()
        .and_then(|p| p.selected())
        .map(|e| e.name.clone())
        .filter(|n| n != "..")
    {
        spans.push(dim_sep.clone());
        spans.push(chip(name, readable_on(theme().status_bg)));
    }

    // A narrowed listing must never look like a complete one, so the active
    // filter stays visible after leaving filter mode.
    if let Some(filter) = app.active_pane().map(|p| p.filter.clone()).filter(|f| !f.is_empty()) {
        let total = app.active_pane().map(|p| p.all_entries.len()).unwrap_or(0);
        spans.push(dim_sep.clone());
        let filter_chip = if ja {
            format!("フィルタ /{} ({}/{} 件)", filter, item_count, total)
        } else {
            format!("filter /{} ({} of {})", filter, item_count, total)
        };
        spans.push(chip(filter_chip, Color::Rgb(80, 200, 120)));
    }

    // The git branch of the active pane's repository, with ahead/behind and a
    // changed-file count — the "branch bar" every developer glances at.
    if let Some(git) = app.git_for(app.focused) {
        spans.push(dim_sep.clone());
        let branch_glyph = if nerd_fonts() { "\u{e0a0} " } else { "" };
        let mut label = format!("{}{}", branch_glyph, git.branch);
        if git.ahead > 0 {
            label.push_str(&format!(" ↑{}", git.ahead));
        }
        if git.behind > 0 {
            label.push_str(&format!(" ↓{}", git.behind));
        }
        let changed = git.changed_count();
        if changed > 0 {
            label.push_str(&format!("  ✚{}", changed));
        }
        // Green when clean, amber when there are uncommitted changes.
        let color = if changed > 0 { Color::Rgb(240, 210, 120) } else { Color::Rgb(130, 205, 150) };
        spans.push(chip(label, color));
    }

    // Free space on the active pane's mount — always in view, since a copy or
    // an extract of a huge tree is a glance away from "will this fit". Amber
    // past 80% used, red past 95%, so a filling disk announces itself.
    if let Some(u) = app.disk_for(app.focused) {
        spans.push(dim_sep.clone());
        let frac = u.used_fraction();
        let color = if frac >= 0.95 {
            Color::Rgb(230, 110, 110)
        } else if frac >= 0.80 {
            Color::Rgb(240, 210, 120)
        } else {
            Color::Rgb(130, 175, 210)
        };
        let label = format!(
            "{}{} free / {}",
            if nerd_fonts() { "\u{f0a0} " } else { "" },
            cian_core::disk::human_size(u.free),
            cian_core::disk::human_size(u.total),
        );
        spans.push(chip(label, color));
    }

    if app.zoomed {
        spans.push(dim_sep.clone());
        spans.push(chip("[zoom]".to_string(), theme().accent));
    }

    // A running operation keeps a chip here — the whole story once the
    // progress popup is tucked away, a heartbeat even while it shows.
    if let Some(job) = &app.op_job {
        spans.push(dim_sep.clone());
        let p = &job.latest;
        let pct = if let Some(f) = (p.bytes_done * 100).checked_div(p.bytes_total) {
            format!(" {}%", f.min(100))
        } else if p.files_total > 0 {
            format!(" {}/{}", p.files_done, p.files_total)
        } else {
            String::new()
        };
        let queued = if app.op_queue.is_empty() {
            String::new()
        } else {
            format!(" +{}", app.op_queue.len())
        };
        if app.op_stalled() {
            let secs = job.last_progress.elapsed().as_secs();
            spans.push(chip(
                format!("⚠ {}{} — stalled {}s{}", job.label, pct, secs, queued),
                Color::Rgb(235, 200, 100),
            ));
        } else {
            // …and the ceiling, when there is one: a transfer that is slow on
            // purpose should say so, or it looks like a transfer that is slow.
            let capped = match app.transfer_limit {
                Some(b) => format!("  ≤{}", crate::rate_text(b)),
                None => String::new(),
            };
            spans.push(chip(
                format!("↻ {}{}{}{}", job.label, pct, queued, capped),
                theme().accent,
            ));
        }
    }

    let has_msg = app.message.as_ref().is_some_and(|m| !m.is_empty());
    if let Some(msg) = app.message.as_ref() {
        if !msg.is_empty() {
            spans.push(dim_sep.clone());
            spans.push(Span::styled(
                format!("◂ {}", msg),
                Style::default()
                    .fg(text_tone(message_color(msg), theme().status_bg))
                    .bg(theme().status_bg)
                    .add_modifier(Modifier::ITALIC | Modifier::BOLD),
            ));
        }
    }

    // Everything on this row except the message is also on screen somewhere
    // else — the path is in the pane title, the branch in its header. The
    // message is the only thing here that is news, and it was last in the
    // queue for space: on a real terminal with a long path and a git chip it
    // was pushed off the right-hand edge and simply never seen. So when they
    // do not all fit, the chips give way, one at a time, from the left —
    // keeping the mode chip, which is what says whether a keystroke will be
    // read as a command.
    if has_msg {
        let total = |v: &[Span]| v.iter().map(|s| width(&s.content)).sum::<usize>();
        let room = area.width as usize;
        // The message is the last two spans (separator + text); never drop it.
        while total(&spans) > room && spans.len() > 3 {
            spans.remove(1);
        }
    }

    let line = Line::from(spans);
    let p = Paragraph::new(line).style(Style::default().bg(theme().status_bg));
    f.render_widget(p, area);

    // The active shell pane's title (its `user@host: cwd`), right-aligned so it
    // sits in the bottom-right and tracks whichever split/tab is active —
    // rather than staying on the first pane. Drawn as its own right-aligned
    // paragraph over the same row.
    // …but not over a message. The title is the same every frame; the message
    // is the answer to what was just pressed.
    if let Some(title) = app.shell.active_title().filter(|_| !has_msg) {
        let shown = format!(" {} ", truncate(&title, (area.width / 2).max(8) as usize));
        f.render_widget(
            Paragraph::new(shown)
                .alignment(Alignment::Right)
                .style(
                    Style::default()
                        .fg(Color::Rgb(150, 200, 235))
                        .bg(theme().status_bg)
                        .add_modifier(Modifier::BOLD),
                ),
            area,
        );
    }
}

/// A progress bar for the running file operation, and the way to stop it.
fn draw_op_progress(f: &mut Frame, area: Rect, app: &App) {
    let Some(job) = &app.op_job else { return };
    draw_progress_bar(f, area, job.label, &job.latest, job.started, app.lang);
}

/// A centered progress dialog: label, current item, a bar, counts and elapsed.
/// Shared by file operations and the directory comparison.
fn draw_progress_bar(
    f: &mut Frame,
    area: Rect,
    label: &str,
    p: &cian_core::progress::Progress,
    started: Instant,
    lang: Lang,
) {
    let w = 74u16.min(area.width.saturating_sub(2));
    let rect = centered_rect(w, 8, area);
    clear_popup(f, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(accent_on_popup())
        .title(format!(" {} ", tr_op_label(lang, label)));
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);

    // Which entry, shortened from the middle so the directory and the filename
    // both stay legible.
    f.render_widget(
        Paragraph::new(truncate_middle(&p.current, inner.width as usize))
            .style(Style::default().fg(Color::Rgb(190, 190, 210))),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let frac = p.fraction().clamp(0.0, 1.0);
    let bar_y = inner.y + 2;
    f.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(50, 50, 66))),
        Rect::new(inner.x, bar_y, inner.width, 1),
    );
    let filled = ((inner.width as f32) * frac).round() as u16;
    if filled > 0 {
        f.render_widget(
            Block::default().style(Style::default().bg(theme().accent)),
            Rect::new(inner.x, bar_y, filled.min(inner.width), 1),
        );
    }

    let counts = if p.bytes_total > 0 {
        format!(
            "{} / {}   ({} of {} files)",
            cian_core::human_size(p.bytes_done),
            cian_core::human_size(p.bytes_total),
            p.files_done,
            p.files_total
        )
    } else {
        format!("{} of {} files", p.files_done, p.files_total)
    };
    // Elapsed time, so a slow volume looks slow rather than stuck.
    let secs = started.elapsed().as_secs();
    let elapsed = if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    };
    f.render_widget(
        Paragraph::new(format!("{:>3}%   {}   ·  {}", (frac * 100.0) as u16, counts, elapsed)),
        Rect::new(inner.x, bar_y + 2, inner.width, 1),
    );
    f.render_widget(
        Paragraph::new(tr(lang, " Esc = stop   b = background ", " Esc = 中止   b = バックグラウンドへ ")).style(
            Style::default().fg(readable_on(theme().accent)).bg(theme().accent).add_modifier(Modifier::BOLD),
        ),
        footer_row(inner),
    );
}

#[allow(clippy::type_complexity)]
/// Register one clickable row spanning `inner`'s width at `y`, standing in for
/// selecting list index `idx`.
fn push_row_zone(zones: &mut Vec<PopupZone>, inner: Rect, y: u16, idx: usize) {
    zones.push(PopupZone {
        rect: Rect::new(inner.x, y, inner.width, 1),
        kind: ZoneKind::SelectRow(idx),
    });
}

/// The AI chat, rendered with `&mut App` so it can stash the transcript's rect,
/// scroll and flat lines for mouse selection.
/// The image preview. On a terminal that answered the startup graphics query
/// (kitty / iTerm2 / sixel), the picture renders as real pixels; everywhere
/// else it falls back to half-block (`▀`) cells — top pixel the glyph's
/// foreground, bottom pixel its background — which any 24-bit terminal can
/// show. Both paths decode to fit and cache.
fn draw_image(f: &mut Frame, area: Rect, app: &mut App) {
    let lang = app.lang;
    let rect = centered_rect(area.width.saturating_sub(2), area.height.saturating_sub(2), area);
    clear_popup(f, rect);
    let inner = rect.inner(Margin { vertical: 1, horizontal: 1 });
    let body_w = inner.width;
    let body_h = inner.height.saturating_sub(1); // leave a row for the footer

    // Real pixels when the terminal offers them — but only while that path is
    // actually producing a picture. When it fails (a protocol the terminal
    // advertised and then would not take, a format the decoder does not
    // know), the half-block renderer below is a worse picture rather than no
    // picture, which is the difference between "grainy" and "broken".
    if app.gfx_picker.is_some() && !app.gfx_failed {
        draw_image_gfx(f, rect, inner, app);
        return;
    }

    // A front end that draws its own icons draws its own pictures: it is handed
    // the rectangle and the path, and puts real pixels there. Half-blocks are
    // what a terminal is reduced to — two pixels per cell — and a window has no
    // reason to be reduced to it. See [`crate::ImageSlot`].
    if app.native_icons {
        if let Popup::ImageView { path, title, error, .. } = &app.popup {
            let (path, title, error) = (path.clone(), title.clone(), error.clone());
            let caption = image::image_dimensions(&path)
                .map(|(w, h)| format!("{w}×{h}px"))
                .unwrap_or_default();
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(border_type())
                .border_style(
                    accent_on_popup(),
                )
                .style(Style::default().bg(theme().popup_bg).fg(readable_on(theme().popup_bg)))
                .title(format!(" {title}  —  {caption} "));
            f.render_widget(block, rect);
            match error {
                Some(e) => f.render_widget(
                    Paragraph::new(format!("cannot show image: {e}"))
                        .style(Style::default().fg(text_tone(theme().file.archive, surface()))),
                    inner,
                ),
                None if body_h > 0 => {
                    app.image_slot = Some(crate::ImageSlot {
                        x: inner.x,
                        y: inner.y,
                        w: body_w,
                        h: body_h,
                        path,
                    });
                }
                None => {}
            }
            f.render_widget(
                Paragraph::new(tr(lang, " Esc / q close ", " Esc / q 閉じる "))
                    .style(Style::default().fg(theme().dim)),
                Rect::new(inner.x, inner.y + body_h, inner.width, 1),
            );
            return;
        }
    }

    // (Re)decode when first shown or after a resize.
    let (title, caption, rows, err) = if let Popup::ImageView { path, title, shown, error } = &mut app.popup {
        if error.is_none() && shown.as_ref().map(|(c, r, _)| (*c, *r)) != Some((body_w, body_h)) {
            match cian_core::image::thumbnail(path, body_w, body_h) {
                Ok(t) => *shown = Some((body_w, body_h, t)),
                Err(e) => *error = Some(e.to_string()),
            }
        }
        let mut rows: Vec<Line> = Vec::new();
        let mut caption = String::new();
        if let Some((_, _, t)) = shown {
            caption = format!("{}×{}px", t.src_w, t.src_h);
            for ry in 0..t.rows as usize {
                let mut spans: Vec<Span> = Vec::with_capacity(t.cols as usize);
                for cx in 0..t.cols as usize {
                    let (top, bot) = t.cells[ry * t.cols as usize + cx];
                    spans.push(Span::styled(
                        "▀",
                        Style::default()
                            .fg(Color::Rgb(top.0, top.1, top.2))
                            .bg(Color::Rgb(bot.0, bot.1, bot.2)),
                    ));
                }
                rows.push(Line::from(spans));
            }
        }
        (title.clone(), caption, rows, error.clone())
    } else {
        (String::new(), String::new(), Vec::new(), None)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(accent_on_popup())
        .style(Style::default().bg(theme().popup_bg).fg(readable_on(theme().popup_bg)))
        .title(format!(" {}  —  {} ", title, caption));
    f.render_widget(block, rect);

    if let Some(e) = err {
        f.render_widget(
            Paragraph::new(format!("cannot show image: {}", e)).style(Style::default().fg(text_tone(theme().file.archive, surface()))),
            inner,
        );
    } else {
        // Centre the picture in its box, vertically and horizontally.
        let img_h = rows.len() as u16;
        let img_w = rows.first().map(|l| l.spans.len() as u16).unwrap_or(0);
        let top = inner.y + (body_h.saturating_sub(img_h)) / 2;
        let left = inner.x + (body_w.saturating_sub(img_w)) / 2;
        let pic = Rect::new(left, top, img_w.min(body_w), img_h.min(body_h));
        f.render_widget(Paragraph::new(rows), pic);
    }

    let footer_area = footer_row(inner);
    f.render_widget(
        Paragraph::new(tr(lang, " S-Enter reveal   E edit   Esc close ", " S-Enter 場所へ   E 編集   Esc 閉じる "))
            .style(Style::default().fg(readable_on(theme().accent)).bg(theme().accent).add_modifier(Modifier::BOLD)),
        footer_area,
    );
}

/// The `:preview` panel, borrowing the shell's area: what the cursor is on,
/// rendered with the F3 assets — syntax colour for code, pixels for images
/// where the terminal can, listings for folders and archives.
fn draw_preview_panel(f: &mut Frame, area: Rect, app: &mut App) {
    let lang = app.lang;
    // Resolve what to show; a reason-not-to is shown as a note.
    let target = crate::preview::preview_target(app);
    let (title_name, note) = match &target {
        Ok(p) => {
            app.ensure_preview(p);
            (p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), None)
        }
        Err(e) => (String::new(), Some(e.clone())),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(Style::default().fg(theme().border))
        .title(Line::from(vec![
            Span::styled(
                " ⌥ preview ",
                Style::default()
                    .fg(text_tone(theme().accent, surface()))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(truncate_middle(&title_name, 48), Style::default().fg(dim_text(surface()))),
            Span::raw(" "),
        ]))
        // Explicitly styled: an unstyled title takes the *border's* colour,
        // and a border colour is chosen to be quiet.
        .title_bottom(Span::styled(
            tr(
                lang,
                " :preview off   Shift+J = shell ",
                " :preview で解除   シェルは Shift+J ",
            ),
            Style::default().fg(dim_text(surface())),
        ));
    let inner = area.inner(Margin { vertical: 1, horizontal: 1 });
    // Wipe the panel before drawing into it. A `Paragraph` writes only the
    // characters it has, and a `Block`'s style recolours cells without
    // replacing them — so the tail of a longer previous file stayed on screen
    // underneath the shorter new one, and the two read as one garbled
    // document. The preview changes contents on every cursor move, which is
    // the worst case for leaving anything behind.
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default().style(Style::default().bg(surface())),
        area,
    );
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    // Two rows is the floor for anything readable. Below it, say why rather
    // than drawing a one-line sliver that looks like a failure.
    if inner.height < 2 {
        f.render_widget(
            Paragraph::new(tr(lang, "(drag the border down for a preview)", "（境界線を下げるとプレビューが出ます）"))
                .style(Style::default().fg(theme().dim)),
            inner,
        );
        return;
    }

    if let Some(msg) = note {
        // A note can be several lines (the cloud explanation is), so it wraps
        // rather than being clipped to the first row.
        f.render_widget(
            Paragraph::new(msg)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme().dim)),
            inner,
        );
        return;
    }

    // Image: pixels when the terminal can, half-blocks otherwise. Cached like
    // the F3 popup, but in preview-owned state.
    if matches!(app.preview.as_ref().map(|p| &p.body), Some(crate::preview::PreviewBody::Image)) {
        let path = app.preview.as_ref().map(|p| p.path.clone()).unwrap_or_default();
        // Decoding happens on another thread: several megabytes of PNG has to
        // be unpacked whole before anything can be scaled, and doing that here
        // takes the time out of the interface — the cursor stopped dead on
        // every large picture. `decoded` is what has arrived, if it has.
        let decoded = app.take_decoded(&path);
        let waiting = app.preview_decode.is_some();
        if app.gfx_picker.is_some() && !app.gfx_failed {
            if app.preview_gfx.as_ref().map(|(p, _)| p != &path).unwrap_or(true) {
                app.preview_gfx = None;
                if let (Some(img), Some(picker)) = (decoded.clone(), app.gfx_picker.as_ref()) {
                    app.preview_gfx = Some((path.clone(), picker.new_resize_protocol(img)));
                }
            }
            if let Some((_, proto)) = app.preview_gfx.as_mut() {
                f.render_stateful_widget(ratatui_image::StatefulImage::default(), inner, proto);
                // If the terminal's own image protocol did not produce
                // anything, fall through to the half-block renderer rather
                // than leaving an empty box: an unreadable picture is still
                // better than none, and the failure is silent otherwise.
                match proto.last_encoding_result() {
                    Some(Err(_)) => {
                        app.gfx_failed = true;
                        app.preview_gfx = None;
                        app.full_clear = true;
                    }
                    _ => return,
                }
            }
        }
        let mut drew = false;
        // The half-block renderer scales from the image the decoder thread
        // handed over, rather than decoding it again for every box size.
        if let Some(img) = decoded {
            if let Some(state) = app.preview.as_mut() {
                state.decoded = Some(img);
            }
        }
        if let Some(state) = app.preview.as_mut() {
            if state.thumb.as_ref().map(|(c, r, _)| (*c, *r)) != Some((inner.width, inner.height)) {
                state.thumb = state
                    .decoded
                    .as_ref()
                    .and_then(|i| {
                        cian_core::image::thumbnail_of(i, inner.width, inner.height).ok()
                    })
                    .map(|t| (inner.width, inner.height, t));
            }
            if state.thumb.is_none() && waiting {
                f.render_widget(
                    Paragraph::new(tr(lang, "reading the picture…", "画像を読み込み中…"))
                        .style(Style::default().fg(muted_on(surface()))),
                    inner,
                );
                return;
            }
            if let Some((_, _, t)) = &state.thumb {
                drew = true;
                let mut rows: Vec<Line> = Vec::new();
                for ry in 0..t.rows as usize {
                    let mut spans = Vec::with_capacity(t.cols as usize);
                    for cx in 0..t.cols as usize {
                        let (top, bot) = t.cells[ry * t.cols as usize + cx];
                        spans.push(Span::styled(
                            "▀",
                            Style::default()
                                .fg(Color::Rgb(top.0, top.1, top.2))
                                .bg(Color::Rgb(bot.0, bot.1, bot.2)),
                        ));
                    }
                    rows.push(Line::from(spans));
                }
                let left = inner.x + (inner.width.saturating_sub(t.cols)) / 2;
                let pic = Rect::new(left, inner.y, t.cols.min(inner.width), (t.rows).min(inner.height));
                f.render_widget(Paragraph::new(rows), pic);
            }
        }
        // Never leave the panel blank: an empty box reads as "the feature is
        // broken", when the honest answer is that this image could not be
        // decoded (or the panel has no room for it).
        if !drew {
            f.render_widget(
                Paragraph::new(tr(lang, "(cannot render this image here)", "（この画像はここに描画できません）"))
                    .style(Style::default().fg(theme().dim)),
                inner,
            );
        }
        return;
    }

    let Some(state) = app.preview.as_ref() else { return };
    let body_fg = readable_on(theme().base_bg.unwrap_or(Color::Black));
    match &state.body {
        crate::preview::PreviewBody::Text { lines, hl } => {
            let mut shown: Vec<Line> = Vec::with_capacity(inner.height as usize);
            for (i, l) in lines.iter().take(inner.height as usize).enumerate() {
                let clipped = truncate(l, inner.width as usize);
                match hl.get(i) {
                    Some(cats) if !cats.is_empty() => {
                        let spans: Vec<Span> = clipped
                            .chars()
                            .enumerate()
                            .map(|(ci, ch)| {
                                let style = cats
                                    .get(ci)
                                    .map(|c| hl_style(*c))
                                    .unwrap_or(Style::default().fg(body_fg));
                                Span::styled(ch.to_string(), style)
                            })
                            .collect();
                        shown.push(Line::from(spans));
                    }
                    _ => shown.push(Line::from(Span::styled(
                        clipped,
                        Style::default().fg(body_fg),
                    ))),
                }
            }
            if shown.is_empty() {
                shown.push(Line::from(Span::styled(
                    tr(lang, "(empty file)", "（空のファイル）"),
                    Style::default().fg(dim_text(surface())),
                )));
            }
            f.render_widget(Paragraph::new(shown), inner);
        }
        crate::preview::PreviewBody::List { rows, truncated } => {
            let mut shown: Vec<Line> = rows
                .iter()
                .take(inner.height as usize)
                .map(|r| Line::from(Span::styled(truncate(r, inner.width as usize), Style::default().fg(body_fg))))
                .collect();
            if *truncated && shown.len() == inner.height as usize {
                if let Some(last) = shown.last_mut() {
                    *last = Line::from(Span::styled("…", Style::default().fg(theme().dim)));
                }
            }
            f.render_widget(Paragraph::new(shown), inner);
        }
        crate::preview::PreviewBody::Note(msg) => {
            f.render_widget(
                Paragraph::new(msg.clone()).style(Style::default().fg(theme().dim)),
                inner,
            );
        }
        crate::preview::PreviewBody::Image => unreachable!("handled above"),
    }
}

/// The terminal-graphics image path: decode once per file (cached on `App`,
/// keyed by path), then let ratatui-image resize/encode for the box each
/// frame in whatever protocol the terminal offered at startup.
fn draw_image_gfx(f: &mut Frame, rect: Rect, inner: Rect, app: &mut App) {
    let lang = app.lang;
    let body_h = inner.height.saturating_sub(1); // the footer keeps its row
    let (path, title) = if let Popup::ImageView { path, title, .. } = &app.popup {
        (path.clone(), title.clone())
    } else {
        return;
    };
    // (Re)decode when a different image opens.
    if app.img_proto.as_ref().map(|(p, _)| p != &path).unwrap_or(true) {
        app.img_proto = None;
        if let (Ok(img), Some(picker)) = (image::open(&path), app.gfx_picker.as_ref()) {
            app.img_proto = Some((path.clone(), picker.new_resize_protocol(img)));
        }
    }
    let caption = image::image_dimensions(&path)
        .map(|(w, h)| format!("{}×{}px", w, h))
        .unwrap_or_default();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(accent_on_popup())
        .style(Style::default().bg(theme().popup_bg).fg(readable_on(theme().popup_bg)))
        .title(format!(" {}  —  {} ", title, caption));
    f.render_widget(block, rect);

    let pic = Rect::new(inner.x, inner.y, inner.width, body_h);
    match app.img_proto.as_mut() {
        Some((_, proto)) => {
            f.render_stateful_widget(
                ratatui_image::StatefulImage::default(),
                pic,
                proto,
            );
            if matches!(proto.last_encoding_result(), Some(Err(_))) {
                // Remembered rather than retried: a protocol that failed once
                // fails every frame, and the half-block renderer is right
                // there.
                app.gfx_failed = true;
                app.img_proto = None;
                app.full_clear = true;
            }
        }
        None => {
            // Nothing decoded it: say so here, since the half-block path
            // would have nothing to draw either.
            f.render_widget(
                Paragraph::new(tr(lang, "cannot show image", "画像を表示できません"))
                    .style(Style::default().fg(text_tone(theme().file.archive, surface()))),
                pic,
            );
        }
    }

    let footer_area = footer_row(inner);
    f.render_widget(
        Paragraph::new(tr(lang, " S-Enter reveal   E edit   Esc close ", " S-Enter 場所へ   E 編集   Esc 閉じる "))
            .style(Style::default().fg(readable_on(theme().accent)).bg(theme().accent).add_modifier(Modifier::BOLD)),
        footer_area,
    );
}

/// Width of the viewer's blame gutter: `hash(7) + " " + author(11) + " "`.
const BLAME_W: usize = 20;

/// Colour for a syntax-highlight category (a VS Code-dark-ish palette).
fn hl_style(cat: cian_core::highlight::Category) -> Style {
    Style::default().fg(hl_style_for(cat))
}

/// The colour one syntax category is drawn in, on the page it will land on.
pub(crate) fn hl_style_for(cat: cian_core::highlight::Category) -> Color {
    use cian_core::highlight::Category as C;
    // Plain text is whatever reads on this page — it is not a syntax colour,
    // it is the absence of one, and the near-white chosen for a dark theme
    // vanished on a light one the moment the cursor line was tinted under it.
    if cat == C::Plain {
        return readable_on(surface());
    }
    let c = match cat {
        C::Plain => unreachable!("handled above"),
        C::Keyword => Color::Rgb(197, 134, 192), // mauve
        C::Type => Color::Rgb(78, 201, 176),      // teal
        C::Str => Color::Rgb(206, 145, 120),      // salmon
        C::Comment => Color::Rgb(106, 153, 85),   // green
        C::Number => Color::Rgb(181, 206, 168),   // pale green
        C::Tag => Color::Rgb(86, 156, 214),       // blue
        C::Attr => Color::Rgb(156, 220, 254),     // light blue
    };
    fit_to_surface(c)
}

/// Pull a colour into range for the current surface.
///
/// The palette above was picked against a dark ground, where a pale green
/// number and a light-blue attribute read well. On a light theme they are two
/// shades from the page and disappear — the more so under the cursor line's
/// own tint. Rather than keep a second palette, each colour is moved along its
/// own hue until it has room to be seen: darkened on a light page, lightened
/// on a dark one, and left alone when it already stands clear.
fn fit_to_surface(c: Color) -> Color {
    let (Color::Rgb(r, g, b), Color::Rgb(sr, sg, sb)) = (c, surface()) else { return c };
    let lum = |r: u8, g: u8, b: u8| (299 * r as i32 + 587 * g as i32 + 114 * b as i32) / 1000;
    let (cl, sl) = (lum(r, g, b), lum(sr, sg, sb));
    // Enough separation to read comfortably at terminal weights.
    const WANT: i32 = 90;
    if (cl - sl).abs() >= WANT {
        return c;
    }
    // Toward black on a light page, toward white on a dark one.
    let toward_black = sl > 140;
    let need = WANT - (cl - sl).abs();
    let shift = |v: u8| -> u8 {
        if toward_black {
            ((v as i32) - need).clamp(0, 255) as u8
        } else {
            ((v as i32) + need).clamp(0, 255) as u8
        }
    };
    Color::Rgb(shift(r), shift(g), shift(b))
}

/// A text color that reads clearly on `bg`: near-black on a light background,
/// near-white on a dark one. Keeps popup text legible under any theme — a light
/// theme (e.g. Solarized Light) would otherwise show pale text on a pale ground.
///
/// Which of the two it is, is decided by measuring rather than by a
/// brightness threshold. A mid-tone chip — a theme accent under the READ
/// badge, say — sits near enough to the line that the threshold could call it
/// either way, and calling it wrong puts pale text on a pale blue. Whichever
/// of the two actually contrasts more, wins.
/// The surface a popup is drawn on: its background, and a foreground the
/// theme guarantees is legible against it.
///
/// Written out at every popup — eighteen of them — and a nineteenth that gave
/// only the background is how a dialog ends up with the terminal's own text
/// colour on the theme's paper.
pub(crate) fn popup_style() -> Style {
    Style::default().bg(theme().popup_bg).fg(readable_on(theme().popup_bg))
}

/// The accent bar a popup's footer and title sit on: the accent as paper, and
/// bold text the theme guarantees is legible on it.
pub(crate) fn accent_bar() -> Style {
    Style::default()
        .fg(readable_on(theme().accent))
        .bg(theme().accent)
        .add_modifier(Modifier::BOLD)
}

/// A popup's own border and heading: the accent, toned to stay legible where
/// it is drawn on the popup's paper rather than filling it.
pub(crate) fn accent_on_popup() -> Style {
    Style::default().fg(text_tone(theme().accent, theme().popup_bg)).add_modifier(Modifier::BOLD)
}

pub(crate) fn readable_on(bg: Color) -> Color {
    const DARK: Color = Color::Rgb(30, 32, 40);
    const LIGHT: Color = Color::Rgb(228, 228, 240);
    let bg = as_rgb(bg);
    if !matches!(bg, Color::Rgb(..)) {
        return Color::Rgb(225, 225, 240); // unknown → assume a dark ground
    }
    let soft = if contrast_ratio(DARK, bg) >= contrast_ratio(LIGHT, bg) { DARK } else { LIGHT };
    if contrast_ratio(soft, bg) >= 4.5 {
        return soft;
    }
    // A mid-tone ground — Catppuccin Latte's blue, Dracula's selection — is
    // far enough from both of the soft tones that neither clears the bar.
    // Only then is it worth the harsher pure black or white: the softer pair
    // is what the rest of the interface is drawn in, and the eye notices the
    // difference long before it notices the extra contrast.
    const BLACK: Color = Color::Rgb(0, 0, 0);
    const WHITE: Color = Color::Rgb(255, 255, 255);
    let hard = if contrast_ratio(BLACK, bg) >= contrast_ratio(WHITE, bg) { BLACK } else { WHITE };
    if contrast_ratio(hard, bg) > contrast_ratio(soft, bg) {
        hard
    } else {
        soft
    }
}

/// A named ANSI colour as the RGB a terminal actually paints, so it can be
/// measured like any other. cian's own default theme is built on `Cyan`, and
/// leaving it unmeasurable meant "unknown → assume a dark ground", which put
/// pale text on a bright cyan badge — the one place the default theme has to
/// be right.
pub(crate) fn as_rgb(c: Color) -> Color {
    let (r, g, b) = match c {
        Color::Rgb(..) => return c,
        Color::Black => (0, 0, 0),
        Color::Red => (205, 0, 0),
        Color::Green => (0, 205, 0),
        Color::Yellow => (205, 205, 0),
        Color::Blue => (0, 0, 238),
        Color::Magenta => (205, 0, 205),
        Color::Cyan => (0, 205, 205),
        Color::Gray => (229, 229, 229),
        Color::DarkGray => (127, 127, 127),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (92, 92, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::White => (255, 255, 255),
        // Reset is the terminal's own colour and Indexed is its palette:
        // neither is knowable from here.
        _ => return c,
    };
    Color::Rgb(r, g, b)
}

/// A colour's relative luminance (WCAG): sRGB undone, then weighted for the
/// eye. Not the quick `0.299r + 0.587g + 0.114b` used elsewhere for "is this
/// page light or dark" — that one is fine for picking a direction, but it
/// cannot say whether two colours are far enough apart to read.
pub(crate) fn rel_luminance(c: Color) -> f32 {
    let Color::Rgb(r, g, b) = as_rgb(c) else { return 0.0 };
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// How far apart two colours read, as WCAG's contrast ratio: 1.0 is the same
/// colour, 21.0 is black on white. Around 4.5 is comfortable for body text.
pub(crate) fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (x, y) = (rel_luminance(a), rel_luminance(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// `c`, kept as itself but moved until it reads as text on `bg`.
///
/// For a colour that carries meaning — a mode's colour, a theme's accent —
/// used as text rather than as a chip. It keeps its hue and only gives up as
/// much lightness as it must: a theme accent chosen to look right *as* an
/// accent is often too close to that theme's own page to read a number in.
/// The selection band, lifted off a dark page until it can be seen.
///
/// Measured across the presets, every one of them — light and dark alike —
/// puts its selection between 1.1 and 1.4 times the contrast of its page. On a
/// light page that is enough: a slightly darker band under a row reads at a
/// glance. On a dark one it does not, and that is the report: "the light theme
/// shows me where I am and the dark one does not", with the *same* numbers
/// behind both.
///
/// So the rule is one-sided on purpose. A dark page has its selection pushed
/// away until it is unmistakable; a light page is left exactly as its author
/// drew it.
pub(crate) fn selection_on(page: Color, sel: Color) -> Color {
    /// Enough for a band to read as a band. Below this it is a shade.
    const WANT: f32 = 2.0;
    let (Color::Rgb(mut r, mut g, mut b), Color::Rgb(..)) = (sel, page) else { return sel };
    if rel_luminance(page) > 0.18 {
        return sel; // a light page: already legible, and its own author's choice
    }
    for _ in 0..48 {
        if contrast_ratio(Color::Rgb(r, g, b), page) >= WANT {
            break;
        }
        // Lifted, not tinted: the hue is the theme's and stays the theme's.
        r = (r as u16 + 6).min(255) as u8;
        g = (g as u16 + 6).min(255) as u8;
        b = (b as u16 + 6).min(255) as u8;
    }
    Color::Rgb(r, g, b)
}

pub(crate) fn text_tone(c: Color, bg: Color) -> Color {
    const WANT: f32 = 4.5;
    let (Color::Rgb(mut r, mut g, mut b), Color::Rgb(..)) = (c, bg) else { return c };
    // Away from the page: darker on a light one, lighter on a dark one.
    let darken = rel_luminance(bg) > 0.18;
    for _ in 0..24 {
        if contrast_ratio(Color::Rgb(r, g, b), bg) >= WANT {
            break;
        }
        let step = |v: u8| -> u8 {
            if darken {
                (v as i16 - 10).clamp(0, 255) as u8
            } else {
                (v as i16 + 10).clamp(0, 255) as u8
            }
        };
        let (nr, ng, nb) = (step(r), step(g), step(b));
        if (nr, ng, nb) == (r, g, b) {
            break; // saturated — this is as far as it goes
        }
        (r, g, b) = (nr, ng, nb);
    }
    Color::Rgb(r, g, b)
}

/// What a dialog row is drawn on: the dialog itself, or — for the row under
/// the cursor — the selection colour. Text on a row has to be measured
/// against whichever it actually lands on, and the two can be far apart: on
/// Solarized Light a grey that reads on the dialog is 3.6:1 on the selection.
pub(crate) fn row_bg(selected: bool) -> Color {
    if selected {
        theme().selected_bg
    } else {
        theme().popup_bg
    }
}

/// The theme's own dim colour, kept readable where it is used as *text*.
///
/// The presets choose `dim` for borders as much as for words, and a light
/// theme's border grey is nearly the page itself — which is right for a rule
/// and wrong for a column heading. Borders keep the colour as chosen; text
/// asks for this.
pub(crate) fn dim_text(bg: Color) -> Color {
    text_tone(theme().dim, bg)
}

/// Text one step quieter than body text, and still readable on `bg`.
///
/// For the things that are deliberately secondary — the tabs that are not
/// being read, a hint's description beside its key. A fixed grey cannot do
/// this job: the one picked against a dark page sat two shades from a light
/// one. Mixing the page's own readable colour back toward the page keeps the
/// *relationship* (quieter than the text beside it) on any theme.
pub(crate) fn muted_on(bg: Color) -> Color {
    let (Color::Rgb(tr_, tg, tb), Color::Rgb(br, bg_, bb)) = (readable_on(bg), bg) else {
        return readable_on(bg);
    };
    // Seven parts text to three parts page, backed off toward the text if
    // that lands too close to the page to read — on a mid-tone ground like
    // Dracula's selection there is not three parts of room to give away.
    for parts in [7u16, 8, 9] {
        let mix = |t: u8, b: u8| ((t as u16 * parts + b as u16 * (10 - parts)) / 10) as u8;
        let c = Color::Rgb(mix(tr_, br), mix(tg, bg_), mix(tb, bb));
        if contrast_ratio(c, bg) >= 4.5 {
            return c;
        }
    }
    readable_on(bg)
}

/// Inline Markdown within one text run: `**bold**` and `` `code` ``. Anything
/// that would cross a wrap boundary is simply left as plain text.
fn md_inline(text: &str, base: Style, code_c: Color) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<Span> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        // Inline code: `...`
        if chars[i] == '`' {
            if let Some(rel) = chars[i + 1..].iter().position(|&c| c == '`') {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), base));
                }
                let code: String = chars[i + 1..i + 1 + rel].iter().collect();
                spans.push(Span::styled(code, Style::default().fg(code_c)));
                i = i + rel + 2;
                continue;
            }
        }
        // Bold: **...**
        if chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            if let Some(rel) = chars[i + 2..].windows(2).position(|w| w == ['*', '*']) {
                let end = i + 2 + rel;
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), base));
                }
                let b: String = chars[i + 2..end].iter().collect();
                spans.push(Span::styled(b, base.add_modifier(Modifier::BOLD)));
                i = end + 2;
                continue;
            }
        }
        buf.push(chars[i]);
        i += 1;
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, base));
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

/// Render one raw line of an assistant answer as Markdown into wrapped, styled
/// lines paired with their plain text (for copy / scroll mapping). `in_code`
/// carries fenced-code-block state between lines; `gutter` is the speaker bar.
fn md_body_line(
    raw: &str,
    width: usize,
    gutter: Color,
    body_c: Color,
    in_code: &mut bool,
) -> Vec<(String, Line<'static>)> {
    let code_c = Color::Rgb(206, 145, 120);
    let head_c = Color::Rgb(120, 190, 255);
    let quote_c = muted_on(theme().popup_bg);
    let w = width.saturating_sub(2).max(1);
    let bar = || Span::styled("▏ ", Style::default().fg(gutter));
    let mut out: Vec<(String, Line)> = Vec::new();
    let trimmed = raw.trim_start();

    // A ``` fence toggles code mode and draws a faint rule.
    if trimmed.starts_with("```") {
        *in_code = !*in_code;
        out.push((
            raw.to_string(),
            Line::from(vec![bar(), Span::styled("─".repeat(w.min(40)), Style::default().fg(quote_c))]),
        ));
        return out;
    }
    if *in_code {
        for chunk in wrap_str(raw, w) {
            let line = Line::from(vec![bar(), Span::styled(chunk.clone(), Style::default().fg(code_c))]);
            out.push((chunk, line));
        }
        return out;
    }
    // Heading: one-to-three leading '#'.
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if (1..=3).contains(&hashes) && trimmed.chars().nth(hashes) == Some(' ') {
        let text = trimmed[hashes + 1..].trim();
        for chunk in wrap_str(text, w) {
            let line = Line::from(vec![
                bar(),
                Span::styled(chunk.clone(), Style::default().fg(head_c).add_modifier(Modifier::BOLD)),
            ]);
            out.push((chunk, line));
        }
        return out;
    }
    // Bullet: "- " / "* " → "• "
    if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
        let mut first = true;
        for chunk in wrap_str(rest, w.saturating_sub(2)) {
            let marker = if first { "• " } else { "  " };
            let mut spans = vec![bar(), Span::styled(marker, Style::default().fg(gutter))];
            spans.extend(md_inline(&chunk, Style::default().fg(body_c), code_c));
            out.push((format!("{marker}{chunk}"), Line::from(spans)));
            first = false;
        }
        return out;
    }
    // Blockquote: "> "
    if let Some(rest) = trimmed.strip_prefix("> ") {
        for chunk in wrap_str(rest, w.saturating_sub(2)) {
            let line = Line::from(vec![
                bar(),
                Span::styled("│ ", Style::default().fg(quote_c)),
                Span::styled(chunk.clone(), Style::default().fg(quote_c).add_modifier(Modifier::ITALIC)),
            ]);
            out.push((chunk, line));
        }
        return out;
    }
    // Plain paragraph with inline styling.
    for chunk in wrap_str(raw, w) {
        let mut spans = vec![bar()];
        spans.extend(md_inline(&chunk, Style::default().fg(body_c), code_c));
        out.push((chunk, Line::from(spans)));
    }
    out
}

fn draw_ai_chat(f: &mut Frame, area: Rect, app: &mut App) {
    let lang = app.lang;
    // How this window presents itself: the action that opened it, and whether
    // the local model or crmaine is answering.
    let skin = if let Popup::AiChat { skin, .. } = &app.popup {
        skin.clone()
    } else {
        ChatSkin::of(ChatMode::Ai)
    };
    let width: u16 = 76u16.min(area.width.saturating_sub(2));
    let height = area.height.saturating_sub(2).max(8);
    let rect = centered_rect(width, height, area);
    clear_popup(f, rect);
    // Each backend wears its own colour, so the frame alone says who is
    // answering: crmaine's signature carmine (the same frame the remote pane
    // wears), and cyan for the local model.
    let accent = text_tone(if skin.simple { AI_SIMPLE } else { CRMAINE }, theme().popup_bg);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
        .style(popup_style())
        .title(Line::from(vec![
            Span::styled(" ◈ ", Style::default().fg(accent).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{} ", skin.title),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
        ]))
        .title_bottom(tr(
            lang,
            " Enter=send  Shift+Enter=newline  Ctrl+V=paste  Ctrl+R=history  Ctrl+D=what it read  Esc=stop/close ",
            " Enter=送信  Shift+Enter=改行  Ctrl+V=貼り付け  Ctrl+R=履歴  Ctrl+D=拾った断片  Esc=中断/閉じる ",
        ));
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);
    let body_w = inner.width.max(1) as usize;

    // The input can be several lines (Alt+Enter); the transcript gives up a row
    // per extra input line, capped so a huge paste can't swallow the answer.
    // Wrapped, not just split: a long line typed or pasted in one piece used
    // to run off the right-hand edge, so what had been typed could not be
    // read back. Capped, so a huge paste still leaves the answer on screen —
    // and it is the *end* that is kept, which is where the caret is.
    let input_wrapped: Vec<String> = if let Popup::AiChat { input, .. } = &app.popup {
        wrap_input(input, inner.width.saturating_sub(2) as usize)
    } else {
        Vec::new()
    };
    let input_rows = input_wrapped.len().clamp(1, 8);
    // Pasted images get their own row above the input, so the count is visible
    // before sending rather than only in the transient status message.
    let attach_n = app.chat_attachments.len();
    let attach_rows = u16::from(attach_n > 0);
    let view_h = inner.height.saturating_sub(input_rows as u16 + attach_rows) as usize;

    let mut flat: Vec<String> = Vec::new();
    let mut shown: Vec<Line> = Vec::new();
    let mut off = 0usize;
    if let Popup::AiChat { log, scroll, pending, sel, .. } = &mut app.popup {
        // Flat plain-text lines (for copying) and their styled counterparts.
        // Each turn is a speaker header line followed by the wrapped body,
        // indented — the "crmaine - Ajent" name is too long to sit inline.
        let mut styled: Vec<Line> = Vec::new();
        // Message text must contrast with the popup ground under any theme.
        let body_c = readable_on(theme().popup_bg);
        let source_c = Color::Rgb(150, 175, 205);
        let dim_c = muted_on(theme().popup_bg);
        for m in log.iter() {
            // The assistant signs with the backend that actually answered — a
            // reply from the local model must not read as crmaine's work.
            let (glyph, name, name_c) = if m.user {
                ("▍", tr(lang, "you", "あなた"), text_tone(theme().accent, theme().popup_bg))
            } else if skin.simple {
                ("◆", "AI - simple", accent)
            } else {
                ("◆", tr(lang, "crmaine", "カーマイン"), accent)
            };
            styled.push(Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(name_c).add_modifier(Modifier::BOLD)),
                Span::styled(name.to_string(), Style::default().fg(name_c).add_modifier(Modifier::BOLD)),
            ]));
            flat.push(name.to_string());
            // Once crmaine's "— sources —" rule appears, the rest of the turn is
            // its citation list; render those quietly and in a link-ish blue.
            let mut in_sources = false;
            let mut in_code = false;
            for raw in m.text.split('\n') {
                if raw.trim() == "— sources —" {
                    in_sources = true;
                    styled.push(Line::from(vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(
                            tr(lang, "sources", "参照元"),
                            Style::default().fg(dim_c).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    flat.push(raw.to_string());
                    continue;
                }
                // The assistant's prose is Markdown; the user's text and the
                // citation list stay literal.
                if !m.user && !in_sources {
                    for (plain, line) in md_body_line(raw, body_w, name_c, body_c, &mut in_code) {
                        styled.push(line);
                        flat.push(plain);
                    }
                    continue;
                }
                let text_c = if in_sources { source_c } else { body_c };
                for chunk in wrap_str(raw, body_w.saturating_sub(2)) {
                    styled.push(Line::from(vec![
                        // A thin gutter in the speaker's colour gives the thread
                        // a chat feel without boxing every message.
                        Span::styled("▏ ", Style::default().fg(name_c)),
                        Span::styled(chunk.clone(), Style::default().fg(text_c)),
                    ]));
                    flat.push(chunk);
                }
            }
            styled.push(Line::from(""));
            flat.push(String::new());
        }
        if *pending {
            // A spinner in the backend's colour, driven off the wall clock so it
            // turns while the answer is in flight (the loop force-repaints
            // meanwhile). See [`spinner_frame`] for why it is not braille.
            let frame = spinner_frame(app.startup_at.elapsed().as_millis());
            let label = tr(lang, "AI - simple is thinking…", "AI - simple が考えています…");
            styled.push(Line::from(vec![
                Span::styled(
                    format!("{frame} "),
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(label, Style::default().fg(dim_c).add_modifier(Modifier::ITALIC)),
            ]));
            flat.push(String::new());
        }
        let max_scroll = flat.len().saturating_sub(view_h);
        off = (*scroll).min(max_scroll);
        *scroll = off; // usize::MAX means "stick to bottom"; clamp it here

        let sel_range = sel.map(|(a, b)| (a.min(b), a.max(b)));
        for (i, line) in styled.into_iter().enumerate().skip(off).take(view_h) {
            let selected = sel_range.map(|(a, b)| i >= a && i <= b).unwrap_or(false);
            shown.push(if selected {
                line.style(Style::default().bg(theme().selected_bg))
            } else {
                line
            });
        }
    }

    // Stash the geometry so a mouse drag can map to a line range and copy it.
    app.ai_rect = Rect::new(inner.x, inner.y, inner.width, view_h as u16);
    app.ai_scroll = off;
    app.ai_lines = flat;

    f.render_widget(Paragraph::new(shown), app.ai_rect);
    if attach_rows > 0 {
        let label = match lang {
            Lang::Ja => format!("画像 {attach_n} 枚"),
            Lang::En if attach_n == 1 => "1 image".to_string(),
            Lang::En => format!("{attach_n} images"),
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("▣ {label}"),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ))),
            Rect::new(inner.x, inner.y + view_h as u16, inner.width, 1),
        );
    }
    // The input, possibly several lines. A "> " prompt on the first row, aligned
    // continuation on the rest, and a block caret at the very end.
    let in_style = Style::default()
        .fg(readable_on(theme().selected_bg))
        .add_modifier(Modifier::BOLD)
        .bg(theme().selected_bg);
    // The tail of it: with more than fits, the end is what is being typed.
    let first = input_wrapped.len().saturating_sub(input_rows);
    let last = input_wrapped.len().saturating_sub(1);
    let mut in_lines: Vec<Line> = Vec::with_capacity(input_rows);
    for (i, seg) in input_wrapped.iter().enumerate().skip(first) {
        let prefix = if i == 0 { "> " } else { "  " };
        let caret = if i == last { "\u{2588}" } else { "" };
        in_lines.push(Line::from(Span::styled(format!("{prefix}{seg}{caret}"), in_style)));
    }
    f.render_widget(
        Paragraph::new(in_lines).style(Style::default().bg(theme().selected_bg)),
        Rect::new(inner.x, inner.y + view_h as u16 + attach_rows, inner.width, input_rows as u16),
    );
}

/// What is being typed, laid out in rows of `cols` columns.
///
/// Explicit line breaks split; anything longer than the width wraps, because
/// a prompt you cannot read back is a prompt you cannot correct. Measured in
/// columns rather than characters, so a line of Japanese wraps where it looks
/// like it should.
fn wrap_input(text: &str, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    for seg in text.split('\n') {
        if seg.is_empty() || cols == 0 {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        let mut w = 0usize;
        for ch in seg.chars() {
            let cw = cian_core::textops::char_cols(ch, w);
            if w + cw > cols {
                out.push(std::mem::take(&mut cur));
                w = 0;
            }
            cur.push(ch);
            w += cw;
        }
        out.push(cur);
    }
    out
}

/// The operation queue (`:queue`): the running op with its progress and
/// stall age, then everything waiting its turn.
fn draw_op_queue(f: &mut Frame, area: Rect, app: &mut App) {
    let lang = app.lang;
    let cursor = match &app.popup {
        Popup::OpQueue { cursor } => *cursor,
        _ => return,
    };
    let w = 60u16.min(area.width.saturating_sub(2));
    let n_rows = 1 + app.op_queue.len();
    let h = (n_rows as u16 + 4).clamp(6, area.height.saturating_sub(2));
    let inner = popup_frame(
        f,
        area,
        w,
        h,
        tr(lang, " operation queue ", " 操作キュー "),
        tr(lang, " x=stop/remove (x again=abandon)  Esc ", " x=停止/削除（再度x=見捨て）  Esc "),
    );
    let body_c = readable_on(theme().popup_bg);
    let mut lines: Vec<Line> = Vec::new();
    // Row 0: the runner.
    match &app.op_job {
        Some(job) => {
            let p = &job.latest;
            let pct = if let Some(f) = (p.bytes_done * 100).checked_div(p.bytes_total) {
                format!("{}%", f.min(100))
            } else {
                format!("{}/{}", p.files_done, p.files_total)
            };
            let stalled = app.op_stalled();
            let state = if job.cancel_requested.is_some() {
                tr(lang, "stopping…", "停止中…").to_string()
            } else if stalled {
                let s = job.last_progress.elapsed().as_secs();
                if lang == Lang::Ja { format!("⚠ 停滞 {}秒", s) } else { format!("⚠ stalled {}s", s) }
            } else {
                tr(lang, "running", "実行中").to_string()
            };
            let c = if stalled { Color::Rgb(235, 200, 100) } else { Color::Rgb(130, 205, 150) };
            lines.push(Line::from(vec![
                Span::styled(if cursor == 0 { "▶ " } else { "  " }, Style::default().fg(text_tone(theme().accent, theme().popup_bg))),
                Span::styled(format!("{} {} ", job.label, pct), Style::default().fg(body_c).add_modifier(Modifier::BOLD)),
                Span::styled(state, Style::default().fg(c)),
            ]));
        }
        None => lines.push(Line::from(Span::styled(
            tr(lang, "  (nothing running)", "  （実行中なし）"),
            Style::default().fg(dim_text(theme().popup_bg)),
        ))),
    }
    // The waiting line.
    for (i, q) in app.op_queue.iter().enumerate() {
        let sel = cursor == i + 1;
        lines.push(Line::from(vec![
            Span::styled(if sel { "▶ " } else { "  " }, Style::default().fg(text_tone(theme().accent, theme().popup_bg))),
            Span::styled(
                format!("{}. {}", i + 1, q.label),
                Style::default().fg(if sel { body_c } else { dim_text(theme().popup_bg) }),
            ),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// The UI-toggles menu: each switch with its current state, cursor-highlighted.
fn draw_toggles(f: &mut Frame, area: Rect, app: &App) {
    let lang = app.lang;
    let Popup::Toggles { cursor } = &app.popup else { return };
let cursor = *cursor;
let rows = app.toggle_rows();
let width: u16 = 42u16.min(area.width.saturating_sub(2));
let height = (rows.len() as u16 + 3).clamp(5, area.height.saturating_sub(2));
let rect = centered_rect(width, height, area);
clear_popup(f, rect);
let block = Block::default()
    .borders(Borders::ALL)
    .border_type(border_type())
    .border_style(accent_on_popup())
    .style(popup_style())
    .title(tr(lang, " toggles ", " トグル "))
    .title_bottom(tr(lang, " Enter/Space=flip  ↑↓  Esc ", " Enter/Space=切替  ↑↓  Esc "));
let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
f.render_widget(block, rect);

let body_c = readable_on(theme().popup_bg);
let dim_c = muted_on(theme().popup_bg);
let on_c = text_tone(Color::Rgb(130, 205, 150), theme().popup_bg);
let w = inner.width as usize;
let mut lines: Vec<Line> = Vec::new();
for (i, (_, label, state, on)) in rows.iter().enumerate() {
    let sel = i == cursor;
    let marker = if sel { "▶ " } else { "  " };
    // Right-align the state text on the row.
    let pad = w.saturating_sub(2 + label.chars().count() + state.chars().count()).max(1);
    let label_style = if sel {
        Style::default().fg(readable_on(theme().selected_bg)).bg(theme().selected_bg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(body_c)
    };
    let state_style = if *on {
        Style::default().fg(on_c).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(dim_c)
    };
    lines.push(Line::from(vec![
        Span::styled(marker, Style::default().fg(text_tone(theme().accent, theme().popup_bg))),
        Span::styled(label.clone(), label_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(state.clone(), state_style),
    ]));
}
f.render_widget(Paragraph::new(lines), inner);
}

/// The chat history picker: past conversations this session, newest first.
fn draw_ai_history(f: &mut Frame, area: Rect, app: &App) {
    let lang = app.lang;
    let Popup::AiHistory { cursor } = &app.popup else { return };
let cursor = *cursor;
// This list mixes both backends' conversations, so it wears neither one's
// colour — each row carries its own badge instead.
let frame_c = text_tone(theme().accent, theme().popup_bg);
let dim_c = muted_on(theme().popup_bg);
let width: u16 = 72u16.min(area.width.saturating_sub(2));
let height = (app.ai_history.len() as u16 + 3).clamp(6, area.height.saturating_sub(2));
let rect = centered_rect(width, height, area);
clear_popup(f, rect);
let block = Block::default()
    .borders(Borders::ALL)
    .border_type(border_type())
    .border_style(Style::default().fg(frame_c).add_modifier(Modifier::BOLD))
    .style(popup_style())
    .title(tr(lang, " chat history ", " チャット履歴 "))
    .title_bottom(tr(lang, " Enter=open  d=delete  ↑↓  Esc ", " Enter=開く  d=削除  ↑↓  Esc "));
let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
f.render_widget(block, rect);

let body_c = readable_on(theme().popup_bg);
let view_h = inner.height as usize;
let first = if cursor >= view_h { cursor + 1 - view_h } else { 0 };
let mut lines: Vec<Line> = Vec::new();
for (i, c) in app.ai_history.iter().enumerate().skip(first).take(view_h) {
    let sel = i == cursor;
    let log = c.log();
    let title = App::ai_history_title(log);
    let turns = log.iter().filter(|m| m.user).count();
    let marker = if sel { "▶ " } else { "  " };
    let badge = format!("{:<6} ", c.mode().badge());
    let title_style = if sel {
        Style::default()
            .fg(readable_on(theme().selected_bg))
            .bg(theme().selected_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(body_c)
    };
    lines.push(Line::from(vec![
        Span::styled(marker, Style::default().fg(frame_c)),
        Span::styled(badge, Style::default().fg(dim_c)),
        Span::styled(title, title_style),
        Span::styled(format!("  ({turns})"), Style::default().fg(dim_c)),
    ]));
}
f.render_widget(Paragraph::new(lines), inner);
}

/// The editable commit-message preview. `editing` shows a caret and a different
/// footer; otherwise it is a read-only preview with commit / edit / cancel keys.
fn draw_commit_message(f: &mut Frame, area: Rect, app: &mut App) {
    let lang = app.lang;
    let Popup::CommitMessage { buffer, stat, editing, .. } = &app.popup else { return };
let editing = *editing;
let width: u16 = 80u16.min(area.width.saturating_sub(2));
let height = area.height.saturating_sub(2).clamp(10, 30);
let rect = centered_rect(width, height, area);
clear_popup(f, rect);
let title = if editing {
    tr(lang, " Draft commit message — editing ", " コミットメッセージ生成 — 編集中 ")
} else {
    tr(lang, " Draft commit message ", " コミットメッセージ生成 ")
};
let footer = if editing {
    tr(lang, " type to edit   Enter=newline   Esc=done editing ",
          " 入力で編集   Enter=改行   Esc=編集終了 ")
} else {
    tr(lang, " Enter/c=commit   e=edit   Esc=cancel ",
          " Enter/c=コミット   e=編集   Esc=取消 ")
};
let block = Block::default()
    .borders(Borders::ALL)
    .border_type(border_type())
    .border_style(Style::default().fg(text_tone(AI_SIMPLE, theme().popup_bg)).add_modifier(Modifier::BOLD))
    .style(popup_style())
    .title(title)
    .title_bottom(footer);
let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
f.render_widget(block, rect);

let body_w = inner.width.max(1) as usize;
let mut lines: Vec<Line> = Vec::new();
// The staged-files summary, quietly, so the reviewer sees what it covers.
if !stat.is_empty() {
    for raw in stat.lines() {
        for chunk in wrap_str(raw, body_w) {
            lines.push(Line::from(Span::styled(
                chunk,
                Style::default().fg(muted_on(theme().popup_bg)),
            )));
        }
    }
    lines.push(Line::from(Span::styled(
        "─".repeat(body_w.min(60)),
        Style::default().fg(dim_text(theme().popup_bg)),
    )));
}
// The message itself. A trailing block marks the edit point when editing.
let subject_c = readable_on(theme().popup_bg);
let body_c = muted_on(theme().popup_bg);
let shown = if editing { format!("{}\u{2588}", buffer) } else { buffer.clone() };
for (i, raw) in shown.split('\n').enumerate() {
    let c = if i == 0 { subject_c } else { body_c };
    let modifier = if i == 0 { Modifier::BOLD } else { Modifier::empty() };
    let wrapped = wrap_str(raw, body_w);
    if wrapped.is_empty() {
        lines.push(Line::from(""));
    }
    for chunk in wrapped {
        lines.push(Line::from(Span::styled(chunk, Style::default().fg(c).add_modifier(modifier))));
    }
}
f.render_widget(Paragraph::new(lines), inner);
}

/// The junk-review list: a checkbox per candidate, its name, size and the
/// reason the AI gave. Nothing is deleted here — Enter hands the checked ones
/// to the normal delete confirmation.
fn draw_junk_review(f: &mut Frame, area: Rect, app: &mut App) {
    let lang = app.lang;
    let width: u16 = 88u16.min(area.width.saturating_sub(2));
    let height = area.height.saturating_sub(2).clamp(8, 30);
    let rect = centered_rect(width, height, area);
    clear_popup(f, rect);
    let (n, checked) = if let Popup::JunkReview { items, .. } = &app.popup {
        (items.len(), items.iter().filter(|i| i.selected).count())
    } else {
        (0, 0)
    };
    let title = if lang == Lang::Ja {
        format!(" ゴミファイル検出  {}/{} 選択 ", checked, n)
    } else {
        format!(" Detect junk files  {}/{} checked ", checked, n)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(Style::default().fg(text_tone(AI_SIMPLE, theme().popup_bg)).add_modifier(Modifier::BOLD))
        .style(popup_style())
        .title(title)
        .title_bottom(tr(lang,
            " Space/click=toggle  a=all  Enter/d=delete checked  Esc=cancel ",
            " Space/クリック=切替  a=全て  Enter/d=選択を削除  Esc=取消 "));
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);

    let body_h = inner.height as usize;
    let body_w = inner.width as usize;
    let mut rows: Vec<Line> = Vec::new();
    if let Popup::JunkReview { items, cursor, scroll } = &mut app.popup {
        // Keep the cursor in view.
        keep_in_view(*cursor, scroll, body_h);
        for (i, it) in items.iter().enumerate().skip(*scroll).take(body_h) {
            let sel = i == *cursor;
            let checkbox = if it.selected { "[x] " } else { "[ ] " };
            let box_c = if it.selected { theme().mark_fg } else { Color::Rgb(120, 120, 140) };
            let name = it.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let name_c = if sel {
                text_tone(theme().accent, row_bg(sel))
            } else {
                readable_on(row_bg(sel))
            };
            let reason = if it.reason.is_empty() { String::new() } else { format!("— {}", it.reason) };
            let base = if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
            rows.push(Line::from(vec![
                Span::styled(checkbox, base.fg(box_c).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{}  ", pad_to(&truncate_middle(&name, 28), 28)),
                    base.fg(name_c).add_modifier(Modifier::BOLD)),
                Span::styled(truncate(&reason, body_w.saturating_sub(36)),
                    base.fg(muted_on(row_bg(sel)))),
            ]));
        }
        app.junk_rect = Rect::new(inner.x, inner.y, inner.width, body_h.min(items.len().saturating_sub(*scroll)) as u16);
    }
    f.render_widget(Paragraph::new(rows), inner);
}

/// The duplicate-file review: files grouped by identical content, a checkbox
/// per copy (the keeper of each group left unchecked). Enter deletes the checked.
fn draw_dupe_review(f: &mut Frame, area: Rect, app: &mut App) {
    let lang = app.lang;
    let width: u16 = 96u16.min(area.width.saturating_sub(2));
    let height = area.height.saturating_sub(2).clamp(8, 30);
    let rect = centered_rect(width, height, area);
    clear_popup(f, rect);
    let (n, checked) = if let Popup::DupeReview { items, .. } = &app.popup {
        (items.len(), items.iter().filter(|i| i.selected).count())
    } else {
        (0, 0)
    };
    let title = if lang == Lang::Ja {
        format!(" 重複ファイル  {}/{} 選択 ", checked, n)
    } else {
        format!(" duplicate files  {}/{} checked ", checked, n)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(accent_on_popup())
        .style(popup_style())
        .title(title)
        .title_bottom(tr(lang,
            " Space/click=toggle  a=all  Enter/d=delete checked  Esc=cancel ",
            " Space/クリック=切替  a=全て  Enter/d=選択を削除  Esc=取消 "));
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);

    let body_h = inner.height as usize;
    let body_w = inner.width as usize;
    let mut rows: Vec<Line> = Vec::new();
    if let Popup::DupeReview { items, cursor, scroll } = &mut app.popup {
        keep_in_view(*cursor, scroll, body_h);
        for (i, it) in items.iter().enumerate().skip(*scroll).take(body_h) {
            let sel = i == *cursor;
            // A group-change gets a subtle "#N" tag so the groups read apart.
            let group_start = i == 0 || items.get(i.wrapping_sub(1)).map(|p| p.group != it.group).unwrap_or(true);
            let checkbox = if it.selected { "[x] " } else { "[ ] " };
            let box_c = if it.selected { theme().mark_fg } else { Color::Rgb(120, 120, 140) };
            let base = if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
            let tag = if group_start { format!("#{} ", it.group + 1) } else { "   ".to_string() };
            let path_c = if it.keeper {
                text_tone(Color::Rgb(130, 205, 150), theme().popup_bg)
            } else {
                readable_on(theme().popup_bg)
            };
            let suffix = if it.keeper { tr(lang, "  (keep)", "  (残す)") } else { "" };
            let shown = it.path.display().to_string();
            rows.push(Line::from(vec![
                Span::styled(checkbox, base.fg(box_c).add_modifier(Modifier::BOLD)),
                Span::styled(tag, base.fg(muted_on(row_bg(sel)))),
                Span::styled(truncate_middle(&shown, body_w.saturating_sub(14)), base.fg(path_c)),
                Span::styled(suffix, base.fg(text_tone(Color::Rgb(130, 205, 150), row_bg(sel)))),
            ]));
        }
        app.dupe_rect = Rect::new(inner.x, inner.y, inner.width, body_h.min(items.len().saturating_sub(*scroll)) as u16);
    }
    f.render_widget(Paragraph::new(rows), inner);
}

/// The structure-suggestion review: a checkbox per proposed move showing
/// `name → folder/`, with the AI's reason. Enter runs the checked moves.
fn draw_structure_review(f: &mut Frame, area: Rect, app: &mut App) {
    let lang = app.lang;
    let width: u16 = 92u16.min(area.width.saturating_sub(2));
    let height = area.height.saturating_sub(2).clamp(8, 30);
    let rect = centered_rect(width, height, area);
    clear_popup(f, rect);
    let (n, checked) = if let Popup::StructureReview { items, .. } = &app.popup {
        (items.len(), items.iter().filter(|i| i.selected).count())
    } else {
        (0, 0)
    };
    let title = if lang == Lang::Ja {
        format!(" ディレクトリ構成を提案  {}/{} 選択 ", checked, n)
    } else {
        format!(" Suggest folder structure  {}/{} checked ", checked, n)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(Style::default().fg(text_tone(AI_SIMPLE, theme().popup_bg)).add_modifier(Modifier::BOLD))
        .style(popup_style())
        .title(title)
        .title_bottom(tr(lang,
            " Space/click=toggle  a=all  Enter/m=move checked  Esc=cancel ",
            " Space/クリック=切替  a=全て  Enter/m=選択を移動  Esc=取消 "));
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);

    let body_h = inner.height as usize;
    let body_w = inner.width as usize;
    let mut rows: Vec<Line> = Vec::new();
    if let Popup::StructureReview { items, cursor, scroll, .. } = &mut app.popup {
        keep_in_view(*cursor, scroll, body_h);
        for (i, it) in items.iter().enumerate().skip(*scroll).take(body_h) {
            let sel = i == *cursor;
            let checkbox = if it.selected { "[x] " } else { "[ ] " };
            let box_c = if it.selected { theme().mark_fg } else { Color::Rgb(120, 120, 140) };
            let name_c = if sel {
                text_tone(theme().accent, row_bg(sel))
            } else {
                readable_on(row_bg(sel))
            };
            let base = if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
            // `name  →  folder/`, then the reason quietly at the end.
            let arrow = format!("{}  →  {}/", pad_to(&truncate_middle(&it.name, 26), 26), it.dest);
            let reason = if it.reason.is_empty() { String::new() } else { format!("   — {}", it.reason) };
            rows.push(Line::from(vec![
                Span::styled(checkbox, base.fg(box_c).add_modifier(Modifier::BOLD)),
                Span::styled(truncate(&arrow, body_w.saturating_sub(6)),
                    base.fg(name_c).add_modifier(Modifier::BOLD)),
                Span::styled(truncate(&reason, body_w.saturating_sub(4)),
                    base.fg(muted_on(row_bg(sel)))),
            ]));
        }
        app.struct_rect = Rect::new(inner.x, inner.y, inner.width, body_h.min(items.len().saturating_sub(*scroll)) as u16);
    }
    f.render_widget(Paragraph::new(rows), inner);
}

/// The bulk-rename review: a checkbox per proposed rename showing `old → new`.
/// Enter renames the checked files in place.
fn draw_rename_review(f: &mut Frame, area: Rect, app: &mut App) {
    let lang = app.lang;
    let width: u16 = 92u16.min(area.width.saturating_sub(2));
    let height = area.height.saturating_sub(2).clamp(8, 30);
    let rect = centered_rect(width, height, area);
    clear_popup(f, rect);
    let (n, checked, by_ai) = if let Popup::RenameReview { items, by_ai, .. } = &app.popup {
        (items.len(), items.iter().filter(|i| i.selected).count(), *by_ai)
    } else {
        (0, 0, false)
    };
    // Named for whichever side proposed the renames: the AI menu item, or the
    // `:brename` pattern.
    let head = match (by_ai, lang) {
        (true, Lang::Ja) => "AIリネーム",
        (true, Lang::En) => "AI rename",
        (false, Lang::Ja) => "リネーム候補",
        (false, Lang::En) => "proposed renames",
    };
    let title = if lang == Lang::Ja {
        format!(" {}  {}/{} 選択 ", head, checked, n)
    } else {
        format!(" {}  {}/{} checked ", head, checked, n)
    };
    let accent = text_tone(if by_ai { AI_SIMPLE } else { theme().accent }, theme().popup_bg);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
        .style(popup_style())
        .title(title)
        .title_bottom(tr(lang,
            " Space/click=toggle  a=all  Enter/r=rename checked  Esc=cancel ",
            " Space/クリック=切替  a=全て  Enter/r=選択をリネーム  Esc=取消 "));
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);

    let body_h = inner.height as usize;
    let body_w = inner.width as usize;
    let half = body_w.saturating_sub(8) / 2;
    let mut rows: Vec<Line> = Vec::new();
    if let Popup::RenameReview { items, cursor, scroll, .. } = &mut app.popup {
        keep_in_view(*cursor, scroll, body_h);
        for (i, it) in items.iter().enumerate().skip(*scroll).take(body_h) {
            let sel = i == *cursor;
            let checkbox = if it.selected { "[x] " } else { "[ ] " };
            let box_c = if it.selected { theme().mark_fg } else { Color::Rgb(120, 120, 140) };
            let base = if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
            let old_c = if sel { readable_on(theme().popup_bg) } else { Color::Rgb(200, 200, 215) };
            rows.push(Line::from(vec![
                Span::styled(checkbox, base.fg(box_c).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{}  →  ", pad_to(&truncate_middle(&it.old, half), half)),
                    base.fg(old_c)),
                Span::styled(truncate_middle(&it.new, half),
                    base.fg(theme().accent).add_modifier(Modifier::BOLD)),
            ]));
        }
        app.rename_rect = Rect::new(inner.x, inner.y, inner.width, body_h.min(items.len().saturating_sub(*scroll)) as u16);
    }
    f.render_widget(Paragraph::new(rows), inner);
}

/// The frame nearly every popup wears: a centred `w`×`h` box, cleared, bordered
/// in the accent colour, with `title` along the top and `footer` along the
/// bottom. Returns the inner area to draw into.
///
/// Pass `""` for a title or footer the popup does not want — an empty one draws
/// nothing. The handful of popups that need something else (their own anchor
/// rect, a filled background, a tighter margin) still build their own block;
/// this is the common case, not a mandate.
/// Keep `cursor` inside the `body_h` rows that start at `scroll`.
///
/// Ten copies of this existed and they did not agree: some guarded on
/// `body_h > 0` and some did not. Without the guard a zero-height body makes
/// `cursor >= scroll + 0` true the moment the cursor is at or past the scroll,
/// and the list scrolls to `cursor + 1` — past the end of something that has no
/// room to show a row in the first place.
fn keep_in_view(cursor: usize, scroll: &mut usize, body_h: usize) {
    if cursor < *scroll {
        *scroll = cursor;
    } else if body_h > 0 && cursor >= *scroll + body_h {
        *scroll = cursor + 1 - body_h;
    }
}

fn popup_frame<'a>(
    f: &mut Frame,
    area: Rect,
    w: u16,
    h: u16,
    title: impl Into<Line<'a>>,
    footer: impl Into<Line<'a>>,
) -> Rect {
    popup_frame_in(f, area, w, h, title, footer, text_tone(theme().accent, theme().popup_bg))
}

/// The same frame in a chosen colour — for the AI - simple windows, which wear
/// [`AI_SIMPLE`] instead of the theme accent.
#[allow(clippy::too_many_arguments)]
fn popup_frame_in<'a>(
    f: &mut Frame,
    area: Rect,
    w: u16,
    h: u16,
    title: impl Into<Line<'a>>,
    footer: impl Into<Line<'a>>,
    accent: Color,
) -> Rect {
    let rect = centered_rect(w, h, area);
    clear_popup(f, rect);
    // `Clear` empties the cells; it does not colour them. Without a surface
    // of its own a dialog shows the terminal's background, which is no
    // theme's — the one place the palette never reached.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
        .style(popup_style())
        .title(title)
        .title_bottom(footer);
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);
    inner
}

#[allow(clippy::too_many_arguments)]
fn draw_popup(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    hosts: &[cian_lua::SshHost],
    snippets: &[cian_lua::Snippet],
    find: Option<(&str, &str, Option<cian_core::search::Outcome>, cian_core::search::Mode)>,
    dests: &[(String, PathBuf)],
    zones: &mut Vec<PopupZone>,
    lang: Lang,
    menu_lang: Lang,
    show_ws: bool,
    ruler: bool,
    tab_at: usize,
    tab_names: &[String],
    tab_rects: &mut Vec<(Rect, usize)>,
    close_rect: &mut Rect,
    docked: bool,
    active: bool,
    tracks: &mut Vec<crate::ScrollTrack>,
) {
    // Every popup with a shape of its own draws itself. The rest — the
    // confirm/notice dialogs, which differ only in their wording — fall through
    // to the one renderer they share.
    match popup {
        Popup::ThemePicker { .. } => draw_theme_picker(f, area, popup, lang),
        Popup::Manual { .. } => draw_manual(f, area, popup, lang),
        Popup::Report { .. } => draw_report(f, area, popup, lang),
        Popup::ContextMenu { .. } => draw_context_menu(f, area, popup, menu_lang),
        Popup::SshHosts { .. } => draw_ssh_hosts(f, area, popup, hosts, zones, lang),
        Popup::Snippets { .. } => draw_snippets(f, area, popup, snippets, zones, lang),
        Popup::RemoteBrowser { .. } => draw_remote_browser(f, area, popup, zones, lang),
        Popup::LocalDest { .. } => draw_local_dest(f, area, popup, zones, lang),
        Popup::SshUsers { .. } => draw_ssh_users(f, area, popup, hosts, zones, lang),
        Popup::FindResults { .. } => draw_find_results(f, area, popup, find, zones, lang),
        Popup::GrepReplace(_) => draw_grep_replace(f, area, popup, zones, lang),
        Popup::Shortcuts { .. } => draw_shortcuts(f, area, popup, zones, lang),
        Popup::History { .. } => draw_history(f, area, popup, zones, lang),
        Popup::DestPicker { .. } => draw_dest_picker(f, area, popup, dests, zones, lang),
        Popup::Viewer { .. } => {
            let (rects, close) =
                draw_viewer(f, area, popup, lang, (show_ws, ruler), (tab_at, tab_names, &[]), docked, active, tracks);
            *tab_rects = rects;
            *close_rect = close;
        }
        Popup::DirCompare { .. } => draw_dir_compare(f, area, popup, zones, lang),
        Popup::Diff { .. } => draw_diff(f, area, popup, lang),
        Popup::Archive { .. } => draw_archive(f, area, popup, zones, lang),
        Popup::Palette { .. } => draw_palette(f, area, popup, lang),
        Popup::DiskUsage { .. } => draw_disk_usage(f, area, popup, zones, lang),
        Popup::GitLog { .. } => draw_git_log(f, area, popup, zones, lang),
        Popup::Macros { .. } => draw_macros(f, area, popup, zones, lang),
        Popup::SortPicker { .. } => draw_sort_picker(f, area, popup, zones, lang),
        Popup::EncodingPicker { .. } => draw_encoding_picker(f, area, popup, zones, lang),
        Popup::ColorPicker { .. } => draw_color_picker(f, area, popup, zones, lang),
        _ => draw_simple_dialog(f, area, popup, zones, lang),
    }
}

/// The confirm/notice dialogs, which differ only in their text: each supplies
/// a title, body lines and a footer hint, and they share one frame, one body
/// paragraph and one button row.
fn draw_simple_dialog(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let popup: &Popup = popup;
    let (title, body, footer) = match popup {
        Popup::ConfirmDelete { targets } => {
            let title = tr(lang, " delete ", " 削除 ").to_string();
            let head = if lang == Lang::Ja {
                format!("{} 件 → ゴミ箱:", targets.len())
            } else {
                format!("{} item(s) → trash:", targets.len())
            };
            let mut lines = vec![head, String::new()];
            for p in targets.iter().take(8) { lines.push(format!("  {}", p.display())); }
            if targets.len() > 8 {
                lines.push(tr_count(lang, targets.len() - 8));
            }
            let foot = tr(lang, " y/Enter=trash  a=delete permanently  n/Esc=cancel ",
                " y/Enter=ゴミ箱  a=完全削除  n/Esc=取消 ");
            (title, lines, foot.to_string())
        }
        Popup::ConfirmNoBom { targets } => {
            let title = tr(lang, " strip BOM ", " BOM除去 ").to_string();
            let head = if lang == Lang::Ja {
                format!("{} 件から UTF-8 BOM を除去します:", targets.len())
            } else {
                format!("strip the UTF-8 BOM from {} file(s):", targets.len())
            };
            let mut lines = vec![head, String::new()];
            for p in targets.iter().take(8) {
                lines.push(format!("  {}", p.display()));
            }
            if targets.len() > 8 {
                lines.push(tr_count(lang, targets.len() - 8));
            }
            lines.push(String::new());
            lines.push(
                tr(lang,
                   "UTF-16 files are detected and left alone (their BOM is load-bearing)",
                   "UTF-16 のファイルは検出してスキップします（BOM が必須のため）")
                .to_string(),
            );
            let foot = tr(lang, " y/Enter=strip  n/Esc=cancel ", " y/Enter=除去  n/Esc=取消 ");
            (title, lines, foot.to_string())
        }
        Popup::ConfirmZipAdd { archive, sub, sources } => {
            let title = tr(lang, " add to zip ", " zipへ追加 ").to_string();
            let where_ = format!(
                "{}{}{}",
                archive.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
                if sub.is_empty() { "" } else { "/" },
                sub
            );
            let head = if lang == Lang::Ja {
                format!("{} 件 → {}:", sources.len(), where_)
            } else {
                format!("{} item(s) → {}:", sources.len(), where_)
            };
            let mut lines = vec![head, String::new()];
            for p in sources.iter().take(8) {
                lines.push(format!("  {}", p.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()));
            }
            if sources.len() > 8 {
                lines.push(tr_count(lang, sources.len() - 8));
            }
            lines.push(String::new());
            lines.push(tr(lang, "same names inside the zip are replaced", "zip内の同名メンバーは置き換えられます").to_string());
            let foot = tr(lang, " y/Enter=add  n/Esc=cancel ", " y/Enter=追加  n/Esc=取消 ");
            (title, lines, foot.to_string())
        }
        Popup::ConfirmZipDelete { archive, members, shown } => {
            let title = tr(lang, " delete from zip ", " zipから削除 ").to_string();
            let head = if lang == Lang::Ja {
                format!(
                    "{} 件（メンバー {} 個）を {} から削除:",
                    shown.len(),
                    members.len(),
                    archive.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()
                )
            } else {
                format!(
                    "{} item(s) ({} member(s)) from {}:",
                    shown.len(),
                    members.len(),
                    archive.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()
                )
            };
            let mut lines = vec![head, String::new()];
            for m in shown.iter().take(8) {
                lines.push(format!("  {}", m));
            }
            if shown.len() > 8 {
                lines.push(tr_count(lang, shown.len() - 8));
            }
            lines.push(String::new());
            lines.push(tr(
                lang,
                "the zip is rewritten — there is no trash for this",
                "zipを書き直します — ゴミ箱には行きません",
            ).to_string());
            let foot = tr(lang, " y/Enter=delete  n/Esc=cancel ", " y/Enter=削除  n/Esc=取消 ");
            (title, lines, foot.to_string())
        }
        Popup::ConfirmTransfer { op, targets, dest } => {
            let title = match (op, lang) {
                (PendingOp::Copy, Lang::Ja) => " コピー ",
                (PendingOp::Move, Lang::Ja) => " 移動 ",
                (PendingOp::Copy, Lang::En) => " copy ",
                (PendingOp::Move, Lang::En) => " move ",
            }.to_string();
            let head = format!("{} {} → {}", targets.len(), tr(lang, "item(s)", "件"), dest.display());
            let mut lines = vec![head, String::new()];
            for p in targets.iter().take(8) { lines.push(format!("  {}", p.display())); }
            if targets.len() > 8 {
                lines.push(tr_count(lang, targets.len() - 8));
            }
            let foot = if targets.len() == 1 {
                tr(lang, " y/Enter=Yes  a=overwrite  r=rename  n/Esc=cancel ",
                    " y/Enter=実行  a=上書き  r=改名  n/Esc=取消 ")
            } else {
                tr(lang, " y/Enter=Yes(skip)  a=overwrite  n/Esc=cancel ",
                    " y/Enter=実行(重複はスキップ)  a=上書き  n/Esc=取消 ")
            };
            (title, lines, foot.to_string())
        }
        Popup::TextInput { title, prompt, kind, .. } => {
            // The field line is filled in below as styled Lines (the cursor
            // highlights a character rather than inserting one, so nothing
            // shifts as it moves).
            let body = vec![prompt.clone(), String::new()];
            // The fields that take a paragraph say so. A hint about a key that
            // does nothing is worse than no hint, so the plain ones keep theirs.
            let foot = if kind.is_multiline() {
                tr(
                    lang,
                    " Enter=ok  Shift+Enter=newline  ←→ move  Esc=cancel ",
                    " Enter=決定  Shift+Enter=改行  ←→ 移動  Esc=取消 ",
                )
            } else {
                tr(lang, " Enter=ok  ←→ move  Esc=cancel ", " Enter=決定  ←→ 移動  Esc=取消 ")
            };
            (format!(" {} ", title), body, foot.to_string())
        }
        Popup::Notice { lines } => {
            let title = tr(lang, " notice ", " お知らせ ").to_string();
            let foot = tr(lang, " y = copy   Enter / Esc = close ", " y = コピー   Enter / Esc = 閉じる ");
            (title, lines.clone(), foot.to_string())
        }
        Popup::Search { buffer } => {
            (
                tr(lang, " search ", " 検索 ").to_string(),
                vec![
                    tr(lang, "find (substring, case-insensitive):", "検索（部分一致・大小無視）:").to_string(),
                    format!("/{}_", buffer),
                ],
                tr(lang, " ↑↓ step matches  Enter=jump  Esc=cancel  (then n/N) ",
                    " ↑↓ マッチ移動  Enter=ジャンプ  Esc=取消  (後で n/N) ").to_string(),
            )
        }
        Popup::ConfirmQuit => {
            (
                tr(lang, " quit cian? ", " cian を終了？ ").to_string(),
                vec![tr(lang, "Are you sure you want to quit?", "本当に終了しますか？").to_string()],
                tr(lang, " y / Enter = yes   n / Esc = no ", " y / Enter = はい   n / Esc = いいえ ").to_string(),
            )
        }
        Popup::ConfirmClose { target } => {
            let what = match (target, lang) {
                (CloseTarget::ShellPane, Lang::Ja) => "このシェルペイン",
                // Named for what goes: a tab is every pane split inside it, and
                // whatever each of them is still running.
                (CloseTarget::ShellTab, Lang::Ja) => "このシェルタブ（分割ごと）",
                // Named for what is at stake, not for the file: the file is
                // on disk either way, and what this question is really about
                // is the part of it that is not.
                (CloseTarget::ViewerFile, Lang::Ja) => "このファイル — 未保存の変更は失われます",
                (CloseTarget::FileTab(_), Lang::Ja) => "このタブ",
                (CloseTarget::ShellPane, Lang::En) => "this shell pane",
                (CloseTarget::ShellTab, Lang::En) => "this shell tab, splits and all",
                (CloseTarget::ViewerFile, Lang::En) => "this file — unsaved changes will be lost",
                (CloseTarget::FileTab(_), Lang::En) => "this tab",
            };
            let head = if lang == Lang::Ja { format!("{}を閉じますか？", what) } else { format!("Close {}?", what) };
            (
                tr(lang, " close? ", " 閉じる？ ").to_string(),
                vec![head],
                tr(lang, " y / Enter = yes   n / Esc = no ", " y / Enter = はい   n / Esc = いいえ ").to_string(),
            )
        }
        Popup::ConfirmNewTab { .. } => (
            tr(lang, " new tab ", " 新しいタブ ").to_string(),
            vec![tr(lang, "Open another tab in this pane?", "このペインにタブをもう一つ開きますか？")
                .to_string()],
            tr(lang, " y / Enter = yes   n / Esc = no ", " y / Enter = はい   n / Esc = いいえ ")
                .to_string(),
        ),
        Popup::AiShellConfirm { command, .. } => {
            (
                tr(lang, " Command from description ", " 説明からコマンド生成 ").to_string(),
                vec![
                    tr(lang, "Insert this command at the shell prompt?", "このコマンドをシェルのプロンプトに入力しますか？").to_string(),
                    String::new(),
                    format!("  {}", command),
                ],
                tr(
                    lang,
                    " y/Enter = insert   r = not quite, try again   n/Esc = cancel ",
                    " y/Enter = 入力   r = 少し違う、やり直す   n/Esc = 取消 ",
                )
                .to_string(),
            )
        }
        Popup::ConfirmDiscard { targets, .. } => {
            let head = if lang == Lang::Ja {
                format!("{} 件の変更をコミット時点に戻します:", targets.len())
            } else {
                format!("discard changes to {} path(s):", targets.len())
            };
            let mut lines = vec![
                head,
                String::new(),
            ];
            for p in targets.iter().take(8) {
                lines.push(format!("  {}", p.display()));
            }
            if targets.len() > 8 {
                lines.push(tr_count(lang, targets.len() - 8));
            }
            lines.push(String::new());
            lines.push(tr(lang,
                "This throws away uncommitted changes and cannot be undone",
                "コミットしていない変更は失われ、元に戻せません").to_string());
            (
                tr(lang, " discard changes ", " 変更を破棄 ").to_string(),
                lines,
                tr(lang, " y/Enter = discard   n/Esc = cancel ", " y/Enter = 破棄   n/Esc = 取消 ").to_string(),
            )
        }
        Popup::ConfirmShortcutDelete { name, .. } => {
            let lines = vec![
                tr(lang, "remove this bookmark?", "このお気に入りを削除しますか？").to_string(),
                String::new(),
                format!("  {name}"),
                String::new(),
                tr(
                    lang,
                    "The place itself is untouched. Only the bookmark goes",
                    "場所そのものは消えません。お気に入りの登録だけを消します",
                )
                .to_string(),
            ];
            (
                tr(lang, " remove bookmark ", " お気に入りの削除 ").to_string(),
                lines,
                tr(
                    lang,
                    " y/Enter = remove   n/Esc = keep ",
                    " y/Enter = 削除   n/Esc = やめる ",
                )
                .to_string(),
            )
        }
        Popup::ConfirmDiffCopy { src, dst, is_dir, .. } => {
            let what = if *is_dir {
                tr(lang, "directory", "ディレクトリ")
            } else {
                tr(lang, "file", "ファイル")
            };
            let head = if lang == Lang::Ja {
                format!("既存の{}を上書きします:", what)
            } else {
                format!("overwrite the existing {}:", what)
            };
            let lines = vec![
                head,
                String::new(),
                format!("  {} {}", tr(lang, "from", "元"), src.display()),
                format!("  {}   {}", tr(lang, "to", "先"), dst.display()),
                String::new(),
                tr(lang, "The destination will be replaced", "コピー先は置き換えられます").to_string(),
            ];
            (
                tr(lang, " copy across ", " 反対側へコピー ").to_string(),
                lines,
                tr(lang, " y/Enter = overwrite   n/Esc = cancel ", " y/Enter = 上書き   n/Esc = 取消 ").to_string(),
            )
        }
        Popup::ConfirmDirSync { to_right, ops, extra, .. } => {
            let arrow = if *to_right { "left → right" } else { "right → left" };
            let arrow_ja = if *to_right { "左 → 右" } else { "右 → 左" };
            let n = ops.len();
            let head = if lang == Lang::Ja {
                format!("ディレクトリを一方向に同期（{}）", arrow_ja)
            } else {
                format!("one-way folder sync ({})", arrow)
            };
            let mut lines = vec![
                head,
                String::new(),
                format!("  {} {}", tr(lang, "copy / overwrite:", "コピー／上書き:"), n),
            ];
            if *extra > 0 {
                lines.push(format!(
                    "  {} {}",
                    tr(lang, "destination-only, kept:", "コピー先のみ・保持:"),
                    extra
                ));
            }
            lines.push(String::new());
            lines.push(
                tr(
                    lang,
                    "Nothing is deleted; the source's files are copied over",
                    "削除は行いません。コピー元のファイルで置き換えます",
                )
                .to_string(),
            );
            (
                tr(lang, " synchronize ", " 同期 ").to_string(),
                lines,
                tr(lang, " y/Enter = sync   n/Esc = cancel ", " y/Enter = 同期   n/Esc = 取消 ").to_string(),
            )
        }
        Popup::ConfirmRemoteDelete { name, is_dir, .. } => {
            let head = if *is_dir {
                tr(lang, "delete this folder and everything inside it, on the server:",
                      "このディレクトリを中身ごとサーバ上で削除します:").to_string()
            } else {
                tr(lang, "delete this file on the server:", "このファイルをサーバ上で削除します:").to_string()
            };
            let lines = vec![
                head,
                String::new(),
                format!("  {}", name),
                String::new(),
                tr(lang, "this cannot be undone. the server has no trash", "取り消せません。サーバにゴミ箱はありません").to_string(),
            ];
            (
                tr(lang, " remote delete ", " リモート削除 ").to_string(),
                lines,
                tr(lang, " y/Enter = delete   n/Esc = cancel ", " y/Enter = 削除   n/Esc = 取消 ").to_string(),
            )
        }
        Popup::ConfirmRemoteMove { plan, from, to } => {
            let n = plan.files.len();
            let head = if lang == Lang::Ja {
                format!("{} 個をホスト間で移動します:", n)
            } else {
                format!("move {} item(s) across hosts:", n)
            };
            let lines = vec![
                head,
                String::new(),
                format!("  {}  →  {}", from, to),
                String::new(),
                tr(lang, "Each file is copied, then deleted from the source", "各ファイルをコピー後、コピー元から削除します").to_string(),
            ];
            (
                tr(lang, " move across hosts ", " ホスト間の移動 ").to_string(),
                lines,
                tr(lang, " y/Enter = move   n/Esc = cancel ", " y/Enter = 移動   n/Esc = 取消 ").to_string(),
            )
        }
        Popup::ConfirmSnippet { name, cmd, .. } => {
            let head = if lang == Lang::Ja {
                format!("スニペットを送信しますか？  「{}」", name)
            } else {
                format!("send this snippet?  \"{}\"", name)
            };
            let lines = vec![head, String::new(), format!("  $ {}", cmd)];
            (
                tr(lang, " send snippet ", " スニペット送信 ").to_string(),
                lines,
                tr(lang, " y/Enter = send   n/Esc = cancel ", " y/Enter = 送信   n/Esc = 取消 ").to_string(),
            )
        }
        Popup::ConfirmElevate { op, targets, dest } => {
            let verb = match (op, lang) {
                (PendingOp::Copy, Lang::Ja) => "コピー",
                (PendingOp::Move, Lang::Ja) => "移動",
                (PendingOp::Copy, Lang::En) => "copy",
                (PendingOp::Move, Lang::En) => "move",
            };
            let body = if lang == Lang::Ja {
                vec![
                    format!("{} への書き込みには管理者権限が必要です", dest.display()),
                    String::new(),
                    format!("{} 件の{}を昇格して再試行しますか？ UACの確認が出ます", targets.len(), verb),
                ]
            } else {
                vec![
                    format!("{} needs administrator rights to write to", dest.display()),
                    String::new(),
                    format!("Retry the {} of {} item(s) elevated? A UAC prompt will appear.", verb, targets.len()),
                ]
            };
            (
                tr(lang, " administrator rights ", " 管理者権限 ").to_string(),
                body,
                tr(lang, " y/Enter = retry as admin   n/Esc = cancel ",
                    " y/Enter = 管理者として再試行   n/Esc = 取消 ").to_string(),
            )
        }
        // All handled above, before this match.
        Popup::Manual { .. }
        | Popup::Report { .. }
        | Popup::ContextMenu { .. }
        | Popup::ColorPicker { .. }
        | Popup::SortPicker { .. }
        | Popup::Macros { .. }
        | Popup::GitLog { .. }
        | Popup::EncodingPicker { .. }
        | Popup::SshHosts { .. }
        | Popup::SshUsers { .. }
        | Popup::Snippets { .. }
        | Popup::ThemePicker { .. }
        | Popup::RemoteBrowser { .. }
        | Popup::LocalDest { .. }
        | Popup::Shortcuts { .. }
        | Popup::History { .. }
        | Popup::FindResults { .. }
        | Popup::GrepReplace(_)
        | Popup::DestPicker { .. }
        | Popup::Viewer { .. }
        | Popup::Diff { .. }
        | Popup::DirCompare { .. }
        | Popup::Archive { .. }
        | Popup::DiskUsage { .. }
        | Popup::Palette { .. }
        | Popup::AiChat { .. }
        | Popup::AiHistory { .. }
        | Popup::Toggles { .. }
        | Popup::ImageView { .. }
        | Popup::CommitMessage { .. }
        | Popup::JunkReview { .. }
        | Popup::StructureReview { .. }
        | Popup::RenameReview { .. }
        | Popup::DupeReview { .. }
        | Popup::OpQueue { .. }
        | Popup::None => return,
    };

    // A text-input box is wider (long descriptions and pasted paths need room)
    // and grows taller as the value wraps, so nothing you type is cut off.
    let width: u16 = match popup {
        Popup::TextInput { .. } => 96u16.min(area.width.saturating_sub(2)),
        // A notice can be a key list, whose lines are a key and a sentence; at
        // seventy columns every one of them wrapped, which turns a list into a
        // wall. It takes what the longest line asks for, within reason.
        Popup::Notice { lines } => {
            let longest = lines.iter().map(|l| width(l)).max().unwrap_or(0) as u16;
            longest.saturating_add(6).clamp(40, 110).min(area.width.saturating_sub(2))
        }
        _ => 70u16.min(area.width.saturating_sub(2)),
    };
    // How many rows the body actually needs once the paragraph below wraps it.
    //
    // Counted in display columns, not characters. A line of Japanese is twice
    // as wide as its character count says, so `chars().count() / cols` decided
    // a sixty-character sentence needed no second row when it needed two — and
    // the wrapped remainder fell off the bottom of the box. That was the whole
    // of "the text is cut off": the wrapping worked, the room for it did not.
    //
    // And for every popup, not only the text input. The command the AI proposes
    // is one long line that wraps exactly the same way, and nothing was making
    // room for *its* second row either.
    let inner_w = width.saturating_sub(4).max(1) as usize;
    let grown = |s: &str| wrap_input(s, inner_w).len().saturating_sub(1) as u16;
    let mut extra_rows: u16 = body.iter().map(|l| grown(l)).sum();
    if let Popup::TextInput { buffer, .. } = popup {
        // The field replaces body line 1 and carries a one-column prefix.
        extra_rows += grown(&format!(">{buffer}"));
    }
    let height = (body.len() as u16 + 4 + extra_rows).max(6).min(area.height.saturating_sub(2));
    let rect = centered_rect(width, height, area);

    clear_popup(f, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        // The AI - simple dialogs (the command confirm, the rename/search
        // prompts) wear the local model's cyan; the rest keep the theme accent.
        .border_style(Style::default().fg(popup_accent(popup)).add_modifier(Modifier::BOLD))
        .style(popup_style())
        .title(title);
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);

    // Clickable buttons for the dialogs. Each stands in for the key it mirrors,
    // so the keyboard shortcuts in the footer keep working unchanged.
    let buttons: Vec<(&str, ZoneKind)> = match popup {
        Popup::ConfirmDelete { .. } => vec![
            (tr(lang, "Trash", "ゴミ箱"), ZoneKind::Enter),
            (tr(lang, "Delete!", "完全削除"), ZoneKind::Char('a')),
            (tr(lang, "Cancel", "取消"), ZoneKind::Esc),
        ],
        Popup::ConfirmTransfer { targets, .. } => {
            let mut b = vec![
                (tr(lang, "Yes", "実行"), ZoneKind::Enter),
                (tr(lang, "Overwrite", "上書き"), ZoneKind::Char('a')),
            ];
            if targets.len() == 1 {
                b.push((tr(lang, "Rename", "改名"), ZoneKind::Char('r')));
            }
            b.push((tr(lang, "Cancel", "取消"), ZoneKind::Esc));
            b
        }
        Popup::Notice { .. } => vec![
            (tr(lang, "Copy", "コピー"), ZoneKind::Char('y')),
            (tr(lang, "Close", "閉じる"), ZoneKind::Enter),
        ],
        Popup::TextInput { .. } => vec![
            (tr(lang, "OK", "決定"), ZoneKind::Enter),
            (tr(lang, "Cancel", "取消"), ZoneKind::Esc),
        ],
        Popup::Search { .. } => vec![
            (tr(lang, "Jump", "ジャンプ"), ZoneKind::Enter),
            (tr(lang, "Cancel", "取消"), ZoneKind::Esc),
        ],
        Popup::ConfirmQuit | Popup::ConfirmClose { .. } | Popup::ConfirmNewTab { .. } => vec![
            (tr(lang, "Yes", "はい"), ZoneKind::Enter),
            (tr(lang, "No", "いいえ"), ZoneKind::Esc),
        ],
        Popup::ConfirmElevate { .. } => vec![
            (tr(lang, "Retry as admin", "管理者として再試行"), ZoneKind::Enter),
            (tr(lang, "Cancel", "取消"), ZoneKind::Esc),
        ],
        Popup::AiShellConfirm { .. } => vec![
            (tr(lang, "Insert", "入力"), ZoneKind::Enter),
            (tr(lang, "Try again", "やり直す"), ZoneKind::Char('r')),
            (tr(lang, "Cancel", "取消"), ZoneKind::Esc),
        ],
        Popup::ConfirmDiscard { .. } => vec![
            (tr(lang, "Discard", "破棄"), ZoneKind::Enter),
            (tr(lang, "Cancel", "取消"), ZoneKind::Esc),
        ],
        _ => vec![],
    };

    let mut body_text: Vec<Line> = body.into_iter().map(Line::from).collect();
    // The text-input field renders the cursor as a highlighted character so
    // moving it never shifts the surrounding text (was inserting a caret glyph).
    // Not a popup renderer of its own: it rewrites the line the shared body
    // above already laid out.
    if let Popup::TextInput { buffer, cursor, kind, select_all, .. } = popup {
        if body_text.len() >= 2 {
            let field = caret_lines(buffer, *cursor, kind.is_secret(), *select_all);
            body_text.splice(1..2, field);
        }
    }
    // A dialog gets a dedicated button row above the hint footer; everything
    // else keeps the single hint line.
    let button_row = !buttons.is_empty() && inner.height >= 3;
    let body_h = inner.height.saturating_sub(if button_row { 2 } else { 1 });
    let body_area = Rect::new(inner.x, inner.y, inner.width, body_h);
    let footer_area = footer_row(inner);

    // Spelled out rather than inherited: a `Block`'s style does not reach a
    // paragraph rendered into it, so without this the text kept the
    // terminal's own foreground — invisible on a light dialog.
    let p = Paragraph::new(body_text)
        .style(Style::default().fg(readable_on(theme().popup_bg)))
        .wrap(Wrap { trim: false });
    f.render_widget(p, body_area);

    if button_row {
        let btn_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(2), inner.width, 1);
        let mut x = btn_area.x;
        for (label, kind) in &buttons {
            let text = format!("[ {} ]", label);
            let w = text.chars().count() as u16;
            if x + w > btn_area.x + btn_area.width {
                break;
            }
            let r = Rect::new(x, btn_area.y, w, 1);
            f.render_widget(
                Paragraph::new(text).style(
                    accent_on_popup(),
                ),
                r,
            );
            zones.push(PopupZone { rect: r, kind: *kind });
            x += w + 2; // a gap so adjacent buttons are visually distinct
        }
    }

    let footer_p = Paragraph::new(footer).style(
        accent_bar(),
    );
    f.render_widget(footer_p, footer_area);
}

/// The theme gallery. The active theme is already applied live (the global
/// was swapped as the cursor moved), so the popup itself renders in the
/// previewed palette; a swatch row lets palettes be compared at a glance.
fn draw_theme_picker(f: &mut Frame, area: Rect, popup: &mut Popup, lang: Lang) {
    let Popup::ThemePicker { cursor, scope } = popup else { return };
    let names = crate::theme::THEME_NAMES;
    let pane_scope = matches!(scope, ThemeScope::Pane { .. });
    let w = 46u16.min(area.width);
    let h = (names.len() as u16 + 4).min(area.height.saturating_sub(2)).max(8);
    let rect = centered_rect(w, h, area);
    clear_popup(f, rect);
    f.render_widget(Block::default().style(popup_style()), rect);
    let title = match scope {
        ThemeScope::App { .. } => tr(lang, " theme — whole app ", " テーマ — 全体 "),
        ThemeScope::Pane { side, .. } if *side == 0 => tr(lang, " theme — left pane ", " テーマ — 左ペイン "),
        ThemeScope::Pane { .. } => tr(lang, " theme — right pane ", " テーマ — 右ペイン "),
    };
    let footer = if pane_scope {
        tr(lang, " j/k=preview  Enter=keep  x=follow app  Esc=cancel ",
                 " j/k=プレビュー  Enter=決定  x=全体に従う  Esc=取消 ")
    } else {
        tr(lang, " j/k=preview  Enter=keep  Esc=cancel ",
                 " j/k=プレビュー  Enter=決定  Esc=取消 ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(accent_on_popup())
        .title(title)
        .title_bottom(footer);
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);
    let view_h = inner.height as usize;
    let scroll = cursor.saturating_sub(view_h.saturating_sub(1)).min(*cursor);
    let mut lines: Vec<Line> = Vec::new();
    for (i, name) in names.iter().enumerate().skip(scroll).take(view_h) {
        let sel = i == *cursor;
        let pal = crate::theme::theme_preset(name).unwrap_or_default();
        // A compact swatch: directory / code / archive / executable accents.
        let sw = |c: Color| Span::styled("█", Style::default().fg(c));
        let name_style = if sel {
            accent_on_popup()
        } else {
            Style::default().fg(text_tone(theme().file.plain, theme().popup_bg))
        };
        lines.push(Line::from(vec![
            Span::styled(if sel { "▸ " } else { "  " }, name_style),
            Span::styled(format!("{:<20}", name), name_style),
            sw(pal.file.directory), sw(pal.file.code), sw(pal.file.archive),
            sw(pal.file.executable), sw(pal.accent),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// What is half-typed at the editor panel, if anything: the `:` line, the
/// `/` search, a block insert, or the count and operator of a vi command
/// still being spelled out.
///
/// Its own function because the row it goes on depends on where the panel
/// is: inside the frame when the panel fills the window, and on cian's own
/// prompt line when it is docked in a pane — the line `:` and `/` use in the
/// file panes, so everything typed at cian is typed in the same place.
pub(crate) fn editor_prompt(popup: &Popup, lang: Lang) -> Option<String> {
    let Popup::Viewer {
        editing, sub_walk, block_input, sub_input, find_input, count, pending, replace, ..
    } = popup
    else {
        return None;
    };
    // The replace bar comes first: while it is open it is what is being typed
    // into, and everything below describes something else.
    if let Some(r) = replace {
        return Some(replace_bar_line(r, lang));
    }
    editor_prompt_parts(
        *editing,
        sub_walk.as_deref(),
        block_input.as_deref(),
        sub_input.as_deref(),
        find_input.as_deref(),
        *count,
        *pending,
        lang,
    )
}

/// The replace bar as one line: the two fields with the caret in the one being
/// typed into, the three switches with the ones that are on filled in, and the
/// keys that act. It has to fit a narrow window, so the switches are two
/// characters each and the key list is the shortest form that still names
/// them.
fn replace_bar_line(r: &crate::ReplaceBar, lang: Lang) -> String {
    let caret = |s: &str, here: bool| if here { format!("{s}▏") } else { s.to_string() };
    let sw = |on: bool, label: &str| format!("{}{}", if on { "☑" } else { "☐" }, label);
    let (find, with) = (tr(lang, "find", "置換前"), tr(lang, "with", "置換後"));
    use cian_core::substitute::Pattern;
    // Named rather than ticked: it is one question with three answers, and a
    // box that is sometimes "wildcard" cannot be a box.
    let how = match r.pattern {
        Pattern::Plain => tr(lang, "as typed", "文字通り"),
        Pattern::Wildcard => tr(lang, "* ? wildcard", "ワイルドカード(*?)"),
        Pattern::Regex => tr(lang, "regex", "正規表現"),
    };
    format!(
        "{find} {}   {with} {}   {}{} {} {}   {}",
        caret(&r.find, !r.in_with),
        caret(&r.with, r.in_with),
        how,
        tr(lang, "(M-r)", "(M-r)"),
        sw(r.case_sensitive, tr(lang, "Aa(M-c)", "大小区別(M-c)")),
        sw(r.word, tr(lang, "word(M-w)", "単語(M-w)")),
        tr(
            lang,
            "Enter=this one  S-Enter=all  M-n=next  Tab=field  Esc",
            "Enter=1件ずつ  S-Enter=すべて  M-n=次へ  Tab=欄移動  Esc=閉じる",
        ),
    )
}

#[allow(clippy::too_many_arguments)]
fn editor_prompt_parts(
    editing: bool,
    sub_walk: Option<&SubWalk>,
    block_input: Option<&BlockInput>,
    sub_input: Option<&str>,
    find_input: Option<&str>,
    count: Option<usize>,
    pending: Option<char>,
    lang: Lang,
) -> Option<String> {
    let editing = &editing;
    if *editing || sub_walk.is_some() {
        None
    } else if let Some(b) = block_input {
        let what = match b.kind {
            crate::BlockEdit::Insert => tr(lang, "insert ▏", "左端に挿入 ▏"),
            crate::BlockEdit::Append => tr(lang, "append ▕", "右端に追記 ▕"),
            crate::BlockEdit::Replace => tr(lang, "replace ▊", "矩形を置換 ▊"),
            crate::BlockEdit::LineStart => tr(lang, "line start ▏", "各行の先頭 ▏"),
            crate::BlockEdit::LineEnd => tr(lang, "line end ▕", "各行の末尾 ▕"),
        };
        // A line selection has no column to report; a rectangle does.
        let ragged = matches!(b.kind, crate::BlockEdit::LineStart | crate::BlockEdit::LineEnd);
        let rows = b.block.bottom - b.block.top + 1;
        Some(format!(
            "{} {}_   {}",
            what,
            b.text,
            match (ragged, lang == Lang::Ja) {
                (true, true) => format!("({rows} 行)"),
                (true, false) => format!("({rows} lines)"),
                (false, true) => format!("({rows} 行, {} 桁目)", b.block.left + 1),
                (false, false) => format!("({rows} lines, col {})", b.block.left + 1),
            }
        ))
    } else if let Some(cmd) = sub_input {
        // What the prompt takes, shown rather than assumed: the replace form
        // first, then the word commands, because a blank prompt with no menu
        // is a prompt you have to have read the manual to use.
        // The menu is for someone who has not started yet. Once there is
        // something typed, it is the typed text that has to be readable, and
        // a wall of vocabulary beside it is only in the way.
        Some(if cmd.is_empty() {
            format!(
                ":_   {}",
                tr(lang,
                   "s/old/new/[gci] · w wq q q! · preview block outline ws sort uniq han zen expand[ all] unexpand reindent lf crlf",
                   "s/old/new/[gci] · w wq q q! · preview block outline ws sort uniq han zen expand[ all] unexpand reindent lf crlf"),
            )
        } else if cmd.starts_with('s') {
            // Mid-replace, the flags are the part still to be decided — and
            // the whole reason `r` seeded the prompt was so they would be.
            format!(
                ":{}_   {}",
                cmd,
                tr(lang,
                   "flags: g all on a line · c confirm each · i ignore case",
                   "フラグ: g 行内すべて · c 1件ずつ確認 · i 大小無視"),
            )
        } else {
            format!(":{}_", cmd)
        })
    } else if let Some(q) = find_input {
        Some(format!("/{}_", q))
    } else if count.is_some() || pending.is_some() {
        // A half-typed command — the `4` of `48G`, the `d` of `d3d`, the `z`
        // of `zz`. vi shows nothing here and leaves you to remember what you
        // have pressed; on a full-screen file that is a guess.
        let typed = format!(
            "{}{}",
            pending.map(String::from).unwrap_or_default(),
            count.map(|c| c.to_string()).unwrap_or_default(),
        );
        Some(format!(
            "{typed}_   {}",
            match pending {
                Some('z') => tr(lang, "z: a fold · zz zt zb the cursor line", "z: 折りたたみ · zz zt zb カーソル行の位置"),
                Some('d') => tr(lang, "d again deletes the line", "もう一度 d で行削除"),
                Some(_) => tr(lang, "Esc cancels", "Esc で取消"),
                None => tr(
                    lang,
                    "G line · j k l h · w b · } { · Esc cancels",
                    "G 行番号へ · j k l h · w b · } { · Esc で取消",
                ),
            }
        ))
    } else {
        None
    }
}

/// Where the cursor is in the editor panel, as a human counts it: the line,
/// and the column *the screen* is showing — two full-width characters are
/// four columns, and the ruler marks them that way. Counting characters
/// instead would disagree with the ruler on every Japanese line.
pub(crate) fn editor_position(popup: &Popup) -> Option<(usize, usize)> {
    let Popup::Viewer { view, line, col, .. } = popup else { return None };
    let shown = view
        .lines
        .get(*line)
        .map(|l| {
            l.chars()
                .take(*col)
                .fold(0usize, |at, c| at + cian_core::textops::char_cols(c, at))
        })
        .unwrap_or(0);
    Some((line + 1, shown + 1))
}

/// What the editor panel is doing, as a word and a colour.
///
/// Read once and used twice: on the panel's own frame when it fills the
/// window, and on cian's status bar when it is docked in a pane — where the
/// mode belongs with every other "what is going on" the window reports.
pub(crate) fn editor_mode(m: EditorMode) -> (&'static str, Color) {
    match m {
        EditorMode::Command => ("COMMAND", Color::Rgb(200, 100, 200)),
        EditorMode::Search => ("SEARCH", Color::Rgb(80, 200, 120)),
        // Not orange: the selecting modes are orange, and "the next key goes
        // into the file" is the one state worth never mistaking.
        EditorMode::Edit => ("EDIT", Color::Rgb(235, 105, 105)),
        // Notepad's typing state wears its own word. It is the same state as
        // EDIT underneath, but "EDIT" invites the question "as opposed to
        // what?" — and in this grammar there is no opposite to be in. Saying
        // NOTEPAD answers the question the badge would otherwise raise.
        EditorMode::Notepad => ("NOTEPAD", Color::Rgb(235, 105, 105)),
        EditorMode::Read => ("READ", theme().accent),
        EditorMode::Visual => ("VISUAL", Color::Rgb(255, 140, 0)),
        EditorMode::VisualLine => ("V-LINE", Color::Rgb(255, 140, 0)),
        EditorMode::VisualBlock => ("V-BLOCK", Color::Rgb(255, 175, 60)),
    }
}

/// The modes the editor panel can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorMode {
    Read,
    Edit,
    Notepad,
    Command,
    Search,
    Visual,
    VisualLine,
    VisualBlock,
}

fn popup_mode_of(
    sub: bool,
    find: bool,
    editing: bool,
    visual: Option<ViewVisual>,
    notepad: bool,
) -> EditorMode {
    if sub {
        EditorMode::Command
    } else if find {
        EditorMode::Search
    } else if editing {
        // In vim, typing is the news: a selection cannot be live at the same
        // time. In notepad the two coexist — typing is always possible, so it
        // is not news, and the selection is the more interesting of the facts.
        match (notepad, visual) {
            (true, Some(ViewVisual::Char)) => EditorMode::Visual,
            (true, Some(ViewVisual::Line)) => EditorMode::VisualLine,
            (true, Some(ViewVisual::Block)) => EditorMode::VisualBlock,
            (true, None) => EditorMode::Notepad,
            (false, _) => EditorMode::Edit,
        }
    } else {
        match visual {
            None => EditorMode::Read,
            Some(ViewVisual::Char) => EditorMode::Visual,
            Some(ViewVisual::Line) => EditorMode::VisualLine,
            Some(ViewVisual::Block) => EditorMode::VisualBlock,
        }
    }
}

/// The mode of the file the keyboard is pointed at, if that is a file.
pub(crate) fn editor_mode_of(popup: &Popup, notepad: bool) -> Option<EditorMode> {
    match popup {
        Popup::Viewer { sub_input, find_input, editing, visual, .. } => Some(popup_mode_of(
            sub_input.is_some(),
            find_input.is_some(),
            *editing,
            *visual,
            notepad,
        )),
        _ => None,
    }
}

/// The manual is taller than any terminal, so it renders as a scrolling
/// viewport rather than the fixed block the other popups use.
fn draw_manual(f: &mut Frame, area: Rect, popup: &mut Popup, lang: Lang) {
    let Popup::Manual { lines, scroll } = popup else { return };
    // 70 cut a third of the entries in half: the widest is 122 cells, and the
    // Japanese descriptions are two cells a character. Wide enough for almost
    // all of them, and the rest wrap.
    draw_scrolling_text(f, area, lines, scroll, tr(lang, " manual ", " キー一覧 "), 104, lang);
}

/// A read-only report (`:ragdebug`) — the manual's viewport with its own title.
fn draw_report(f: &mut Frame, area: Rect, popup: &mut Popup, lang: Lang) {
    let Popup::Report { title, lines, scroll, .. } = popup else { return };
    let title = title.clone();
    // Wider than the manual: a report is a table of keys or of scores, and a
    // truncated row is a row that has to be guessed at.
    draw_scrolling_text(f, area, lines, scroll, &title, 92, lang);
}

/// The scrolling viewport both of the above share: a bordered block whose body
/// is `lines` from `scroll`, with a percentage in the frame and the scroll keys
/// along the bottom. `scroll` is clamped here, which also normalises an
/// over-scrolled offset from the key handler.
fn draw_scrolling_text(
    f: &mut Frame,
    area: Rect,
    lines: &[String],
    scroll: &mut usize,
    title: &str,
    max_w: u16,
    lang: Lang,
) {
    let height = area.height.saturating_sub(2).max(6);
    let width: u16 = max_w.min(area.width.saturating_sub(2));
    let rect = centered_rect(width, height, area);
    let inner = rect.inner(Margin { vertical: 1, horizontal: 2 });
    let view_h = inner.height.saturating_sub(1) as usize;

    // Wrapped, not cut. This viewport used to truncate every line that reached
    // the edge, which in a narrow terminal is most of the manual — and a key
    // list whose descriptions end in `…` is a key list you have to guess at.
    // The continuations line up under the description so an entry that took
    // two rows still reads as one entry.
    let lines: Vec<String> =
        lines.iter().flat_map(|l| crate::util::wrap_hanging(l, inner.width as usize)).collect();

    // Clamp so the last page sits flush with the bottom; this also
    // normalises an over-scrolled offset from the key handler.
    let max_scroll = lines.len().saturating_sub(view_h);
    *scroll = (*scroll).min(max_scroll);
    let offset = *scroll;

    clear_popup(f, rect);
    let pos = match (offset * 100).checked_div(max_scroll) {
        Some(pct) => format!(" {}% ", pct),
        // Everything fits; there is nothing to scroll.
        None => " all ".to_string(),
    };
    // The dialog's own surface. Without this the manual and the reports were
    // the one thing on screen the theme did not reach: `Clear` leaves the
    // cells at the terminal's own colours, which is a dark box on a light
    // theme and a stranger's colours on any of them.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(accent_on_popup())
        .style(popup_style())
        .title(title.to_string())
        .title_bottom(pos);
    f.render_widget(block, rect);

    // Every row already fits, so nothing here can cut a 全角 character in half
    // — which used to leave the second cell as one nobody wrote, drawn as a
    // stray reversed block at the end of exactly the lines long enough to
    // reach the edge.
    let body: Vec<Line> = lines
        .iter()
        .skip(offset)
        .take(view_h)
        .map(|l| Line::from(l.clone()))
        .collect();
    let body_area = Rect::new(inner.x, inner.y, inner.width, view_h as u16);
    f.render_widget(
        Paragraph::new(body)
            .style(popup_style()),
        body_area,
    );

    let footer_area =
        footer_row(inner);
    let footer_text = match lang {
        Lang::En => " j/k scroll  u/d page  g/G  Esc close ",
        Lang::Ja => " j/k スクロール  u/d ページ  g/G  Esc 閉じる ",
    };
    let footer = Paragraph::new(footer_text).style(
        accent_bar(),
    );
    f.render_widget(footer, footer_area);
}

/// The context menu is anchored at the pointer rather than centred, so it
/// sizes and positions itself.
fn draw_context_menu(f: &mut Frame, area: Rect, popup: &mut Popup, menu_lang: Lang) {
    let Popup::ContextMenu { items, cursor, at } = popup else { return };
    // The context menu follows `menu_lang` (which may differ from the rest
    // of the UI) so it can be pinned to Japanese on an English interface.
    let lang = menu_lang;
    let (name_w, hint_w) = menu_dims(items, lang);
    let rect = context_menu_rect(items, *at, area, lang);

    clear_popup(f, rect);
    // Follow the theme's own surface (light on a light theme) with readable
    // text, rather than the always-dark popup background.
    let surf = surface();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(Style::default().fg(text_tone(theme().accent, theme().popup_bg)))
        .style(Style::default().bg(surf));
    let inner = rect.inner(Margin { vertical: 1, horizontal: 1 });
    f.render_widget(block, rect);

    let rows: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let sel = i == *cursor;
            let style = if sel {
                Style::default().bg(theme().selected_bg).fg(readable_on(theme().selected_bg)).add_modifier(Modifier::BOLD)
            } else {
                Style::default().bg(surf).fg(readable_on(surf))
            };
            // "▸ name … (hint)": name left-aligned, hint right-aligned in a
            // shared column, with even 2-cell gutters on both sides.
            let (name, hint) = menu_label_parts(item.label(lang));
            let marker = if sel { "▸ " } else { "  " };
            let body = if hint_w > 0 {
                format!("{}{}  {}  ", marker, pad_to(name, name_w), pad_left(hint, hint_w))
            } else {
                format!("{}{}  ", marker, pad_to(name, name_w))
            };
            Line::from(Span::styled(body, style))
        })
        .collect();
    f.render_widget(Paragraph::new(rows), inner);
}

/// The two lines every filterable list opens with: what has been typed, and
/// "(no match)" when it has ruled everything out.
/// Everything inside a popup's frame except the footer row.
fn body_rows(inner: Rect) -> Rect {
    Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1))
}

/// The last row inside a popup's frame — where its key hints go. Written out
/// twenty-two times, and off by one in the twenty-third is a footer drawn over
/// the last line of the body.
fn footer_row(inner: Rect) -> Rect {
    Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1)
}

fn filter_head(filter: &str, empty: bool) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(format!("/{filter}_"), accent_on_popup()))];
    if empty {
        lines.push(Line::from(Span::styled(
            "  (no match)",
            Style::default().fg(muted_on(theme().popup_bg)),
        )));
    }
    lines
}

/// One row of such a list: the accent when the cursor is on it, plain text
/// otherwise.
fn row_style(selected: bool) -> Style {
    if selected {
        accent_on_popup()
    } else {
        Style::default().fg(readable_on(theme().popup_bg))
    }
}

fn draw_ssh_hosts(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    hosts: &[cian_lua::SshHost],
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::SshHosts { cursor, filter } = popup else { return };
    let needle = filter.to_lowercase();
    let matches: Vec<&cian_lua::SshHost> = hosts
        .iter()
        .filter(|h| {
            needle.is_empty()
                || h.name.to_lowercase().contains(&needle)
                || h.host.to_lowercase().contains(&needle)
        })
        .collect();
    let w = 56u16.min(area.width);
    let h = (matches.len() as u16 + 5).min(area.height.saturating_sub(2)).max(6);
    let footer = tr(lang, " Enter=select  F2=type by hand  Esc ", " Enter=選択  F2=手入力  Esc ");
    let inner = popup_frame(f, area, w, h, tr(lang, " ssh — host ", " SSH — ホスト "), footer);

    let mut lines = filter_head(filter, matches.is_empty());
    for (i, hst) in matches.iter().enumerate() {
        let sel = i == *cursor;
        let style = row_style(sel);
        let users = if hst.users.len() == 1 {
            hst.users[0].name.clone()
        } else {
            format!("{} users", hst.users.len())
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{}{:<16}", if sel { "▸ " } else { "  " }, hst.name), style),
            Span::styled(
                format!("{:<22} {}", hst.host, users),
                Style::default().fg(muted_on(theme().popup_bg)),
            ),
        ]));
        // Row 0 is the filter line, so host `i` sits one below it.
        push_row_zone(zones, inner, inner.y + 1 + i as u16, i);
    }
    let body_area = body_rows(inner);
    f.render_widget(Paragraph::new(lines), body_area);
    let footer_area =
        footer_row(inner);
    f.render_widget(
        Paragraph::new(tr(lang, " type to filter  ↑↓ select  Enter next  Esc cancel ", " 入力で絞込  ↑↓ 選択  Enter 次へ  Esc 取消 ")).style(
            accent_bar(),
        ),
        footer_area,
    );
}

fn draw_snippets(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    snippets: &[cian_lua::Snippet],
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::Snippets { cursor, filter } = popup else { return };
    let needle = filter.to_lowercase();
    let matches: Vec<&cian_lua::Snippet> = snippets
        .iter()
        .filter(|s| {
            needle.is_empty()
                || s.name.to_lowercase().contains(&needle)
                || s.cmd.to_lowercase().contains(&needle)
        })
        .collect();
    let w = 64u16.min(area.width);
    let h = (matches.len() as u16 + 5).min(area.height.saturating_sub(2)).max(6);
    let inner = popup_frame(f, area, w, h, tr(lang, " snippets → shell ", " スニペット → シェル "), "");

    let mut lines = filter_head(filter, matches.is_empty());
    for (i, s) in matches.iter().enumerate() {
        let sel = i == *cursor;
        let style = row_style(sel);
        // A tag shows what will happen: run, type-only, or confirm-first.
        let tag = if s.confirm { "?" } else if s.enter { "↵" } else { "…" };
        lines.push(Line::from(vec![
            Span::styled(format!("{}{} ", if sel { "▸ " } else { "  " }, tag), style),
            Span::styled(format!("{:<20}", truncate(&s.name, 20)), style),
            Span::styled(
                format!("  {}", truncate(&s.cmd, (inner.width as usize).saturating_sub(26))),
                Style::default().fg(muted_on(theme().popup_bg)),
            ),
        ]));
        push_row_zone(zones, inner, inner.y + 1 + i as u16, i);
    }
    let body_area = body_rows(inner);
    f.render_widget(Paragraph::new(lines), body_area);
    let footer_area =
        footer_row(inner);
    f.render_widget(
        Paragraph::new(tr(lang, " type to filter  ↑↓ select  Enter send  Esc cancel ", " 入力で絞込  ↑↓ 選択  Enter 送信  Esc 取消 ")).style(
            accent_bar(),
        ),
        footer_area,
    );
}

fn draw_remote_browser(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::RemoteBrowser { label, cwd, entries, cursor, scroll, marked, loading, purpose } = popup else { return };
    let uploading = *purpose == BrowsePurpose::Upload;
    let th = theme();
    let w = 70u16.min(area.width);
    let h = area.height.saturating_sub(4).clamp(8, 30);
    let title = if uploading {
        format!(" upload → {}  :  {} ", label, cwd)
    } else {
        format!(" download ← {}  :  {} ", label, cwd)
    };
    let footer = if uploading {
        tr(
            lang,
            " Enter=open  -=up  PgUp/PgDn  u=upload here  Esc ",
            " Enter=開く  -=上  PgUp/PgDn  u=ここへアップロード  Esc ",
        )
    } else {
        tr(
            lang,
            " Enter=open/mark  Space=mark  -=up  PgUp/PgDn  d=download  Esc ",
            " Enter=開く/選択  Space=選択  -=上  PgUp/PgDn  d=ダウンロード  Esc ",
        )
    };
    let inner = popup_frame(f, area, w, h, title, footer);
    let view_h = inner.height as usize;
    if *loading {
        f.render_widget(
            Paragraph::new(tr(lang, "  …listing", "  …取得中"))
                .style(Style::default().fg(muted_on(theme().popup_bg)).add_modifier(Modifier::ITALIC)),
            inner,
        );
        return;
    }
    // The window follows the cursor, and the arithmetic has to say so: this
    // took the *smaller* of where it was and where the cursor needs it, which
    // for a cursor walking downwards is always where it already was. So the
    // listing never scrolled: everything past the first screenful of a server
    // directory could be walked onto and not seen. The panes have had the right
    // version of this all along — one line, and now one owner.
    *scroll = clamp_list_scroll(*scroll, *cursor, view_h, entries.len());
    let mut lines: Vec<Line> = Vec::new();
    if entries.is_empty() {
        lines.push(Line::from(Span::styled("  (empty)", Style::default().fg(muted_on(theme().popup_bg)))));
    }
    // How much listing there is, and where in it this screen sits. A server
    // directory is the one listing whose length is a surprise — you cannot see
    // the folder to guess — so it says so rather than leaving the reader to
    // find out by walking off the end.
    if entries.len() > view_h {
        let bar = Rect::new(inner.x + inner.width.saturating_sub(1), inner.y, 1, inner.height);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_symbol("█")
                .thumb_style(Style::default().fg(text_tone(th.accent, th.popup_bg)))
                .track_symbol(Some("│"))
                .track_style(Style::default().fg(th.border))
                .begin_symbol(None)
                .end_symbol(None),
            bar,
            &mut ScrollbarState::new(entries.len().saturating_sub(view_h)).position(*scroll),
        );
    }
    for (i, e) in entries.iter().enumerate().skip(*scroll).take(view_h) {
        let sel = i == *cursor;
        let checked = marked.contains(&e.name);
        let mark = if checked { "◉ " } else if sel { "▸ " } else { "  " };
        let (icon, name_c) = if e.is_dir {
            ("▸ ", th.file.directory)
        } else {
            ("  ", th.file.plain)
        };
        let size = if e.is_dir { String::new() } else { cian_core::disk::human_size(e.size) };
        // Base fg per row; the selected row also gets a full-width background
        // below so it reads as the focused row, like the file panes.
        let base = if sel {
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
        } else if checked {
            Style::default().fg(text_tone(Color::Rgb(130, 205, 150), theme().popup_bg))
        } else {
            Style::default().fg(name_c)
        };
        let mut line = Line::from(vec![
            Span::styled(format!("{}{}", mark, icon), base),
            Span::styled(format!("{:<40}", truncate(&e.name, 40)), base),
            Span::styled(format!("{:>10}", size), Style::default().fg(th.dim)),
        ]);
        if sel {
            line = line.style(Style::default().bg(th.selected_bg));
        }
        lines.push(line);
        push_row_zone(zones, inner, inner.y + (i - *scroll) as u16, i);
    }
    // Paint the selected row's background across the full inner width first,
    // then the text on top (the spans carry no bg, so it shows through).
    if !entries.is_empty() && *cursor >= *scroll {
        let sel_y = inner.y + (*cursor - *scroll) as u16;
        if sel_y < inner.y + inner.height {
            f.render_widget(
                Block::default().style(Style::default().bg(th.selected_bg)),
                Rect::new(inner.x, sel_y, inner.width, 1),
            );
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_local_dest(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::LocalDest { files, cursor } = popup else { return };
    let opts_len = 4usize;
    let w = 56u16.min(area.width);
    let h = (opts_len as u16 + 4).min(area.height);
    let inner = popup_frame(f, area, w, h, format!(" download {} file(s) to… ", files.len()), "");
    // Labels only; the actual dirs are resolved when a row is chosen.
    let labels = [
        tr(lang, "Left pane", "左ペイン"),
        tr(lang, "Right pane", "右ペイン"),
        tr(lang, "Desktop", "デスクトップ"),
        tr(lang, "Type a path…", "パスを入力…"),
    ];
    let rows: Vec<Line> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let sel = i == *cursor;
            let style = if sel {
                accent_on_popup()
            } else {
                Style::default().fg(readable_on(theme().popup_bg))
            };
            push_row_zone(zones, inner, inner.y + i as u16, i);
            Line::from(Span::styled(format!("{}{}", if sel { "▸ " } else { "  " }, l), style))
        })
        .collect();
    f.render_widget(Paragraph::new(rows), inner);
}

fn draw_ssh_users(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    hosts: &[cian_lua::SshHost],
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::SshUsers { host, cursor } = popup else { return };
    let Some(hst) = hosts.get(*host) else { return };
    let w = 40u16.min(area.width);
    let h = (hst.users.len() as u16 + 4).min(area.height.saturating_sub(2)).max(6);
    let inner = popup_frame(f, area, w, h, format!(" {} — {} ", tr(lang, "ssh", "SSH"), hst.name), "");

    let lines: Vec<Line> = hst
        .users
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let sel = i == *cursor;
            let style = if sel {
                accent_on_popup()
            } else {
                Style::default().fg(readable_on(theme().popup_bg))
            };
            // A key marks logins that will authenticate without typing.
            let mark = if u.has_secret() { "  ◆" } else { "" };
            Line::from(Span::styled(
                format!("{}{}@{}{}", if sel { "▸ " } else { "  " }, u.name, hst.host, mark),
                style,
            ))
        })
        .collect();
    for i in 0..hst.users.len() {
        push_row_zone(zones, inner, inner.y + i as u16, i);
    }
    let body_area = body_rows(inner);
    f.render_widget(Paragraph::new(lines), body_area);
    let footer_area =
        footer_row(inner);
    f.render_widget(
        Paragraph::new(tr(lang, " Enter connect   Esc back ", " Enter 接続   Esc 戻る ")).style(
            accent_bar(),
        ),
        footer_area,
    );
}

/// The grep-replace preview: every line that would change, before / after,
/// with a checkbox. Unchecked rows are dimmed rather than hidden — the point
/// of the list is to see what you decided *not* to do as well as what you did.
fn draw_grep_replace(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::GrepReplace(plan) = popup else { return };
    let w = 110u16.min(area.width.saturating_sub(2));
    let h = area.height.saturating_sub(4).max(8);
    let picked = plan.changes.iter().filter(|c| c.picked).count();
    let files = {
        let mut seen: Vec<&std::path::Path> = Vec::new();
        for c in plan.changes.iter().filter(|c| c.picked) {
            if !seen.contains(&c.path.as_path()) {
                seen.push(c.path.as_path());
            }
        }
        seen.len()
    };
    let title = format!(
        " replace  {}  —  {}/{} line(s) in {} file(s) ",
        plan.what,
        picked,
        plan.changes.len(),
        files
    );
    let inner = popup_frame(f, area, w, h, truncate_middle(&title, w.saturating_sub(4) as usize), "");

    // Bottom-up: the hint bar, the "before" text of the line under the cursor,
    // and — when there is one — a note about files that could not be read,
    // because a silently ignored file is the thing most likely to be mistaken
    // for "already correct".
    let note = (!plan.skipped.is_empty()) as u16;
    let body_h = inner.height.saturating_sub(2 + note) as usize;
    if plan.cursor < plan.scroll {
        plan.scroll = plan.cursor;
    } else if body_h > 0 && plan.cursor >= plan.scroll + body_h {
        plan.scroll = plan.cursor + 1 - body_h;
    }

    let dim = Color::Rgb(120, 120, 140);
    let mut last_file: Option<&std::path::Path> = None;
    if plan.scroll > 0 {
        last_file = plan.changes.get(plan.scroll - 1).map(|c| c.path.as_path());
    }
    for (row, (i, c)) in plan.changes.iter().enumerate().skip(plan.scroll).take(body_h).enumerate() {
        let sel = i == plan.cursor;
        let y = inner.y + row as u16;
        let line_area = Rect::new(inner.x, y, inner.width, 1);
        push_row_zone(zones, inner, y, i);
        if sel {
            f.render_widget(
                Block::default().style(Style::default().bg(theme().selected_bg)),
                line_area,
            );
        }
        let base = if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
        // The file name is printed once per run of lines from that file: with
        // twenty hits in one file, repeating the path twenty times crowds out
        // the text that is actually being decided on.
        let same_file = last_file == Some(c.path.as_path());
        last_file = Some(c.path.as_path());
        let loc = if same_file {
            format!("{:>8}: ", c.line + 1)
        } else {
            format!("{}:{}: ", c.path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), c.line + 1)
        };
        let mark = if c.picked { "[x] " } else { "[ ] " };
        let loc_w = width(&loc).min(inner.width as usize / 3);
        let rest = (inner.width as usize).saturating_sub(4 + loc_w);
        let text_style = if c.picked {
            base.fg(readable_on(row_bg(sel)))
        } else {
            base.fg(dim).add_modifier(Modifier::CROSSED_OUT)
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(mark, if c.picked { base.fg(text_tone(theme().accent, row_bg(sel))) } else { base.fg(dim) }),
                Span::styled(truncate_middle(&loc, loc_w), base.fg(dim_text(row_bg(sel)))),
                Span::styled(truncate(&crate::util::plain(&c.after.replace('\n', "⏎")), rest), text_style),
            ])),
            line_area,
        );
    }

    // The rows show what each line becomes. The one under the cursor is the
    // one being decided, so show what it is now too — a diff of one, exactly
    // where it is needed and nowhere it is not.
    if let Some(c) = plan.changes.get(plan.cursor) {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" now ", Style::default().fg(dim)),
                Span::styled(
                    truncate(&crate::util::plain(&c.before), inner.width.saturating_sub(5) as usize),
                    Style::default().fg(text_tone(theme().file.archive, theme().popup_bg)),
                ),
            ])),
            Rect::new(inner.x, inner.y + inner.height.saturating_sub(2 + note), inner.width, 1),
        );
    }

    if note == 1 {
        let why = plan
            .skipped
            .iter()
            .take(2)
            .map(|s| {
                format!("{} ({})", s.path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), s.why)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let more = if plan.skipped.len() > 2 { format!(" +{}", plan.skipped.len() - 2) } else { String::new() };
        f.render_widget(
            Paragraph::new(truncate(
                &format!(" {} not read: {why}{more}", plan.skipped.len()),
                inner.width as usize,
            ))
            .style(Style::default().fg(text_tone(theme().file.code, theme().popup_bg))),
            Rect::new(inner.x, inner.y + inner.height.saturating_sub(2), inner.width, 1),
        );
    }

    f.render_widget(
        Paragraph::new(tr(
            lang,
            " Space=toggle  a=all  f=this file  Enter=write  Esc=cancel ",
            " Space=切替  a=全部  f=このファイル  Enter=書き込み  Esc=取消 ",
        ))
        .style(accent_bar()),
        footer_row(inner),
    );
}

fn draw_find_results(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    find: Option<(&str, &str, Option<cian_core::search::Outcome>, cian_core::search::Mode)>,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let accent = popup_accent(popup);
    let Popup::FindResults { hits, cursor, scroll, by_ai } = popup else { return };
    let by_ai = *by_ai;
    let w = 96u16.min(area.width.saturating_sub(2));
    let h = area.height.saturating_sub(4).max(8);
    // The AI's semantic search lands in this same list; name it for the menu
    // item that produced it rather than for the `:find` state, which belongs to
    // whatever sweep ran last.
    let title = if by_ai {
        if lang == Lang::Ja {
            format!(" セマンティック検索 — {} 件 ", hits.len())
        } else {
            format!(" Semantic search — {} found ", hits.len())
        }
    } else {
        match find {
            Some((query, root, done, mode)) => {
                let verb = match mode {
                    cian_core::search::Mode::Name => "find",
                    cian_core::search::Mode::Content => "grep",
                };
                let state = match done {
                    None => "searching…".to_string(),
                    Some(cian_core::search::Outcome::Complete) => format!("{} found", hits.len()),
                    Some(cian_core::search::Outcome::Cancelled) => {
                        format!("{} found (stopped)", hits.len())
                    }
                    Some(cian_core::search::Outcome::Truncated) => {
                        format!("{} found (too many, stopped)", hits.len())
                    }
                };
                format!(" {} \"{}\" in {} — {} ", verb, query, root, state)
            }
            None => " find ".to_string(),
        }
    };
    let inner = popup_frame_in(
        f,
        area,
        w,
        h,
        truncate_middle(&title, w.saturating_sub(4) as usize),
        "",
        accent,
    );

    let body_h = inner.height.saturating_sub(1) as usize;
    // Keep the cursor on screen as results stream in beneath it.
    keep_in_view(*cursor, scroll, body_h);

    if hits.is_empty() {
        f.render_widget(
            Paragraph::new("(nothing yet)").style(Style::default().fg(muted_on(theme().popup_bg))),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
    for (row, (i, hit)) in hits.iter().enumerate().skip(*scroll).take(body_h).enumerate() {
        let sel = i == *cursor;
        let y = inner.y + row as u16;
        let line_area = Rect::new(inner.x, y, inner.width, 1);
        push_row_zone(zones, inner, y, i);
        if sel {
            f.render_widget(
                Block::default().style(Style::default().bg(theme().selected_bg)),
                line_area,
            );
        }
        let base = if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
        // The directory part is context; the name is the answer.
        let rel = hit.rel.display().to_string();
        let (dir, name) = match rel.rfind(std::path::MAIN_SEPARATOR) {
            Some(i) => (rel[..=i].to_string(), rel[i + 1..].to_string()),
            None => (String::new(), rel.clone()),
        };
        let avail = inner.width.saturating_sub(4) as usize;
        let mut spans = vec![Span::styled(if sel { " ▸ " } else { "   " }, base)];
        match &hit.line {
            // A content match: the location is a prefix, the matched text
            // is the answer, so give the text the room and the emphasis.
            Some((n, text)) => {
                let loc = format!("{}:{}  ", rel, n);
                let loc_w = width(&loc).min(avail / 2);
                spans.push(Span::styled(
                    truncate_middle(&loc, loc_w),
                    base.fg(dim_text(row_bg(sel))),
                ));
                spans.push(Span::styled(
                    truncate(&crate::util::plain(text), avail.saturating_sub(loc_w)),
                    base.fg(readable_on(row_bg(sel))),
                ));
            }
            None => {
                spans.push(Span::styled(
                    truncate_middle(&dir, avail.saturating_sub(width(&name))),
                    base.fg(dim_text(row_bg(sel))),
                ));
                spans.push(Span::styled(
                    name.clone(),
                    if hit.is_dir {
                        base.fg(text_tone(FileKind::Directory.color(), row_bg(sel))).add_modifier(Modifier::BOLD)
                    } else {
                        base.fg(readable_on(row_bg(sel)))
                    },
                ));
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)), line_area);
    }
    f.render_widget(
        Paragraph::new(tr(lang, " Enter=go  r=replace all  p=panelize  j/k=move  Esc=close ", " Enter=移動  r=一括置換  p=ペイン化  j/k=カーソル  Esc=閉じる ")).style(
            accent_bar(),
        ),
        footer_row(inner),
    );
}

fn draw_shortcuts(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::Shortcuts { entries, cursor, path } = popup else { return };
    let level = sc_level(entries, path);
    // Wide, because these are paths and URLs; the generic 70-column popup
    // wrapped them across lines, which made the list unreadable.
    let w = 96u16.min(area.width.saturating_sub(2));
    let h = (level.len() as u16 + 5).max(8).min(area.height.saturating_sub(2));
    // Breadcrumb of the current group path in the title.
    let mut crumb = String::new();
    let mut walk: &[Shortcut] = entries;
    for &i in path.iter() {
        if let Some(s) = walk.get(i) {
            crumb.push_str(&format!(" / {}", s.name));
            walk = s.children.as_deref().unwrap_or(&[]);
        }
    }
    let title = format!("{}{} ", tr(lang, " shortcuts", " ショートカット"), crumb);
    let inner = popup_frame(f, area, w, h, title, "");

    let body_h = inner.height.saturating_sub(1);
    let footer_area =
        footer_row(inner);

    if level.is_empty() {
        let hint = vec![
            Line::from(Span::styled(
                tr(lang, "(empty)", "（空）"),
                Style::default().fg(muted_on(theme().popup_bg)),
            )),
            Line::from(""),
            Line::from(tr(lang, "a = add a shortcut,  A = add a folder", "a = ショートカット追加,  A = ディレクトリ追加")),
        ];
        f.render_widget(
            Paragraph::new(hint),
            Rect::new(inner.x, inner.y, inner.width, body_h),
        );
    } else {
        // Name column sized to the longest name, within reason, so the
        // targets line up in a column of their own.
        let name_w = level
            .iter()
            .map(|s| width(&s.name))
            .max()
            .unwrap_or(8)
            .clamp(8, 24);
        let target_w = (inner.width as usize).saturating_sub(name_w + 8);

        // Keep the selected row visible once the list outgrows the popup.
        let view = body_h as usize;
        let first = cursor.saturating_sub(view.saturating_sub(1));
        for (row, (i, sc)) in level.iter().enumerate().skip(first).take(view).enumerate() {
            let sel = i == *cursor;
            let y = inner.y + row as u16;
            let line_area = Rect::new(inner.x, y, inner.width, 1);
            push_row_zone(zones, inner, y, i);
            if sel {
                // A full-width bar, not just a marker: which row is active
                // has to be obvious at a glance.
                f.render_widget(
                    Block::default().style(Style::default().bg(theme().selected_bg)),
                    line_area,
                );
            }
            let base = if sel {
                Style::default().bg(theme().selected_bg)
            } else {
                Style::default()
            };
            let name_style = if sel {
                base.fg(text_tone(theme().accent, row_bg(sel))).add_modifier(Modifier::BOLD)
            } else {
                base.fg(readable_on(row_bg(sel))).add_modifier(Modifier::BOLD)
            };
            // The target is reference material: same row, quieter, so the
            // name is what the eye lands on.
            let target_style = base.fg(muted_on(row_bg(sel)));
            // A folder shows a ▸ and its child count instead of a target.
            let (icon, tail) = if sc.is_group() {
                ("▸".to_string(), format!("{} items", sc.children.as_ref().map(|c| c.len()).unwrap_or(0)))
            } else {
                (shortcut_icon(sc.target_str()).to_string(), truncate_middle(sc.target_str(), target_w))
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(if sel { " ▸ " } else { "   " }, name_style),
                    Span::styled(format!("{}  ", icon), base),
                    Span::styled(
                        format!("{}  ", pad_to(&truncate_middle(&sc.name, name_w), name_w)),
                        name_style,
                    ),
                    Span::styled(tail, target_style),
                ])),
                line_area,
            );
        }
    }
    f.render_widget(
        Paragraph::new(tr(lang, " Enter=open/into  a=add  A=folder  d=del  r=edit  ←=back  Esc ", " Enter=開く/入る  a=追加  A=ディレクトリ  d=削除  r=編集  ←=戻る  Esc "))
            .style(
                Style::default()
                    .fg(readable_on(theme().accent))
                    .bg(theme().accent)
                    .add_modifier(Modifier::BOLD),
            ),
        footer_area,
    );
}

fn draw_history(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::History { entries, cursor } = popup else { return };
    // Its own renderer rather than the plain-text popup, so the selected
    // row gets the same highlight bar the shortcuts list has.
    let w = 96u16.min(area.width.saturating_sub(2));
    let h = (entries.len() as u16 + 5).max(6).min(area.height.saturating_sub(2));
    let inner = popup_frame(f, area, w, h, format!(" {} ({}) ", tr(lang, "history", "履歴"), entries.len()), "");

    let body_h = inner.height.saturating_sub(1) as usize;
    let first = cursor.saturating_sub(body_h.saturating_sub(1));
    for (row, (i, p)) in entries.iter().enumerate().skip(first).take(body_h).enumerate() {
        let sel = i == *cursor;
        let line_area = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        push_row_zone(zones, inner, inner.y + row as u16, i);
        if sel {
            f.render_widget(
                Block::default().style(Style::default().bg(theme().selected_bg)),
                line_area,
            );
        }
        let base =
            if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
        let text_style = if sel {
            base.fg(text_tone(theme().accent, row_bg(sel))).add_modifier(Modifier::BOLD)
        } else {
            base.fg(readable_on(row_bg(sel)))
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if sel { " ▸ " } else { "   " }, text_style),
                Span::styled(
                    truncate_middle(&p.display().to_string(), inner.width as usize - 4),
                    text_style,
                ),
            ])),
            line_area,
        );
    }
    f.render_widget(
        Paragraph::new(tr(lang, " ↑↓/jk select  Enter jump  a add shortcut  Esc cancel ", " ↑↓/jk 選択  Enter 移動  a ショートカット追加  Esc 取消 ")).style(
            accent_bar(),
        ),
        footer_row(inner),
    );
}

fn draw_dest_picker(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    dests: &[(String, PathBuf)],
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::DestPicker { op, targets, cursor } = popup else { return };
    let rows = dests.len();
    let w = 84u16.min(area.width.saturating_sub(2));
    let h = (rows as u16 + 6).min(area.height.saturating_sub(2));
    let verb = match (op, lang) {
        (PendingOp::Copy, Lang::En) => "copy",
        (PendingOp::Move, Lang::En) => "move",
        (PendingOp::Copy, Lang::Ja) => "コピー",
        (PendingOp::Move, Lang::Ja) => "移動",
    };
    let dp_title = if lang == Lang::Ja {
        format!(" {} 件を{} ", targets.len(), verb)
    } else {
        format!(" {} {} item(s) to ", verb, targets.len())
    };
    let inner = popup_frame(f, area, w, h, dp_title, "");

    for (i, (kind, path)) in dests.iter().enumerate().take(inner.height.saturating_sub(2) as usize) {
        let sel = i == *cursor;
        let y = inner.y + i as u16;
        let line = Rect::new(inner.x, y, inner.width, 1);
        push_row_zone(zones, inner, y, i);
        if sel {
            f.render_widget(
                Block::default().style(Style::default().bg(theme().selected_bg)),
                line,
            );
        }
        let base = if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if sel { " ▸ " } else { "   " }, base),
                Span::styled(
                    format!("{:<11}", kind),
                    base.fg(dim_text(row_bg(sel))),
                ),
                Span::styled(
                    truncate_middle(&path.display().to_string(), inner.width as usize - 16),
                    base.fg(readable_on(row_bg(sel))),
                ),
            ])),
            line,
        );
    }
    f.render_widget(
        Paragraph::new(tr(lang, " Enter=send here   n=type a path   Esc=cancel ", " Enter=ここへ   n=パス入力   Esc=取消 ")).style(
            accent_bar(),
        ),
        footer_row(inner),
    );
}

/// The outline column down the left of the viewer.
///
/// The highlighted entry is the one the cursor is *inside*, not the one it is
/// on: scrolling through a function body should keep saying which function
/// that is, which is the whole reason to give up the screen width.
fn draw_outline_column(
    f: &mut Frame,
    area: Rect,
    items: &[cian_core::outline::Item],
    line: usize,
) {
    use cian_core::outline::Kind;
    if area.width == 0 || area.height == 0 {
        return;
    }
    // Paint the column's own background first. A `Paragraph` writes only the
    // characters it has, so a row that is now short — or empty — would keep
    // whatever the last frame left in the cells beyond it.
    f.render_widget(Block::default().style(Style::default().bg(surface())), area);
    let here = items.iter().rposition(|i| i.line <= line);
    // Scroll the list so the current entry stays visible in a long file.
    let h = area.height as usize;
    let top = outline_top(items, line, h);
    for (row, (i, item)) in items.iter().enumerate().skip(top).take(h).enumerate() {
        let y = area.y + row as u16;
        let cur = here == Some(i);
        // Four colours that say what a thing is, each pulled onto the page
        // it is written on — they were picked against a dark ground, where a
        // pale blue heading reads and on cream is barely there.
        let colour = text_tone(
            match item.kind {
                Kind::Heading => Color::Rgb(150, 190, 250),
                Kind::Type => Color::Rgb(230, 200, 140),
                Kind::Function => Color::Rgb(170, 220, 175),
                Kind::Section => Color::Rgb(190, 175, 220),
            },
            surface(),
        );
        let indent = "  ".repeat(item.level.min(4));
        let text = format!("{indent}{}", item.text);
        let mut style = Style::default().fg(if cur { colour } else { dim_of(colour) });
        if cur {
            style = style.add_modifier(Modifier::BOLD);
        }
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if cur { "▎" } else { " " }, Style::default().fg(colour)),
                Span::styled(truncate(&text, area.width.saturating_sub(1) as usize), style),
            ])),
            Rect::new(area.x, y, area.width, 1),
        );
    }
}

/// Pull a colour back towards the background, for the entries that are not
/// the current one — the same hue, so the kind is still readable at a glance.
/// The quieter form of an outline colour: the same hue, mixed toward the page
/// it is written on, then pulled back if that took it too close to read.
///
/// It used to halve each channel toward a fixed mid-grey, which is a dark
/// theme's answer — on Nord it landed at 3.0:1 and on a light theme it went
/// the wrong way entirely.
fn dim_of(c: Color) -> Color {
    let bg = surface();
    let (Color::Rgb(r, g, b), Color::Rgb(br, bg_, bb)) = (as_rgb(c), as_rgb(bg)) else {
        return c;
    };
    let mix = |a: u8, b: u8| ((a as u16 * 11 + b as u16 * 9) / 20) as u8;
    text_tone(Color::Rgb(mix(r, br), mix(g, bg_), mix(b, bb)), bg)
}


/// A shade of the current surface, `amount` steps away from it.
///
/// Away, not darker: on a dark theme that means lighter and on a light theme
/// darker, so a tint meant to be *noticed* stays noticeable and a tint meant
/// to sit *under* the text never swallows it. Fixed dark values did the second
/// of those on a light theme, which is where this came from.
pub(crate) fn shade_of_surface(amount: i16) -> Color {
    let (r, g, b) = match surface() {
        Color::Rgb(r, g, b) => (r as i16, g as i16, b as i16),
        _ => (30, 30, 40),
    };
    // In i32: 299 × 255 alone overflows an i16, and a panic here would take
    // the program down every frame.
    let lum = (299 * r as i32 + 587 * g as i32 + 114 * b as i32) / 1000;
    let step = if lum > 140 { -amount } else { amount };
    let clamp = |v: i16| v.saturating_add(step).clamp(0, 255) as u8;
    Color::Rgb(clamp(r), clamp(g), clamp(b))
}

/// Tint the cursor's line.
///
/// A background rather than a colour change, so it sits under the syntax
/// highlighting instead of arguing with it — the tint says which line you are
/// on, and the colours go on saying what the text is. There is no matching
/// stripe down the column: the ruler already marks it, and a full-height bar
/// through the text costs more reading than it repays.
fn cross(base: Style, on_line: bool) -> Style {
    if on_line {
        base.bg(shade_of_surface(28))
    } else {
        base
    }
}

/// The two halves of a split viewer. The focused one comes first.
fn split_viewer_areas(area: Rect, left_right: bool) -> (Rect, Rect) {
    if left_right {
        let w = area.width / 2;
        (
            Rect::new(area.x, area.y, w, area.height),
            Rect::new(area.x + w, area.y, area.width - w, area.height),
        )
    } else {
        let h = area.height / 2;
        (
            Rect::new(area.x, area.y, area.width, h),
            Rect::new(area.x, area.y + h, area.width, area.height - h),
        )
    }
}

/// Where the viewer's ✕ was drawn, so a click can find it. Set by
/// `draw_viewer` each frame; zero-sized when the frame is too narrow for one.
#[allow(clippy::too_many_arguments)]
fn draw_viewer(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    lang: Lang,
    // The two things the viewer draws over the text but does not hold itself:
    // the invisible-character marks and the ruler with its crosshair.
    marks: (bool, bool),
    // Which of the viewer's open files this is, what they are all called, and
    // — when a comparison is running — what each line of *this* half is.
    tab: (usize, &[String], &[cian_core::diff::Mark]),
    // Docked in a pane rather than floating over the window, and whether the
    // keyboard is pointed at it.
    docked: bool,
    active: bool,
    vtracks: &mut Vec<crate::ScrollTrack>,
) -> (Vec<(Rect, usize)>, Rect) {
    let (show_ws, ruler) = marks;
    let (tab_at, tab_names, diff_marks) = tab;
    let tabs = tab_names.len();
    let mut tab_rects: Vec<(Rect, usize)> = Vec::new();
    let mut close_rect = Rect::new(0, 0, 0, 0);
    let Popup::Viewer { title, view, scroll, hscroll, line, col, visual, anchor, find_input, find_query, sub_input, sub_walk, block_input, git_lines, markdown, preview, source, md_styles, md_map, md_width, md_gen, md_seek, editing, dirty, editable, hl, hl_lang, blame, shape, path, count, pending, .. } = popup else { return (tab_rects, close_rect) };
    let rect = viewer_frame_rect_docked(area, docked);
    clear_popup(f, rect);

    // The preview owns `view.lines`: render the source to plain text plus a
    // parallel per-character style grid at the current width and swap it in;
    // leaving preview (or a width change) restores/re-wraps. Everything below
    // — cursor, visual selection, `/` search, the mouse — then works over
    // whichever text is on screen.
    let inner_w = rect.width.saturating_sub(4).max(1);
    let gen = crate::theme::theme_generation();
    if *preview {
        // Rebuilt when the width changes — and when the *theme* does. The
        // grid holds colours, and a preview opened on a light theme kept its
        // near-black text after `:theme` switched to a dark one: black on
        // black, with only the headings and the code blocks (which carry a
        // background of their own) still visible.
        if md_styles.is_empty() || *md_width != inner_w || *md_gen != gen {
            let (plain, styles, map) = crate::markdown::render_styled(source, inner_w as usize);
            view.lines = plain;
            *md_styles = styles;
            *md_map = map;
            *md_width = inner_w;
            *md_gen = gen;
        }
    } else if !md_styles.is_empty() {
        view.lines = source.clone();
        md_styles.clear();
        md_map.clear();
        *md_width = 0;
    }
    // The place asked for before the preview existed, now that the map does.
    if let Some(src) = md_seek.take() {
        if *preview {
            *line = disp_line(md_map, &view.lines, src);
            *scroll = line.saturating_sub(6);
        }
    }
    *line = (*line).min(view.lines.len().saturating_sub(1));
    *col = (*col).min(view.lines.get(*line).map(|l| l.chars().count()).unwrap_or(0));

    // Syntax highlight source code (not the Markdown preview, not while
    // editing). Computed once and cached; the cache is cleared on an edit
    // or re-decode so it refreshes. Colours come from the per-char category.
    if !*preview && !*editing {
        if let Some(lang) = hl_lang {
            // Same as the preview's grid: these are colours, so a theme
            // change makes them wrong rather than merely stale.
            if hl.is_empty() || *md_gen != gen {
                *hl = cian_core::highlight::highlight(&view.lines, *lang)
                    .into_iter()
                    .map(|cats| cats.into_iter().map(hl_style).collect())
                    .collect();
                *md_gen = gen;
            }
        }
    }

    // An edit changes the file's shape. Re-read it when the buffer has moved
    // on — but not while typing, where the outline would flicker on every
    // keystroke and the fold under the cursor could vanish mid-word.
    if !*editing && !*preview {
        let now = crate::fingerprint(&view.lines);
        if shape.as_deref().is_some_and(|s| s.fp != now) {
            *shape = crate::Shape::read(path, &view.lines, shape.as_deref());
        }
    }

    // Where the cursor is drawn, in columns — what the ruler measures and what
    // the crosshair has to agree with. Characters and columns part company the
    // moment a line has Japanese in it.
    let cur_col = view
        .lines
        .get(*line)
        .map(|l| {
            l.chars()
                .take(*col)
                .fold(0usize, |at, c| at + cian_core::textops::char_cols(c, at))
        })
        .unwrap_or(0);

    let kind = match view.kind {
        cian_core::viewer::ViewKind::Text => view.encoding.label(),
        cian_core::viewer::ViewKind::Binary => "binary",
    };
    let size = cian_core::human_size(view.total_bytes);
    let cut = if view.truncated { "  (first 4M shown)" } else { "" };
    // A little mode badge in the title, so which visual mode is active — and
    // where the cursor sits — is never a guess.
    // The viewer says what mode it is in the way the file panes do: a word and
    // a colour, on the border as well as in the chip. Reading, selecting and
    // editing are three quite different things to have a keyboard pointed at,
    // and a badge alone is easy to have not looked at.
    // Typing at a prompt is its own mode and takes the frame, exactly as it
    // does in the file panes and in the same colours — otherwise `:` and `i`
    // begin the same way on screen while meaning opposite things.
    let (mode, mode_color) = editor_mode(popup_mode_of(
        sub_input.is_some(),
        find_input.is_some(),
        *editing,
        *visual,
        notepad_keys(),
    ));
    let dirty_mark = if *dirty { " ●" } else { "" };
    // The BOM is invisible in the text, which is exactly why it gets a badge:
    // three unseen bytes at the top of a script are a classic breakage.
    let bom_mark = if view.bom {
        match view.encoding {
            cian_core::viewer::TextEncoding::Utf8 => " · UTF-8 BOM",
            _ => " · BOM",
        }
    } else {
        ""
    };
    // The line ending is as invisible as the BOM and just as easy to convert
    // by accident, so it gets the same treatment: shown, and only changed on
    // purpose (`:lf` / `:crlf`).
    // The line ending, with the arrow the marks would draw, so the badge and
    // the text agree about which is which.
    let eol_mark = if view.kind == cian_core::viewer::ViewKind::Text {
        let arrow = match view.eol {
            cian_core::viewer::Eol::Crlf => "↵",
            cian_core::viewer::Eol::Cr => "←",
            cian_core::viewer::Eol::Lf => "↓",
        };
        format!(" · {} {}", view.eol.label(), arrow)
    } else {
        String::new()
    };
    let head = if *preview {
        tr(lang, "Markdown preview", "Markdown プレビュー").to_string()
    } else {
        format!("{}, {}{}{}{}", kind, size, cut, bom_mark, eol_mark)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        // Unfocused, the frame goes quiet — the same border colour an
        // unfocused pane wears. A panel that keeps its mode colour while the
        // keys are going somewhere else is a panel that looks live and is
        // not.
        .border_style(if active {
            Style::default().fg(mode_color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme().border)
        })
        // The viewer takes the theme's own surface (light on a light theme),
        // so it truly follows the theme; its text uses readable_on below.
        .style(Style::default().bg(surface()))
        .title(if tabs > 1 {
            // With several files open, which one this is matters more than how
            // big it is: the count replaces the size, and the name keeps its
            // dirty mark.
            // The two arrows come first, at a fixed column, so the mouse can
            // find them without the file name's length coming into it — the
            // same shape the file panes' history arrows have.
            // A strip, as the shell panel has: every open file named, the one
            // being read picked out. The two arrows come first at a fixed
            // column so the mouse can find them whatever the names are.
            let mut spans = vec![Span::styled(
                " ◂ ▸ ".to_string(),
                accent_on_popup(),
            )];
            let mut at = rect.x + 1 + 5;
            for (i, name) in tab_names.iter().enumerate() {
                let label = format!(" {} {} ", i + 1, truncate(name, 18));
                let w = width(&label) as u16;
                tab_rects.push((Rect::new(at, rect.y, w, 1), i));
                at += w;
                spans.push(Span::styled(
                    label,
                    if i == tab_at {
                        // Black on the mode colour is only legible while the
                        // mode colour is dark; on a light theme the badge and
                        // its text came out the same shade. What reads on the
                        // chip is a question about the chip.
                        Style::default()
                            .fg(readable_on(mode_color))
                            .bg(mode_color)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        // The tabs not being read are quieter than the one that
                        // is, but they still have to be readable on this page —
                        // a dark-theme grey vanishes into a light one.
                        Style::default().fg(muted_on(surface()))
                    },
                ));
            }
            if *dirty {
                spans.push(Span::styled(
                    " ●".to_string(),
                    Style::default().fg(fit_to_surface(Color::Rgb(240, 200, 120))),
                ));
            }
            Line::from(spans)
        } else {
            Line::from(format!(" {}{}  —  {} ", title, dirty_mark, head))
        })
        .title_bottom(Line::from(if docked {
            Vec::new()
        } else {
            vec![
            Span::styled(
                format!(" {} ", mode),
                Style::default()
                    .fg(readable_on(mode_color))
                    .bg(mode_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                // The column, counted as the screen counts it — the same
                // number the ruler marks. A character count would disagree
                // with the ruler on any line with Japanese in it, and the
                // column is what a fixed-width record is about anyway.
                format!(" {}:{} ", *line + 1, cur_col + 1),
                // The mode's colour, but as text on the page rather than on a
                // chip — a theme accent alone is not always enough to read a
                // number in on that theme's own paper.
                Style::default().fg(text_tone(mode_color, surface())),
            ),
            ]
        }));
    let whole = rect.inner(Margin { vertical: 1, horizontal: 2 });
    f.render_widget(block, rect);

    // A close button where a window keeps one. `:q` is the keyboard's way out
    // now that Esc no longer closes; this is the mouse's, and it says out loud
    // that the file *can* be closed — which a bare border does not.
    let close_w = 3u16;
    if rect.width > close_w + 4 {
        let close = Rect::new(rect.x + rect.width - close_w - 1, rect.y, close_w, 1);
        let x_bg = if active { mode_color } else { theme().border };
        f.render_widget(
            Paragraph::new(" ✕ ").style(
                Style::default().fg(readable_on(x_bg)).bg(x_bg).add_modifier(Modifier::BOLD),
            ),
            close,
        );
        close_rect = close;
    }

    // The outline takes a column off the left, but only when there is room
    // for both it and a usable amount of text — on a narrow terminal the file
    // is what you came for.
    let outline_w = shape.as_deref().map_or(0, |s| outline_width(whole.width, s.shown, s.items.len()));
    let inner = Rect::new(whole.x + outline_w, whole.y, whole.width - outline_w, whole.height);

    // The ruler is only for reading a fixed-width record, which the rendered
    // Markdown is not, and it costs a row.
    let show_ruler = ruler && !*preview && view.kind == cian_core::viewer::ViewKind::Text;
    // Docked, the hints live on the window's own bottom bar — there is no
    // room for them in a half-width frame, and a truncated hint is worse
    // than none. The row goes back to the file.
    let hint_row = u16::from(!docked);
    // A prompt being typed takes a row of its own above the hints (see
    // `prompt` further down), so the text gives one up while it is open.
    let prompt_row = !docked
        && (sub_input.is_some()
            || find_input.is_some()
            || block_input.is_some()
            || count.is_some()
            || pending.is_some());
    let body_h = inner
        .height
        .saturating_sub(hint_row + u16::from(show_ruler) + u16::from(prompt_row))
        as usize;
    // Closed folds take their lines out of the picture entirely: `visible` is
    // the buffer as it is actually shown, and everything below — scrolling,
    // the cursor, the mouse — works over that rather than over raw line
    // numbers, so a fold cannot leave a gap or a cursor stranded off screen.
    // Not while editing: folding is a reading aid, and hiding lines from
    // someone who is typing into the file is a good way to lose an edit into
    // a region they cannot see.
    let folded = shape
        .as_deref()
        .filter(|_| !*preview && !*editing)
        .map(|s| s.hidden(view.lines.len()))
        .unwrap_or_default();
    // The cursor never sits inside a closed fold; it sits on the heading that
    // closed. Doing it here catches every way the cursor can move — a search
    // hit, a `G`, a grep jump — instead of one arm at a time.
    if !folded.is_empty() && folded.get(*line).copied().unwrap_or(false) {
        if let Some(h) = shape.as_deref().and_then(|s| s.enclosing_fold(*line, view.lines.len())) {
            *line = h;
            *col = 0;
        }
    }
    let visible: Vec<usize> = if folded.is_empty() {
        (0..view.lines.len()).collect()
    } else {
        (0..view.lines.len()).filter(|i| !folded[*i]).collect()
    };
    // `scroll` stays a real line number — it is the file's position, and the
    // percentage in the corner would otherwise lie — so it is converted to and
    // from an index into `visible` around the clamping.
    let vpos = |l: usize| visible.partition_point(|v| *v < l);
    let cur_v = vpos(*line);
    let mut top_v = vpos(*scroll);
    let max_top = visible.len().saturating_sub(body_h);
    top_v = top_v.min(max_top);
    if cur_v < top_v {
        top_v = cur_v;
    } else if cur_v >= top_v + body_h.max(1) {
        top_v = cur_v + 1 - body_h.max(1);
    }
    *scroll = visible.get(top_v).copied().unwrap_or(0);
    let max_scroll = max_top;

    // Line numbers and the git change bar belong to the source only; the
    // rendered preview is a document, not a file listing. The blame gutter,
    // when on, takes the left column instead of line numbers.
    let show_blame = !blame.is_empty() && !*preview && !*editing;
    let numbered = !*preview && !show_blame && view.kind == cian_core::viewer::ViewKind::Text;
    // One column for the fold markers, present on every numbered line whether
    // or not that line folds: a gutter that changes width per line would
    // stagger the text and break the mouse's column mapping.
    let fold_col = usize::from(numbered && shape.as_deref().is_some_and(|s| !s.items.is_empty()));
    let gutter = if show_blame {
        BLAME_W
    } else if numbered {
        format!("{}", view.lines.len()).len().max(3) + 1 + fold_col
    } else {
        0
    };
    let avail = (inner.width as usize).saturating_sub(gutter);
    let hscroll = *hscroll;

    // Ordered selection endpoints, for the highlight geometry.
    let (s0, e0) = order_pos(*anchor, (*line, *col));
    let sel_bg = Style::default().bg(theme().selected_bg);
    // The page's own two colours, swapped. Not `REVERSED`, which swaps
    // whatever is underneath — on a tinted line that is the tint, and the
    // cursor came out as a smudge the same shade as the line it was on.
    let cursor_style = Style::default().fg(surface()).bg(readable_on(surface()));
    let search_bg = Style::default().bg(Color::Rgb(120, 100, 0)).fg(Color::Rgb(255, 240, 190));
    // Body text adapts to the (themed) surface so it reads on light themes.
    let text_fg = readable_on(surface());
    // Character columns matched by the active search, per line, for highlight.
    // Compiled once per frame; the same `/re/`-or-literal language as n/N uses
    // (util::viewer_find), so what glows is exactly what n lands on.
    let matcher = find_query
        .as_ref()
        .filter(|q| !q.is_empty())
        .and_then(|q| cian_core::search::Matcher::parse(q).ok());
    let match_cols = |l: &str| -> Vec<(usize, usize)> {
        let Some(m) = matcher.as_ref() else { return Vec::new() };
        // find_ranges is end-exclusive; the highlight loop below wants
        // inclusive ends.
        m.find_ranges(l).into_iter().map(|(s, e)| (s, e.saturating_sub(1).max(s))).collect()
    };

    // The inclusive selected column range on absolute line `i`, if any.
    let sel_cols = |i: usize, len: usize| -> Option<(usize, usize)> {
        match visual {
            None => None,
            Some(ViewVisual::Line) => {
                if i >= s0.0 && i <= e0.0 { Some((0, len)) } else { None }
            }
            // The block is a rectangle in *columns*, so which characters of
            // this line it covers depends on how wide this line's characters
            // are. Asking the block itself keeps the highlight and the edit
            // agreeing about where the rectangle is.
            Some(ViewVisual::Block) => {
                if i >= s0.0 && i <= e0.0 {
                    let b = cian_core::textops::Block::between(&view.lines, *anchor, (*line, *col));
                    let (from, to) = b.char_range(view.lines.get(i).map(|s| s.as_str()).unwrap_or(""));
                    (to > from).then(|| (from, to - 1))
                } else {
                    None
                }
            }
            Some(ViewVisual::Char) => {
                // Where the two grammars part company. vi's caret sits *on* a
                // character and its selection includes it; a notepad caret
                // sits *between* two and the selection stops short of the one
                // it is in front of. Drawn the way it will be deleted, or the
                // highlight promises a character the next keystroke does not
                // take. See `delete_viewer_selection`.
                let end_col = if notepad_keys() { e0.1.checked_sub(1) } else { Some(e0.1) };
                if i < s0.0 || i > e0.0 {
                    None
                } else if s0.0 == e0.0 {
                    end_col.filter(|e| *e >= s0.1).map(|e| (s0.1, e))
                } else if i == s0.0 {
                    Some((s0.1, len))
                } else if i == e0.0 {
                    end_col.map(|e| (0, e))
                } else {
                    Some((0, len))
                }
            }
        }
    };

    let rows: Vec<Line> = visible
        .iter()
        .skip(top_v)
        .take(body_h)
        .map(|i| (*i, &view.lines[*i]))
        .map(|(i, l)| {
            // The buffer keeps real tabs; the screen cannot show one. Each
            // buffer character is drawn as whatever it looks like — a tab as
            // the spaces up to the next stop — while every column reckoning
            // below (the cursor, the selection, the highlighter, a search hit)
            // stays in buffer characters. Expanding the string first, as this
            // used to, meant the file was saved back with its tabs already
            // spent.
            let marks = show_ws && !*preview;
            let trail_from = if marks {
                l.chars().count() - l.chars().rev().take_while(|c| *c == ' ').count()
            } else {
                usize::MAX
            };
            // `w` is how many columns this character will take, worked out by
            // the same function the block selection and the mouse use.
            let shown = |j: usize, ch: char, w: usize| -> String {
                match ch {
                    '\t' if marks => format!("→{}", " ".repeat(w.saturating_sub(1))),
                    '\t' => " ".repeat(w),
                    // An ideographic space is the one that breaks YAML and
                    // shell scripts while looking exactly like nothing.
                    '\u{3000}' if marks => "□".to_string(),
                    // Only the *trailing* half-width spaces. Dotting every
                    // gap between words makes prose unreadable, and the ones
                    // that matter — the invisible difference between a line
                    // that ends cleanly and one that does not — are at the end.
                    ' ' if j >= trail_from => "·".to_string(),
                    other => other.to_string(),
                }
            };
            // Take buffer characters until their *drawn* width fills the row,
            // starting from the first one past the columns scrolled off to
            // the left. `first` is kept because everything below indexes by
            // buffer character — the cursor, the selection, the highlighter —
            // and those indices do not move when the view does.
            let mut chars: Vec<char> = Vec::new();
            let mut first = 0usize;
            let mut drawn = 0usize;
            for (j, ch) in l.chars().enumerate() {
                let w = cian_core::textops::char_cols(ch, drawn);
                // Off to the left: counted for the tab stops, not drawn. A
                // character straddling the edge is left out rather than
                // sliced, since half of a wide one is not a character.
                if drawn + w <= hscroll {
                    drawn += w;
                    first = j + 1;
                    continue;
                }
                if drawn + w > hscroll + avail {
                    break;
                }
                drawn += w;
                chars.push(ch);
            }
            let len = l.chars().count();
            let sel = sel_cols(i, len);
            // While hex-editing, `col` holds a nibble index (0..32); map it to
            // the dump's on-screen column: offset(8) + 2 spaces, 3 cells per
            // byte, one extra gap after byte 8.
            let cur = if i == *line {
                if *editing && view.kind == cian_core::viewer::ViewKind::Binary {
                    let nib = (*col).min(31);
                    let byte = nib / 2;
                    Some(10 + byte * 3 + usize::from(byte >= 8) + nib % 2)
                } else {
                    Some(*col)
                }
            } else {
                None
            };
            let matches = match_cols(l);
            // The line the cursor is on, and the column it is in, tinted so
            // both can be followed across a wide record without a finger on
            // the screen. Underneath everything that says more than "you are
            // here" — a selection, a search hit, the cursor itself.
            let cross_line = ruler && !*preview && i == *line;
            let cell_style = |j: usize| -> Style {
                // Priority: cursor over selection over a search match; the
                // resting style is the Markdown colour in preview, else plain.
                if cur == Some(j) {
                    // Exactly as built: the page's two colours swapped. Putting
                    // the body colour back on top of it made the character the
                    // same near-black as the block behind it — a solid square
                    // with the letter painted out inside.
                    cursor_style
                } else if sel.map(|(a, b)| j >= a && j <= b).unwrap_or(false) {
                    sel_bg.fg(text_fg)
                } else if matches.iter().any(|(a, b)| j >= *a && j <= *b) {
                    search_bg
                } else if *preview {
                    md_styles.get(i).and_then(|s| s.get(j)).copied().unwrap_or_default()
                } else if !*editing && !hl.is_empty() {
                    let base = hl
                        .get(i)
                        .and_then(|s| s.get(j))
                        .copied()
                        .unwrap_or(Style::default().fg(text_fg));
                    cross(base, cross_line)
                } else {
                    cross(Style::default().fg(text_fg), cross_line)
                }
            };
            // Build the body char-by-char, merging same-styled runs.
            let mut spans: Vec<Span> = Vec::new();
            if show_blame {
                // "hash author……" per line, dimmed; a run of the same commit
                // reads as one block.
                let (hash, who) = blame
                    .get(i)
                    .map(|b| (b.hash.as_str(), b.author.as_str()))
                    .unwrap_or(("", ""));
                let who: String = who.chars().take(11).collect();
                let same_as_prev = i > 0 && blame.get(i - 1).map(|p| p.hash.as_str()) == Some(hash);
                let (shown_hash, shown_who) = if same_as_prev {
                    (String::new(), String::new()) // repeat block: leave blank
                } else {
                    (hash.to_string(), who)
                };
                spans.push(Span::styled(
                    format!("{:<7} {:<11} ", shown_hash, shown_who),
                    Style::default().fg(dim_text(surface())),
                ));
            }
            if numbered {
                // The line number, then a 1-column separator that doubles as
                // the git change bar (green added / amber modified / red for
                // a deletion just above). Keeping the width fixed means the
                // mouse column mapping is unaffected.
                spans.push(Span::styled(
                    format!("{:>w$}", i + 1, w = gutter.saturating_sub(1 + fold_col)),
                    Style::default().fg(dim_text(surface())),
                ));
                // A heading with something under it says so, and says whether
                // it is open. The marker is also the click target.
                if fold_col == 1 {
                    let sh = shape.as_deref();
                    let foldable = sh.is_some_and(|s| s.extent_at(i, view.lines.len()).is_some());
                    let shut = foldable && sh.is_some_and(|s| s.folds.contains(&i));
                    spans.push(Span::styled(
                        if !foldable { " " } else if shut { "▸" } else { "▾" },
                        Style::default().fg(if shut { theme().accent } else { Color::Rgb(110, 110, 135) }),
                    ));
                }
                // The 1-column separator (previously a plain space) is the
                // change bar.
                // A live comparison takes this column while it is running: it
                // is the more urgent of the two answers, and they are both
                // "how does this line differ from something".
                let (bar, bar_c) = match diff_marks.get(i) {
                    Some(cian_core::diff::Mark::Changed) => ("▌", fit_to_surface(Color::Rgb(240, 210, 120))),
                    Some(cian_core::diff::Mark::Only) => ("▌", fit_to_surface(Color::Rgb(130, 205, 150))),
                    _ => match git_lines.get(&i) {
                        Some(cian_core::git::LineChange::Added) => ("▏", fit_to_surface(Color::Rgb(130, 205, 150))),
                        Some(cian_core::git::LineChange::Modified) => ("▏", fit_to_surface(Color::Rgb(240, 210, 120))),
                        Some(cian_core::git::LineChange::DeletedBefore) => ("▁", fit_to_surface(Color::Rgb(230, 120, 120))),
                        None => (" ", Color::Reset),
                    },
                };
                spans.push(Span::styled(bar.to_string(), Style::default().fg(bar_c)));
            }
            let mut run = String::new();
            let mut run_style = cell_style(first);
            // The *absolute* drawn column, so a tab lands on its own stop
            // whatever has scrolled past.
            let mut at = hscroll;
            for (k, ch) in chars.iter().enumerate() {
                let j = first + k;
                let st = cell_style(j);
                if st != run_style && !run.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut run), run_style));
                }
                run_style = st;
                let w = cian_core::textops::char_cols(*ch, at);
                let text = shown(j, *ch, w);
                at += w;
                run.push_str(&text);
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, run_style));
            }
            // The cursor can sit just past the last char (empty line, or end
            // of line): show it as a reversed space so it stays visible.
            if cur == Some(len) && at >= hscroll {
                spans.push(Span::styled(" ".to_string(), cursor_style));
            }
            // A tint that stops where the text stops is not a line highlight;
            // it is a highlight on some words. Carry it to the edge.
            if cross_line {
                let used = (at - hscroll) + usize::from(cur == Some(len));
                if used < avail {
                    spans.push(Span::styled(
                        " ".repeat(avail - used),
                        cross(Style::default(), true),
                    ));
                }
            }
            // With the marks on, the line ending is drawn too, and the two
            // kinds look different — which is the point. A file that is CRLF
            // except for three lines is not something a badge in the title can
            // tell you, and it is exactly the file that causes trouble.
            if marks && len == l.chars().count() {
                spans.push(Span::styled(
                    match view.eol {
                        // One glyph each, the way Sakura draws them: a bent
                        // arrow for a carriage return, a straight one for a
                        // line feed. Two glyphs for CRLF said the same thing
                        // twice and cost a column.
                        cian_core::viewer::Eol::Crlf => "↵",
                        cian_core::viewer::Eol::Cr => "←",
                        cian_core::viewer::Eol::Lf => "↓",
                    },
                    Style::default().fg(text_tone(theme().file.directory, surface())),
                ));
            }
            Line::from(spans)
        })
        .collect();
    let top = inner.y + u16::from(show_ruler);
    let body_area = Rect::new(inner.x, top, inner.width, body_h as u16);
    f.render_widget(Paragraph::new(rows), body_area);
    // How much of the file this is: down the right border, and — when a line
    // runs past the edge — along the bottom one. Both sit *on* the frame, so
    // they cost no text, and both are only drawn when there is something off
    // screen to report.
    let bar = Style::default().fg(if active { theme().accent } else { theme().border });
    let track = Style::default().fg(theme().border);
    if visible.len() > body_h {
        vtracks.push(crate::ScrollTrack {
            rect: Rect::new(area.x + area.width.saturating_sub(1), top, 1, body_h as u16),
            what: crate::ScrollWhat::ViewerRows,
            total: visible.len(),
            shown: body_h,
        });
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_symbol("┃")
                .thumb_style(bar)
                .track_symbol(Some("│"))
                .track_style(track)
                .begin_symbol(None)
                .end_symbol(None),
            Rect::new(area.x + area.width.saturating_sub(1), top, 1, body_h as u16),
            &mut ScrollbarState::new(visible.len().saturating_sub(body_h)).position(top_v),
        );
    }
    // The widest line in view, which is what the bar has to describe: the
    // whole file's widest would jump about as the view moves through it, and
    // measuring every line of a large file on every frame is not free.
    let widest = visible
        .iter()
        .skip(top_v)
        .take(body_h)
        .filter_map(|i| view.lines.get(*i))
        .map(|l| cian_core::textops::col_span(l, usize::MAX).1)
        .max()
        .unwrap_or(0);
    if widest > avail && avail > 0 {
        let w = avail.min(u16::MAX as usize) as u16;
        vtracks.push(crate::ScrollTrack {
            rect: Rect::new(inner.x + gutter as u16, area.y + area.height.saturating_sub(1), w, 1),
            what: crate::ScrollWhat::ViewerCols,
            total: widest,
            shown: avail,
        });
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
                .thumb_symbol("━")
                .thumb_style(bar)
                .track_symbol(Some("─"))
                .track_style(track)
                .begin_symbol(None)
                .end_symbol(None),
            Rect::new(
                inner.x + gutter as u16,
                area.y + area.height.saturating_sub(1),
                w,
                1,
            ),
            &mut ScrollbarState::new(widest.saturating_sub(avail)).position(hscroll),
        );
    }
    if show_ruler {
        // A scale over the text, starting where the text starts: every tenth
        // column numbered, every fifth marked. Counting characters by eye is
        // exactly what it is here to stop.
        let mut scale = String::with_capacity(avail);
        while scale.chars().count() < avail {
            let c = scale.chars().count() + 1; // 1-based, as the corner reads
            scale.push(match c {
                _ if c % 10 == 0 => char::from_digit((c / 10 % 10) as u32, 10).unwrap_or('|'),
                _ if c % 5 == 0 => '+',
                _ => '·',
            });
        }
        // Split by characters, not bytes: the scale is made of `·`, which is
        // two bytes wide, so a column number used as a byte offset lands
        // inside one and takes the program with it.
        let marks: Vec<char> = scale.chars().collect();
        // The scale counts *display* columns, so the mark has to be where the
        // cursor is drawn rather than how many characters precede it. On a
        // line of Japanese those are different numbers, and the ruler was
        // pointing at neither the right column nor a useful one.
        let cur = cur_col.min(marks.len().saturating_sub(1));
        let before: String = marks[..cur.min(marks.len())].iter().collect();
        let at: String = marks.get(cur).into_iter().collect();
        let after: String = marks[(cur + 1).min(marks.len())..].iter().collect();
        let dim = Style::default().fg(dim_text(surface()));
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ".repeat(gutter), dim),
                Span::styled(before, dim),
                // Where the cursor is, in the scale as well as in the text.
                Span::styled(at, Style::default().fg(readable_on(mode_color)).bg(mode_color)),
                Span::styled(after, dim),
            ])),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
    if outline_w > 0 {
        if let Some(sh) = shape.as_deref() {
            draw_outline_column(
                f,
                Rect::new(whole.x, whole.y, outline_w.saturating_sub(1), body_h as u16),
                &sh.items,
                src_line(md_map, *line),
            );
        }
    }
    let pos = match max_scroll {
        0 => "all".to_string(),
        m => format!("{}%", *scroll * 100 / m),
    };
    // While editing, the footer shows the editor keys; otherwise the usual
    // hints. What is being *typed* is not a hint and does not take their row —
    // see `prompt` below.
    let ed = if *editable { tr(lang, " i edit ", " i 編集 ") } else { " " };
    // The `:` / `/` / block prompt, when one is open. It gets a line of its
    // own directly above the hints, the way `:` and `/` do in the file panes:
    // painting over the hints made the two halves of cian behave differently
    // at the very moment the keyboard has been handed somewhere new.
    let prompt: Option<String> = editor_prompt_parts(
        *editing,
        sub_walk.as_deref(),
        block_input.as_deref(),
        sub_input.as_deref(),
        find_input.as_deref(),
        *count,
        *pending,
        lang,
    );
    let footer = if *editing {
        tr(lang,
            " EDIT — type to insert   Ctrl+S save   Esc leave   Shift+Q discard ",
            " 編集中 — 入力で挿入   Ctrl+S 保存   Esc 終了   Shift+Q 破棄 ").to_string()
    } else if let Some(w) = sub_walk {
        // The decision prompt names the change and the progress, so neither
        // has to be held in the head while answering. This one is answered
        // with single keys, not typed into, so it stays on the hint row.
        let h = &w.hits[w.idx.min(w.hits.len().saturating_sub(1))];
        let shorten = |s: &str| truncate(s, 24);
        format!(
            "{}  {} → {}   [{}/{}]",
            tr(lang, " replace?  y yes   n no   a all   q stop ", " 置換?  y はい   n いいえ   a 残り全部   q 中止 "),
            shorten(&h.from),
            shorten(&h.to),
            w.idx + 1,
            w.hits.len(),
        )
    } else {
        {
            {
                let mmd = source.iter().any(|l| {
                    let t = l.trim_start();
                    (t.starts_with("```") || t.starts_with("~~~"))
                        && t.trim_start_matches(['`', '~']).trim().eq_ignore_ascii_case("mermaid")
                });
                // `]]` and `[[` only mean something when the file has a shape,
                // and folding only in the source; offering either otherwise is
                // a hint that answers a question nobody can ask.
                let has_shape = shape.as_deref().is_some_and(|s| !s.items.is_empty());
                let shape_hint = match (has_shape, *preview) {
                    (false, _) => "",
                    (true, true) => tr(lang, " ]] [[ section ", " ]] [[ 見出し "),
                    (true, false) => tr(lang, " ]] [[ section  Space fold  zA all ", " ]] [[ 見出し  Space 折りたたみ  zA 全部 "),
                };
                // `r` only means something once there is something to replace.
                let after_find = if find_query.is_some() {
                    tr(lang, " r replace ", " r 置換 ")
                } else {
                    ""
                };
                let hints = if *preview {
                    format!("{}{}{}{}{}{}",
                        tr(lang, " / f search  n/N  v/V select  y copy ", " / f 検索  n/N  v/V 選択  y コピー "),
                        after_find,
                        ed,
                        shape_hint,
                        if mmd { tr(lang, " m diagram ", " m 図 ") } else { "" },
                        tr(lang, " :preview source  ", " :preview ソース  "))
                } else if *markdown {
                    format!("{}{}{}{}{}",
                        tr(lang, " / f search  n/N  v/V select  y copy ", " / f 検索  n/N  v/V 選択  y コピー "),
                        after_find,
                        ed,
                        shape_hint,
                        tr(lang, " p paste  :preview  ", " p 貼り付け  :preview  "))
                } else {
                    format!("{}{}{}{}{}",
                        tr(lang, " / f search  n/N  v/V select  y copy  p paste ", " / f 検索  n/N  v/V 選択  y コピー  p 貼り付け "),
                        after_find,
                        ed,
                        shape_hint,
                        tr(lang, " e enc  ", " e 文字コード  "))
                };
                format!("{}{} ", hints, pos)
            }
        }
    };
    // Messages the panel raises — "copied", "saved", "nothing to fold here" —
    // go to cian's own status line along the bottom of the window, where
    // every other message in the program appears. They used to take this
    // footer, and docked there is no footer to take: the line was drawn over
    // the *text*, without clearing it, so "copied" appeared with a couple of
    // the file's own characters trailing after it.
    let footer_style =
        Style::default().fg(readable_on(mode_color)).bg(mode_color).add_modifier(Modifier::BOLD);
    let last_row = inner.y + inner.height.saturating_sub(1);
    if !docked {
        f.render_widget(
            Paragraph::new(truncate(&footer, inner.width as usize)).style(footer_style),
            Rect::new(inner.x, last_row, inner.width, 1),
        );
    }
    // The prompt sits on the row above, in the file panes' prompt colours, so
    // the hints it used to cover stay readable while something is being typed.
    // Docked, it goes to the window's prompt line instead — the one `:` and
    // `/` already use in the panes — so everything typed at cian is typed in
    // the same place.
    if let Some(text) = prompt.filter(|_| !docked) {
        if inner.height >= 2 {
            f.render_widget(
                Paragraph::new(truncate(&text, inner.width as usize)).style(prompt_style()),
                Rect::new(inner.x, last_row - 1, inner.width, 1),
            );
        }
    }
    (tab_rects, close_rect)
}

fn draw_dir_compare(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::DirCompare { left, right, entries, cursor, scroll, truncated, .. } = popup else { return };
    use cian_core::dirdiff::Status;
    let counts = {
        let (mut a, mut d, mut m) = (0, 0, 0);
        for e in entries.iter() {
            match e.status {
                Status::OnlyRight => a += 1,
                Status::OnlyLeft => d += 1,
                Status::Differ => m += 1,
            }
        }
        let cut = if *truncated { "  (stopped at 5000)" } else { "" };
        format!("~{} +{} -{}{}", m, a, d, cut)
    };
    let title = format!(" {}  ↔  {}   —   {} ", left, right, counts);
    let (w, h) = (area.width.saturating_sub(2), area.height.saturating_sub(2));
    let inner = popup_frame(f, area, w, h, title, "");

    let body_h = (inner.height.saturating_sub(1) as usize).max(1);
    // Keep the cursor on screen.
    keep_in_view(*cursor, scroll, body_h);
    let first = *scroll;
    let add = Color::Rgb(130, 225, 150);
    let del = Color::Rgb(255, 140, 145);
    let chg = Color::Rgb(240, 210, 120);
    // Two columns with a marker between them, mirroring the file diff: a
    // path sits on the side(s) it exists, so which tree has (or differs on)
    // an entry is read straight down either column.
    let mid = 3usize;
    let col = (inner.width as usize).saturating_sub(mid) / 2;
    for (row, (i, e)) in entries.iter().enumerate().skip(first).take(body_h).enumerate() {
        let sel = i == *cursor;
        let y = inner.y + row as u16;
        let line = Rect::new(inner.x, y, inner.width, 1);
        push_row_zone(zones, inner, y, i);
        if sel {
            f.render_widget(Block::default().style(Style::default().bg(theme().selected_bg)), line);
        }
        let base = if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
        let mut name = e.rel.display().to_string().replace('\\', "/");
        if e.is_dir {
            name.push('/');
        }
        let shown = truncate_middle(&name, col);
        let blank = " ".repeat(col);
        let (mark, mcol, left_txt, right_txt) = match e.status {
            Status::OnlyLeft => ("◀", del, shown.clone(), blank.clone()),
            Status::OnlyRight => ("▶", add, blank.clone(), shown.clone()),
            Status::Differ => ("≠", chg, shown.clone(), shown.clone()),
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(pad_to(&left_txt, col), base.fg(mcol)),
                Span::styled(format!(" {} ", mark), base.fg(mcol).add_modifier(Modifier::BOLD)),
                Span::styled(pad_to(&right_txt, col), base.fg(mcol)),
            ])),
            line,
        );
    }
    f.render_widget(
        Paragraph::new(tr(lang,
            " ◀ left  ▶ right  ≠ differ   Enter=go  </> copy one  [/] sync all  w save  Esc ",
            " ◀ 左  ▶ 右  ≠ 相違   Enter=移動  </> 1件コピー  [/] 一括同期  w 保存  Esc ",
        ))
        .style(accent_bar()),
        footer_row(inner),
    );
}

fn draw_diff(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    lang: Lang,
) {
    let Popup::Diff { left, right, result, folded, fold, scroll, encoding, find, find_input, .. } = popup else { return };
    use cian_core::diff::Row;

    let title = format!(" {} ↔ {}  —  {} ", left, right, cian_core::diff::summary(result));
    let (w, h) = (area.width.saturating_sub(2), area.height.saturating_sub(2));
    let inner = popup_frame(f, area, w, h, title, "");

    let body_h = inner.height.saturating_sub(1) as usize;
    let rows: &[Row] = if *fold { folded } else { &result.rows };
    let max_scroll = rows.len().saturating_sub(body_h);
    *scroll = (*scroll).min(max_scroll);

    // Two equal columns with a marker between them, so the eye can run
    // straight down either file.
    let gutter = 5usize;
    let col = (inner.width as usize).saturating_sub(3 + gutter * 2) / 2;

    let dim = Style::default().fg(dim_text(theme().popup_bg));
    let num = Style::default().fg(dim_text(theme().popup_bg));
    let del = Style::default().fg(text_tone(theme().file.archive, theme().popup_bg));
    let add = Style::default().fg(text_tone(theme().file.executable, theme().popup_bg));
    let chg = Style::default().fg(text_tone(theme().file.code, theme().popup_bg));
    // The exact edited span within a changed line: a solid bar, the way
    // WinMerge marks the characters that actually differ.
    let chg_hot = Style::default()
        .fg(readable_on(Color::Rgb(240, 210, 120)))
        .bg(Color::Rgb(240, 210, 120))
        .add_modifier(Modifier::BOLD);

    let cell = |line: Option<&cian_core::diff::Line>, style: Style| -> Vec<Span<'static>> {
        match line {
            Some(l) => vec![
                Span::styled(format!("{:>w$} ", l.no, w = gutter - 1), num),
                Span::styled(pad_to(&truncate(&l.text, col), col), style),
            ],
            // An absent side is left blank rather than filled, so the gap
            // itself shows which file the line is missing from.
            None => vec![Span::raw(" ".repeat(gutter + col))],
        }
    };

    // A changed line, with its common prefix/suffix left calm and only the
    // edited middle painted as a bar. `prefix`/`suffix` are the shared char
    // counts from `common_affixes`; each side clamps `suffix` to its own
    // length so an insertion (empty middle on one side) stays in bounds.
    let emph_cell = |line: &cian_core::diff::Line, prefix: usize, suffix: usize| -> Vec<Span<'static>> {
        let chars: Vec<char> = line.text.chars().collect();
        let n = chars.len();
        let suffix = suffix.min(n.saturating_sub(prefix));
        let mid_end = n - suffix;
        // Match `cell`'s truncation: keep at most `col` chars, ellipsis when cut.
        let fits = n <= col;
        let budget = if fits { col } else { col.saturating_sub(1) };
        let mut spans = vec![Span::styled(format!("{:>w$} ", line.no, w = gutter - 1), num)];
        let mut buf = String::new();
        let mut buf_hot = false;
        let mut shown = String::new();
        for (i, &c) in chars.iter().take(budget).enumerate() {
            let is_hot = i >= prefix && i < mid_end;
            if !buf.is_empty() && is_hot != buf_hot {
                spans.push(Span::styled(std::mem::take(&mut buf), if buf_hot { chg_hot } else { chg }));
            }
            buf_hot = is_hot;
            buf.push(c);
            shown.push(c);
        }
        if !buf.is_empty() {
            spans.push(Span::styled(buf, if buf_hot { chg_hot } else { chg }));
        }
        if !fits {
            spans.push(Span::styled("…".to_string(), chg));
            shown.push('…');
        }
        let pad = col.saturating_sub(crate::util::width(&shown));
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans
    };

    // Rows whose text matches the active search get a highlight bar.
    let needle = find.as_ref().map(|s| s.to_lowercase());
    let row_matches = |r: &Row| -> bool {
        let Some(q) = &needle else { return false };
        let has = |o: Option<&cian_core::diff::Line>| o.map(|l| l.text.to_lowercase().contains(q)).unwrap_or(false);
        match r {
            Row::Same { left, right } | Row::Changed { left, right } => has(Some(left)) || has(Some(right)),
            Row::Removed { left } => has(Some(left)),
            Row::Added { right } => has(Some(right)),
            Row::Skipped { .. } => false,
        }
    };
    let search_bg = Style::default().bg(Color::Rgb(80, 70, 20));
    let body: Vec<Line> = rows
        .iter()
        .skip(*scroll)
        .take(body_h)
        .map(|r| {
            let line = match r {
                Row::Skipped { lines } => Line::from(Span::styled(
                    format!("{:^w$}", format!("⋯ {} identical lines", lines), w = inner.width as usize),
                    Style::default().fg(dim_text(theme().popup_bg)),
                )),
                Row::Same { left: l, right: rr } => {
                    let mut s = cell(Some(l), dim);
                    s.push(Span::styled(" │ ", num));
                    s.extend(cell(Some(rr), dim));
                    Line::from(s)
                }
                Row::Changed { left: l, right: rr } => {
                    let (p, sfx) = cian_core::diff::common_affixes(&l.text, &rr.text);
                    let mut s = emph_cell(l, p, sfx);
                    s.push(Span::styled(" ~ ", chg.add_modifier(Modifier::BOLD)));
                    s.extend(emph_cell(rr, p, sfx));
                    Line::from(s)
                }
                Row::Removed { left: l } => {
                    let mut s = cell(Some(l), del);
                    s.push(Span::styled(" - ", del.add_modifier(Modifier::BOLD)));
                    s.extend(cell(None, del));
                    Line::from(s)
                }
                Row::Added { right: rr } => {
                    let mut s = cell(None, add);
                    s.push(Span::styled(" + ", add.add_modifier(Modifier::BOLD)));
                    s.extend(cell(Some(rr), add));
                    Line::from(s)
                }
            };
            if row_matches(r) { line.style(search_bg) } else { line }
        })
        .collect();

    // A binary comparison has no rows; say why rather than showing a void.
    let body = if result.binary {
        vec![Line::from(Span::styled(
            if result.identical {
                "  These are binary files, and they are byte-for-byte the same."
            } else {
                "  These are binary files, and their contents differ."
            },
            dim,
        ))]
    } else if result.identical {
        vec![Line::from(Span::styled("  The two files are identical.", add))]
    } else {
        body
    };

    f.render_widget(
        Paragraph::new(body),
        Rect::new(inner.x, inner.y, inner.width, body_h as u16),
    );
    let pos = match max_scroll {
        0 => "all".to_string(),
        m => format!("{}%", *scroll * 100 / m),
    };
    let fold_word = if *fold { tr(lang, "show all", "全表示") } else { tr(lang, "fold", "畳む") };
    // A live `/` search prompt takes over the footer while typing.
    let footer = if let Some(q) = find_input {
        format!(" /{}_ ", q)
    } else {
        format!(
            "{}{}  {}  [{}] {} ",
            tr(lang, " n/N change  / find  f ", " n/N 変更  / 検索  f "),
            fold_word,
            tr(lang, "c copy  w save(.html/.md)  e enc  x explain  g/G  Esc",
                  "c コピー  w 保存(.html/.md)  e 文字コード  x 説明  g/G  Esc"),
            encoding.label(),
            pos
        )
    };
    f.render_widget(
        Paragraph::new(footer)
        .style(accent_bar()),
        footer_row(inner),
    );
}

fn draw_archive(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::Archive { path, members, cursor, scroll } = popup else { return };
    let w = 96u16.min(area.width.saturating_sub(2));
    let h = area.height.saturating_sub(4).max(8);
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let total: u64 = members.iter().map(|m| m.size).sum();
    let title =
        format!(" {}  —  {} entries, {} unpacked ", name, members.len(), cian_core::human_size(total));
    let inner = popup_frame(f, area, w, h, title, "");

    let body_h = inner.height.saturating_sub(1) as usize;
    keep_in_view(*cursor, scroll, body_h);
    for (row, (i, m)) in members.iter().enumerate().skip(*scroll).take(body_h).enumerate() {
        let sel = i == *cursor;
        let line = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        push_row_zone(zones, inner, inner.y + row as u16, i);
        if sel {
            f.render_widget(
                Block::default().style(Style::default().bg(theme().selected_bg)),
                line,
            );
        }
        let base = if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
        let size = if m.is_dir { "—".to_string() } else { cian_core::human_size(m.size) };
        let name_w = inner.width as usize - 14;
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if sel { " ▸ " } else { "   " }, base),
                Span::styled(
                    format!("{:<w$}", truncate_middle(&m.name, name_w), w = name_w),
                    if m.is_dir {
                        base.fg(text_tone(FileKind::Directory.color(), row_bg(sel))).add_modifier(Modifier::BOLD)
                    } else {
                        base.fg(readable_on(row_bg(sel)))
                    },
                ),
                Span::styled(format!("{:>6}", size), base.fg(muted_on(row_bg(sel)))),
            ])),
            line,
        );
    }
    f.render_widget(
        Paragraph::new(tr(lang, " Enter=extract this   a=extract all   Esc=close ", " Enter=これを展開   a=全展開   Esc=閉じる ")).style(
            accent_bar(),
        ),
        footer_row(inner),
    );
}

fn draw_palette(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    lang: Lang,
) {
    let Popup::Palette { kind, query, items, shown, cursor, scroll } = popup else { return };
    let w = 84u16.min(area.width.saturating_sub(2));
    let h = (area.height.saturating_sub(4)).clamp(6, 22);
    let title = match kind {
        PaletteKind::Commands => tr(lang, " command palette ", " コマンドパレット "),
        PaletteKind::Jump => tr(lang, " jump to ", " ジャンプ "),
        PaletteKind::File => tr(lang, " find file ", " ファイル検索 "),
    };
    let inner = popup_frame(f, area, w, h, title, "");

    // Row 0 is the live query; the list fills the rest above the footer.
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("› ", accent_on_popup()),
            Span::styled(format!("{}_", query), Style::default().fg(readable_on(theme().popup_bg))),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let list_top = inner.y + 1;
    let body_h = inner.height.saturating_sub(2) as usize;
    keep_in_view(*cursor, scroll, body_h);
    for (row, si) in (*scroll..shown.len().min(*scroll + body_h)).enumerate() {
        let idx = shown[si];
        let it = &items[idx];
        let sel = si == *cursor;
        let y = list_top + row as u16;
        let line = Rect::new(inner.x, y, inner.width, 1);
        if sel {
            f.render_widget(Block::default().style(Style::default().bg(theme().selected_bg)), line);
        }
        let base = if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
        let label_w = (inner.width as usize * 2 / 5).max(10);
        let detail_w = (inner.width as usize).saturating_sub(label_w + 4);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if sel { " ▸ " } else { "   " }, base),
                Span::styled(
                    format!("{:<w$}", truncate(&it.label, label_w), w = label_w),
                    base.fg(if sel {
                        readable_on(theme().selected_bg)
                    } else {
                        readable_on(theme().popup_bg)
                    })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(truncate_middle(&it.detail, detail_w), base.fg(muted_on(row_bg(sel)))),
            ])),
            line,
        );
    }
    if shown.is_empty() {
        f.render_widget(
            Paragraph::new(tr(lang, "  (no matches)", "  （一致なし）")).style(Style::default().fg(muted_on(theme().popup_bg))),
            Rect::new(inner.x, list_top, inner.width, 1),
        );
    }
    f.render_widget(
        Paragraph::new(tr(lang, " type to filter   ↑/↓ move   Enter run   Esc close ", " 入力で絞込   ↑/↓ 移動   Enter 実行   Esc 閉じる "))
            .style(accent_bar()),
        footer_row(inner),
    );
}

fn draw_disk_usage(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::DiskUsage { dir, entries, total, cursor, scroll } = popup else { return };
    let w = 96u16.min(area.width.saturating_sub(2));
    let h = area.height.saturating_sub(4).max(8);
    let title = format!(
        " {}  —  {}  ({} items) ",
        truncate_middle(&dir.display().to_string(), 60),
        cian_core::human_size(*total),
        entries.len()
    );
    let inner = popup_frame(f, area, w, h, title, "");

    let body_h = inner.height.saturating_sub(1) as usize;
    keep_in_view(*cursor, scroll, body_h);
    // Bars scale to the biggest child, so the space hog fills the bar.
    let max = entries.first().map(|e| e.size).unwrap_or(0).max(1);
    let bar_w = 18usize;
    for (row, (i, e)) in entries.iter().enumerate().skip(*scroll).take(body_h).enumerate() {
        let sel = i == *cursor;
        let y = inner.y + row as u16;
        let line = Rect::new(inner.x, y, inner.width, 1);
        push_row_zone(zones, inner, y, i);
        if sel {
            f.render_widget(Block::default().style(Style::default().bg(theme().selected_bg)), line);
        }
        let base = if sel { Style::default().bg(theme().selected_bg) } else { Style::default() };
        let filled = ((e.size as u128 * bar_w as u128) / max as u128) as usize;
        let bar: String = "█".repeat(filled) + &"░".repeat(bar_w.saturating_sub(filled));
        let pct = if *total > 0 { e.size as f64 * 100.0 / *total as f64 } else { 0.0 };
        let mut name = e.name.clone();
        if e.is_dir {
            name.push('/');
        }
        let name_w = (inner.width as usize).saturating_sub(bar_w + 24);
        let name_style = if e.is_dir {
            base.fg(text_tone(FileKind::Directory.color(), row_bg(sel))).add_modifier(Modifier::BOLD)
        } else {
            base.fg(readable_on(row_bg(sel)))
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if sel { " ▸ " } else { "   " }, base),
                Span::styled(format!("{:<w$}", truncate_middle(&name, name_w), w = name_w), name_style),
                Span::styled(bar, base.fg(text_tone(theme().accent, row_bg(sel)))),
                Span::styled(format!(" {:>8}", cian_core::human_size(e.size)), base.fg(readable_on(row_bg(sel)))),
                Span::styled(format!(" {:>4.0}%", pct), base.fg(muted_on(row_bg(sel)))),
            ])),
            line,
        );
    }
    if entries.is_empty() {
        f.render_widget(
            Paragraph::new(tr(lang, "  (empty)", "  （空）")).style(Style::default().fg(muted_on(theme().popup_bg))),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
    f.render_widget(
        Paragraph::new(tr(lang,
            " Enter=into folder   -=up   j/k move   Esc=close ",
            " Enter=ディレクトリへ   -=上へ   j/k 移動   Esc=閉じる ",
        ))
        .style(accent_bar()),
        footer_row(inner),
    );
}

fn draw_git_log(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::GitLog { title, commits, cursor, scroll, .. } = popup else { return };
    let rect = centered_rect(area.width.saturating_sub(4), area.height.saturating_sub(4), area);
    clear_popup(f, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type())
        .border_style(accent_on_popup())
        .style(popup_style())
        .title(format!(" {} ", title))
        .title_bottom(tr(lang, " Enter=show diff  j/k  g/G  Esc ", " Enter=差分表示  j/k  g/G  Esc "));
    let inner = rect.inner(Margin { vertical: 1, horizontal: 1 });
    f.render_widget(block, rect);
    let body_h = inner.height as usize;
    keep_in_view(*cursor, scroll, body_h);
    let hash_w = 8usize;
    let date_w = 10usize;
    let author_w = 14usize;
    let subj_w = (inner.width as usize).saturating_sub(hash_w + date_w + author_w + 3);
    let rows: Vec<Line> = commits
        .iter()
        .enumerate()
        .skip(*scroll)
        .take(body_h)
        .map(|(i, c)| {
            let sel = i == *cursor;
            let author: String = c.author.chars().take(author_w).collect();
            let subject: String = c.subject.chars().take(subj_w).collect();
            let line = format!(
                "{:<hw$} {:<dw$} {:<aw$} {}",
                c.hash, c.date, author, subject,
                hw = hash_w, dw = date_w, aw = author_w,
            );
            let style = if sel {
                Style::default().fg(readable_on(theme().accent)).bg(theme().accent)
            } else {
                Style::default().fg(readable_on(theme().popup_bg))
            };
            Line::from(Span::styled(line, style))
        })
        .collect();
    for i in 0..commits.len().min(body_h) {
        push_row_zone(zones, inner, inner.y + i as u16, *scroll + i);
    }
    f.render_widget(Paragraph::new(rows), inner);
}

fn draw_macros(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::Macros { cursor, names } = popup else { return };
    let widest = names.iter().map(|n| n.chars().count()).max().unwrap_or(10);
    let w = (widest as u16 + 8).clamp(28, area.width);
    let h = (names.len() as u16 + 3).min(area.height);
    let inner = popup_frame(f, area, w, h, tr(lang, " run a macro ", " マクロを実行 "), "");

    let rows: Vec<Line> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let sel = i == *cursor;
            let style = if sel {
                accent_on_popup()
            } else {
                Style::default().fg(readable_on(theme().popup_bg))
            };
            Line::from(Span::styled(
                format!("{}{}", if sel { "▸ " } else { "  " }, name),
                style,
            ))
        })
        .collect();
    let body_area = body_rows(inner);
    f.render_widget(Paragraph::new(rows), body_area);
    for i in 0..names.len() {
        push_row_zone(zones, inner, inner.y + i as u16, i);
    }
    let footer_area = footer_row(inner);
    f.render_widget(
        Paragraph::new(tr(lang, " Enter=run  j/k  Esc ", " Enter=実行  j/k  Esc ")).style(
            accent_bar(),
        ),
        footer_area,
    );
}

fn draw_sort_picker(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::SortPicker { cursor } = popup else { return };
    let w = 34u16.min(area.width);
    let h = SortKey::ALL.len() as u16 + 3;
    let inner = popup_frame(f, area, w, h.min(area.height), tr(lang, " sort by ", " 並び替え "), "");

    let rows: Vec<Line> = SortKey::ALL
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let sel = i == *cursor;
            let style = if sel {
                accent_on_popup()
            } else {
                Style::default().fg(readable_on(theme().popup_bg))
            };
            // The shortcut letter doubles as the mnemonic.
            let hint = match k {
                SortKey::Name => "n",
                SortKey::Size => "s",
                SortKey::Modified => "d",
                SortKey::Extension => "e",
            };
            Line::from(Span::styled(
                format!("{}{}  ({})", if sel { "▸ " } else { "  " }, sort_label(*k, lang), hint),
                style,
            ))
        })
        .collect();
    let body_area = body_rows(inner);
    f.render_widget(Paragraph::new(rows), body_area);
    for i in 0..SortKey::ALL.len() {
        push_row_zone(zones, inner, inner.y + i as u16, i);
    }
    let footer_area =
        footer_row(inner);
    f.render_widget(
        Paragraph::new(tr(lang, " Enter=apply (again = reverse)  Esc ", " Enter=適用（再度で逆順）  Esc ")).style(
            accent_bar(),
        ),
        footer_area,
    );
}

fn draw_encoding_picker(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::EncodingPicker { cursor, .. } = popup else { return };
    use cian_core::viewer::TextEncoding;
    let w = 34u16.min(area.width);
    let h = TextEncoding::ALL.len() as u16 + 3;
    let inner = popup_frame(f, area, w, h.min(area.height), tr(lang, " text encoding ", " 文字コード "), "");
    let rows: Vec<Line> = TextEncoding::ALL
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let sel = i == *cursor;
            let style = if sel {
                accent_on_popup()
            } else {
                Style::default().fg(readable_on(theme().popup_bg))
            };
            Line::from(Span::styled(
                format!("{}{}", if sel { "▸ " } else { "  " }, e.label()),
                style,
            ))
        })
        .collect();
    f.render_widget(
        Paragraph::new(rows),
        Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1)),
    );
    for i in 0..TextEncoding::ALL.len() {
        push_row_zone(zones, inner, inner.y + i as u16, i);
    }
    f.render_widget(
        Paragraph::new(tr(lang, " Enter=apply  Esc=cancel ", " Enter=適用  Esc=取消 ")).style(
            accent_bar(),
        ),
        footer_row(inner),
    );
}

fn draw_color_picker(
    f: &mut Frame,
    area: Rect,
    popup: &mut Popup,
    zones: &mut Vec<PopupZone>,
    lang: Lang,
) {
    let Popup::ColorPicker { cursor, .. } = popup else { return };
    let w = 26u16.min(area.width);
    let h = pane_bg_presets().len() as u16 + 3;
    let inner = popup_frame(f, area, w, h.min(area.height), tr(lang, " background ", " 背景色 "), "");

    let rows: Vec<Line> = pane_bg_presets()
        .iter()
        .enumerate()
        .map(|(i, (name, color))| {
            let sel = i == *cursor;
            // A swatch of the actual color, so the name is not the only cue.
            let swatch = Span::styled(
                "  ",
                Style::default().bg(color.unwrap_or(Color::Rgb(16, 16, 20))),
            );
            let label = Span::styled(
                format!(" {}{}", if sel { "▸ " } else { "  " }, name),
                if sel {
                    Style::default()
                        .add_modifier(Modifier::BOLD)
                        .fg(text_tone(theme().accent, theme().popup_bg))
                } else {
                    Style::default().fg(readable_on(theme().popup_bg))
                },
            );
            Line::from(vec![swatch, label])
        })
        .collect();
    let body_area = body_rows(inner);
    f.render_widget(Paragraph::new(rows), body_area);
    for i in 0..pane_bg_presets().len() {
        push_row_zone(zones, inner, inner.y + i as u16, i);
    }
    let footer_area =
        footer_row(inner);
    f.render_widget(
        Paragraph::new(tr(lang, " Enter=apply  Esc=cancel ", " Enter=適用  Esc=取消 ")).style(
            accent_bar(),
        ),
        footer_area,
    );
}

/// The mark for a file that is listed but not downloaded.
///
/// `☁` says it best and the window cannot draw it. A cell renderer gives a
/// glyph as much room as `unicode-width` says the *character* needs — one cell
/// for `☁`, which is "ambiguous" — and rasterises into exactly that. The fonts
/// cian ships with draw `☁` at two cells wide (measured: 1080 units against a
/// 540-unit cell), so what appeared on screen was the left half of a cloud.
///
/// A terminal has no such rule — the ink spills into the next cell and the
/// cloud survives — so the terminal keeps it. The window gets an arrow, which
/// is what the placeholder means anyway: not here yet, and reaching for it
/// fetches it.
fn cloud_mark() -> &'static str {
    cloud_mark_for(crate::theme::in_a_window())
}

/// The decision on its own, so it can be asserted without flipping a
/// process-wide switch that every other test would then see.
pub(crate) fn cloud_mark_for(in_a_window: bool) -> &'static str {
    if in_a_window {
        "↓ "
    } else {
        "☁ "
    }
}

/// A ratatui colour as plain bytes, for handing to a renderer that knows
/// nothing about themes. Anything but a truecolor value falls back to the
/// theme's plain text tone, which is what those variants resolve to anyway.
fn rgb_of(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0xcd, 0xcd, 0xda),
    }
}

#[cfg(test)]
mod md_tests {
    use super::*;

    /// Concatenate a styled run's text back to a plain string.
    fn plain(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn inline_splits_bold_and_code_but_keeps_text() {
        let base = Style::default();
        let spans = md_inline("run `ls -l` then **stop**", base, Color::Rgb(1, 2, 3));
        assert_eq!(plain(&spans), "run ls -l then stop");
        // The code span carries the code colour; the bold span the bold modifier.
        assert!(spans.iter().any(|s| s.style.fg == Some(Color::Rgb(1, 2, 3)) && s.content == "ls -l"));
        assert!(spans.iter().any(|s| s.style.add_modifier.contains(Modifier::BOLD) && s.content == "stop"));
    }

    #[test]
    fn unterminated_markers_stay_literal() {
        let spans = md_inline("a `b and **c", Style::default(), Color::Rgb(0, 0, 0));
        assert_eq!(plain(&spans), "a `b and **c");
    }

    #[test]
    fn body_line_handles_headings_bullets_and_code_fences() {
        let g = Color::Rgb(9, 9, 9);
        let b = Color::Rgb(8, 8, 8);
        let mut in_code = false;

        let head = md_body_line("## Title", 40, g, b, &mut in_code);
        assert_eq!(head.len(), 1);
        assert_eq!(head[0].0, "Title"); // hashes stripped

        let bullet = md_body_line("- item", 40, g, b, &mut in_code);
        assert!(bullet[0].0.starts_with("• "));

        // A fence flips code mode; the line inside is verbatim.
        let _fence = md_body_line("```", 40, g, b, &mut in_code);
        assert!(in_code, "opening fence enters code mode");
        let code = md_body_line("x = **not bold** here", 40, g, b, &mut in_code);
        assert_eq!(code[0].0, "x = **not bold** here");
        let _close = md_body_line("```", 40, g, b, &mut in_code);
        assert!(!in_code, "closing fence leaves code mode");
    }
}
