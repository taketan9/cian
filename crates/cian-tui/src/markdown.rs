//! A small Markdown → styled-terminal-lines renderer for the F3 viewer's
//! preview mode. It is a pragmatic, line-based parser (headings, emphasis,
//! inline/fenced code, blockquotes, lists, rules, links) — not a full CommonMark
//! engine. Pipe tables render as bordered, aligned boxes, task lists as
//! checkboxes, and ```mermaid``` `graph`/`flowchart` blocks become a readable
//! arrow-list "flow" (a terminal cannot draw the diagram itself); other mermaid
//! diagram types fall back to a clearly-boxed source block.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

// The reading — inline marks and every block recogniser — lives in cian-core,
// so the window's preview and this renderer agree about what the file says.
// What stays here is how it looks in a terminal.
use cian_core::markdown::{
    cell_align, fence_lang, heading, is_rule, is_table_separator, list_item, split_cells,
    task_item, Align,
};

use crate::render::{dim_text, readable_on, text_tone};
use crate::theme;
use crate::theme::surface;
use crate::util::{pad_left, pad_to, truncate, wrap_str};





/// The plain text of an inline-formatted string (markers stripped), for a table
/// cell — so `**x**` measures and shows as `x`.
fn plain(text: &str) -> String {
    inline(text, Style::default(), usize::MAX)
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

/// Lift a code block or a blockquote off the page it sits on.
///
/// *Away* from the page, not simply lighter: adding to every channel of a
/// light background walks it toward white, which is the page again. A dark
/// page gets lighter, a light page gets darker, and either way the block is
/// visibly a block.
fn elevate(c: Color, n: u8) -> Color {
    let Color::Rgb(r, g, b) = crate::render::as_rgb(c) else {
        // A colour that cannot be measured (a named ANSI one): leave the page
        // as it is rather than guess a direction and get it backwards.
        return c;
    };
    if crate::render::rel_luminance(Color::Rgb(r, g, b)) > 0.5 {
        Color::Rgb(r.saturating_sub(n), g.saturating_sub(n), b.saturating_sub(n))
    } else {
        Color::Rgb(r.saturating_add(n), g.saturating_add(n), b.saturating_add(n))
    }
}

/// Pad `s` to `w` display columns per `align` (assumes `s.width() <= w`).
fn pad_align(s: &str, w: usize, align: Align) -> String {
    match align {
        Align::Left => pad_to(s, w),
        Align::Right => pad_left(s, w),
        Align::Center => {
            let gap = w.saturating_sub(s.width());
            let l = gap / 2;
            format!("{}{s}{}", " ".repeat(l), " ".repeat(gap - l))
        }
    }
}

/// Render a pipe-table as a bordered, column-aligned box.
fn render_table(
    header: &[String],
    aligns: &[Align],
    rows: &[Vec<String>],
    width: usize,
) -> Vec<Line<'static>> {
    let ncols = header.len().max(1);
    let cell = |row: &[String], i: usize| plain(row.get(i).map(String::as_str).unwrap_or(""));
    let align = |i: usize| aligns.get(i).copied().unwrap_or(Align::Left);

    // Natural column widths from the content.
    let mut colw = vec![0usize; ncols];
    for (i, w) in colw.iter_mut().enumerate() {
        *w = plain(header.get(i).map(String::as_str).unwrap_or("")).width();
    }
    for row in rows {
        for (i, w) in colw.iter_mut().enumerate() {
            *w = (*w).max(cell(row, i).width());
        }
    }
    // Shrink the widest columns until the whole table fits `width`. Frame cost is
    // `3*ncols + 1` (a `│`, two padding spaces per column, plus the last `│`).
    let frame = 3 * ncols + 1;
    let budget = width.saturating_sub(frame).max(ncols); // ≥ 1 col each
    while colw.iter().sum::<usize>() > budget {
        let (idx, _) = colw.iter().enumerate().max_by_key(|(_, w)| **w).unwrap();
        if colw[idx] <= 1 {
            break;
        }
        colw[idx] -= 1;
    }

    let border = Style::default().fg(dim_text(surface()));
    let head_style = Style::default()
        .fg(text_tone(theme().accent, surface()))
        .add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(readable_on(surface()));

    // Border rows: left/mid/right corners joined by `fill` across each column.
    let rule = |left: &str, mid: &str, right: &str, fill: &str| {
        let mut s = String::from(left);
        for (i, w) in colw.iter().enumerate() {
            s.push_str(&fill.repeat(w + 2));
            s.push_str(if i + 1 == ncols { right } else { mid });
        }
        Line::from(Span::styled(s, border))
    };
    let data_row = |cells: &dyn Fn(usize) -> String, style: Style| {
        let mut spans = vec![Span::styled("│".to_string(), border)];
        for (i, w) in colw.iter().enumerate() {
            let text = truncate(&cells(i), *w);
            spans.push(Span::styled(format!(" {} ", pad_align(&text, *w, align(i))), style));
            spans.push(Span::styled("│".to_string(), border));
        }
        Line::from(spans)
    };

    let mut out = Vec::new();
    out.push(rule("┌", "┬", "┐", "─"));
    let hdr: Vec<String> = (0..ncols).map(|i| plain(header.get(i).map(String::as_str).unwrap_or(""))).collect();
    out.push(data_row(&|i| hdr[i].clone(), head_style));
    out.push(rule("├", "┼", "┤", "─"));
    for row in rows {
        let r: Vec<String> = (0..ncols).map(|i| cell(row, i)).collect();
        out.push(data_row(&|i| r[i].clone(), body_style));
    }
    out.push(rule("└", "┴", "┘", "─"));
    out
}

/// Render Markdown to a plain-text grid plus a parallel per-character style
/// grid. The viewer drives its cursor / selection / search over the plain text
/// and paints each character with the matching base style, so all the viewer's
/// machinery works over the rendered document unchanged.
pub(crate) fn render_styled(
    source: &[String],
    width: usize,
) -> (Vec<String>, Vec<Vec<Style>>, Vec<usize>) {
    let (lines, map) = render_mapped(source, width);
    let mut plain = Vec::with_capacity(lines.len());
    let mut styles = Vec::with_capacity(lines.len());
    for line in &lines {
        let mut text = String::new();
        let mut st = Vec::new();
        for span in &line.spans {
            for ch in span.content.chars() {
                text.push(ch);
                st.push(span.style);
            }
        }
        plain.push(text);
        styles.push(st);
    }
    (plain, styles, map)
}

/// The same render, plus which source line each display line came from.
///
/// A rendered document has neither the same number of lines as its source nor
/// the same order of them — a heading loses its `#`, a paragraph wraps into
/// four, a code fence swallows its own delimiters. Anything that knows about
/// the *file* (the outline, and so the `]]` motion and the highlighted entry)
/// therefore speaks a different set of line numbers from anything that knows
/// about the *screen*. This is the dictionary between them.
///
/// The mark is taken at the top of each iteration, before the body decides how
/// many lines it will emit, so the several `continue`s below cannot skip it.
pub(crate) fn render_mapped(source: &[String], width: usize) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut marks: Vec<(usize, usize)> = Vec::new();
    let out = render_inner(source, width, &mut marks);
    // Expand "output from here on belongs to source line N" into one entry per
    // output line.
    let mut map = vec![0usize; out.len()];
    for w in 0..marks.len() {
        let (from, src) = marks[w];
        let to = marks.get(w + 1).map(|m| m.0).unwrap_or(out.len());
        for slot in map.iter_mut().take(to.min(out.len())).skip(from) {
            *slot = src;
        }
    }
    (out, map)
}

fn render_inner(
    source: &[String],
    width: usize,
    marks: &mut Vec<(usize, usize)>,
) -> Vec<Line<'static>> {
    let width = width.max(8);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut i = 0;
    while i < source.len() {
        marks.push((out.len(), i));
        let raw = &source[i];
        let trimmed = raw.trim_start();

        // Fenced code block: ``` or ~~~ (optionally with a language).
        if let Some(lang) = fence_lang(trimmed) {
            i += 1;
            let mut code = Vec::new();
            while i < source.len() && fence_lang(source[i].trim_start()).is_none() {
                code.push(source[i].clone());
                i += 1;
            }
            i += 1; // consume the closing fence (if any)
            out.extend(code_block(&lang, &code, width));
            continue;
        }

        // Horizontal rule.
        if is_rule(trimmed) {
            out.push(Line::from(Span::styled(
                "─".repeat(width),
                Style::default().fg(dim_text(surface())),
            )));
            i += 1;
            continue;
        }

        // ATX heading (# .. ######).
        if let Some((level, text)) = heading(trimmed) {
            if !out.is_empty() {
                out.push(Line::from(""));
            }
            // Measured against the page: a heading is the one line that has
            // to be seen from across the room, and some palettes put their
            // accent two shades from their own paper.
            let color = text_tone(theme().accent, surface());
            let prefix = match level {
                1 => "█ ",
                2 => "▊ ",
                _ => "▎ ",
            };
            let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
            let mut spans = vec![Span::styled(prefix.to_string(), style)];
            spans.extend(inline(&text, style, width.saturating_sub(2)));
            out.push(Line::from(spans));
            if level <= 2 {
                out.push(Line::from(Span::styled(
                    "─".repeat(width),
                    Style::default().fg(dim_text(surface())),
                )));
            }
            i += 1;
            continue;
        }

        // Blockquote — a coloured left bar over a subtly raised background band
        // so it reads as a quote against the themed viewer surface.
        if let Some(rest) = trimmed.strip_prefix('>') {
            let qbg = elevate(surface(), 14);
            let bar = Style::default().fg(text_tone(theme().accent, qbg)).bg(qbg).add_modifier(Modifier::BOLD);
            let body = Style::default().fg(readable_on(qbg)).bg(qbg).add_modifier(Modifier::ITALIC);
            for chunk in wrap_str(rest.trim_start(), width.saturating_sub(2)) {
                let mut spans = vec![Span::styled("▎ ".to_string(), bar)];
                spans.extend(inline(&chunk, body, width));
                // Pad the band to full width so the background is a solid block.
                let used: usize = spans.iter().map(|s| s.content.width()).sum();
                if used < width {
                    spans.push(Span::styled(" ".repeat(width - used), Style::default().bg(qbg)));
                }
                out.push(Line::from(spans));
            }
            i += 1;
            continue;
        }

        // Pipe table: a header row, a `|---|:--:|` separator, then body rows.
        if raw.contains('|')
            && i + 1 < source.len()
            && is_table_separator(&source[i + 1])
        {
            let header = split_cells(raw);
            let aligns = split_cells(&source[i + 1]).iter().map(|c| cell_align(c)).collect::<Vec<_>>();
            i += 2;
            let mut rows = Vec::new();
            while i < source.len() {
                let r = source[i].trim();
                if r.is_empty() || !r.contains('|') {
                    break;
                }
                rows.push(split_cells(&source[i]));
                i += 1;
            }
            out.extend(render_table(&header, &aligns, &rows, width));
            continue;
        }

        // Unordered / ordered list item.
        if let Some((marker, text, indent)) = list_item(raw) {
            let pad = " ".repeat(indent);
            // GitHub task list: `- [ ]` / `- [x]` becomes a checkbox glyph in
            // place of the bullet, with the marker stripped from the text.
            let (marker, text, mstyle) = if let Some(r) = task_item(&text) {
                match r {
                    (true, rest) => (
                        "☑".to_string(),
                        rest,
                        Style::default()
                            .fg(text_tone(theme().file.executable, surface()))
                            .add_modifier(Modifier::BOLD),
                    ),
                    (false, rest) => (
                        "☐".to_string(),
                        rest,
                        Style::default().fg(theme().dim).add_modifier(Modifier::BOLD),
                    ),
                }
            } else {
                (
                    marker,
                    text,
                    Style::default()
                        .fg(text_tone(theme().accent, surface()))
                        .add_modifier(Modifier::BOLD),
                )
            };
            let avail = width.saturating_sub(indent + marker.chars().count() + 1);
            let wrapped = inline(&text, Style::default().fg(readable_on(surface())), avail);
            let mut spans = vec![
                Span::styled(pad.clone(), Style::default()),
                Span::styled(format!("{} ", marker), mstyle),
            ];
            spans.extend(wrapped);
            out.push(Line::from(spans));
            i += 1;
            continue;
        }

        // Blank line.
        if trimmed.is_empty() {
            out.push(Line::from(""));
            i += 1;
            continue;
        }

        // Plain paragraph text, wrapped.
        let base = Style::default().fg(readable_on(surface()));
        for chunk in wrap_str(trimmed, width) {
            out.push(Line::from(inline(&chunk, base, width)));
        }
        i += 1;
    }
    out
}





/// One token of a flowchart line: a node reference (raw text incl. any brackets)
/// or an arrow with its optional `|label|`.
enum Tok {
    Node(String),
    Arrow(String),
}

/// Collect `id[label]` / `id(label)` / `id{label}` / `id((label))` declarations
/// from one line into `map` (id → display label), so a later bare `id` in an
/// edge can be shown by its label.
fn collect_node_labels(line: &str, map: &mut std::collections::HashMap<String, String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_alphanumeric() || chars[i] == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let id: String = chars[start..i].iter().collect();
            if let Some((label, next)) = read_bracket_label(&chars, i) {
                map.entry(id).or_insert(label);
                i = next;
            }
        } else {
            i += 1;
        }
    }
}

/// If `chars[i]` opens a node label (`[`, `(`, `{`, possibly doubled like `((`),
/// return the inner label (quotes stripped) and the index past the close.
fn read_bracket_label(chars: &[char], i: usize) -> Option<(String, usize)> {
    let open = *chars.get(i)?;
    let close = match open {
        '[' => ']',
        '(' => ')',
        '{' => '}',
        _ => return None,
    };
    let mut j = i;
    let mut depth = 0; // count leading opens (handles (( )) / [[ ]])
    while chars.get(j) == Some(&open) {
        depth += 1;
        j += 1;
    }
    let text_start = j;
    let mut closes = 0;
    while j < chars.len() && closes < depth {
        if chars[j] == close {
            closes += 1;
        } else {
            closes = 0;
        }
        j += 1;
    }
    let label: String = chars[text_start..j.saturating_sub(depth)].iter().collect();
    let label = label.trim().trim_matches('"').trim().to_string();
    Some((label, j))
}

/// The display label for a node reference token like `A`, `A[X]`, `A(( Y ))`.
fn node_label(token: &str, map: &std::collections::HashMap<String, String>) -> String {
    let chars: Vec<char> = token.trim().chars().collect();
    let mut i = 0;
    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
        i += 1;
    }
    let id: String = chars[..i].iter().collect();
    if let Some((label, _)) = read_bracket_label(&chars, i) {
        if !label.is_empty() {
            return label;
        }
    }
    map.get(&id).cloned().unwrap_or(id)
}

/// Tokenise a flowchart line into nodes and arrows.
fn tokenize_flow(line: &str) -> Vec<Tok> {
    let s = line.trim().trim_end_matches(';');
    let chars: Vec<char> = s.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    let mut buf = String::new();
    let mut depth = 0i32; // inside [] () {}
    while i < chars.len() {
        let c = chars[i];
        if depth == 0 && (c == '-' || c == '=' || c == '<') {
            // Start of an arrow.
            if !buf.trim().is_empty() {
                toks.push(Tok::Node(std::mem::take(&mut buf)));
            } else {
                buf.clear();
            }
            while i < chars.len() && matches!(chars[i], '-' | '=' | '.' | '<' | '>') {
                i += 1;
            }
            if matches!(chars.get(i), Some('x') | Some('o')) {
                i += 1; // --x / --o arrowheads
            }
            let mut label = String::new();
            if chars.get(i) == Some(&'|') {
                i += 1;
                while i < chars.len() && chars[i] != '|' {
                    label.push(chars[i]);
                    i += 1;
                }
                i += 1; // closing |
            }
            toks.push(Tok::Arrow(label.trim().to_string()));
            continue;
        }
        if matches!(c, '[' | '(' | '{') {
            depth += 1;
        } else if matches!(c, ']' | ')' | '}') {
            depth -= 1;
        }
        buf.push(c);
        i += 1;
    }
    if !buf.trim().is_empty() {
        toks.push(Tok::Node(buf));
    }
    toks
}

/// Render a `graph` / `flowchart` mermaid block as a readable list of edges
/// (`from ──label──▶ to`) with node labels resolved. `None` for non-flow diagram
/// types (sequence, class, …), which fall back to the source box.
fn mermaid_flow(lines: &[String], width: usize) -> Option<Vec<Line<'static>>> {
    let mut idx = 0;
    while idx < lines.len() && lines[idx].trim().is_empty() {
        idx += 1;
    }
    let header = lines.get(idx)?.trim();
    if !(header.starts_with("graph") || header.starts_with("flowchart")) {
        return None;
    }

    let mut map = std::collections::HashMap::new();
    for l in lines {
        collect_node_labels(l, &mut map);
    }

    // Edges may sit after the direction on the header line (`graph TD; A-->B`)
    // as well as on their own lines, so parse the header's tail too.
    let head_tail = header.split_once(';').map(|(_, t)| t.to_string()).unwrap_or_default();
    let mut edges: Vec<(String, String, String)> = Vec::new();
    for l in std::iter::once(&head_tail).chain(lines[idx + 1..].iter()) {
        let lt = l.trim();
        if lt.is_empty() || lt.starts_with("%%") || lt.starts_with("subgraph") || lt == "end" {
            continue;
        }
        let toks = tokenize_flow(l);
        let mut k = 0;
        while k + 2 < toks.len() + 1 {
            if let (Some(Tok::Node(a)), Some(Tok::Arrow(lbl)), Some(Tok::Node(b))) =
                (toks.get(k), toks.get(k + 1), toks.get(k + 2))
            {
                edges.push((node_label(a, &map), lbl.clone(), node_label(b, &map)));
                k += 2; // chain: reuse b as the next `from`
            } else {
                break;
            }
        }
    }
    if edges.is_empty() {
        return None;
    }

    let base = surface();
    let bg = elevate(base, 14);
    let node = Style::default().fg(readable_on(bg)).bg(bg).add_modifier(Modifier::BOLD);
    let arrow = Style::default().fg(text_tone(theme().accent, bg)).bg(bg);
    let lbl = Style::default()
        .fg(text_tone(theme().file.executable, bg))
        .bg(bg)
        .add_modifier(Modifier::ITALIC);
    let fill = Style::default().bg(bg);

    let mut out = Vec::new();
    out.push(Line::from(Span::styled(
        format!("{:<w$}", " mermaid flow ", w = width),
        Style::default()
            .bg(elevate(base, 30))
            .fg(text_tone(theme().accent, elevate(base, 30)))
            .add_modifier(Modifier::BOLD),
    )));
    for (from, elabel, to) in &edges {
        let mut spans = vec![Span::styled("  ".to_string(), fill), Span::styled(from.clone(), node)];
        if elabel.is_empty() {
            spans.push(Span::styled("  ──▶  ".to_string(), arrow));
        } else {
            spans.push(Span::styled("  ──".to_string(), arrow));
            spans.push(Span::styled(elabel.clone(), lbl));
            spans.push(Span::styled("──▶  ".to_string(), arrow));
        }
        spans.push(Span::styled(to.clone(), node));
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        if used < width {
            spans.push(Span::styled(" ".repeat(width - used), fill));
        }
        out.push(Line::from(spans));
    }
    Some(out)
}


/// A fenced code block as boxed, monospaced lines, on a theme-derived surface
/// raised off the viewer background. A ```mermaid``` block is first parsed into a
/// readable flow (arrows between node labels); only if that fails does it fall
/// back to the labelled source box (a terminal cannot draw the diagram itself).
fn code_block(lang: &str, lines: &[String], width: usize) -> Vec<Line<'static>> {
    if lang == "mermaid" {
        if let Some(flow) = mermaid_flow(lines, width) {
            return flow;
        }
    }
    let base = surface();
    let code_bg = elevate(base, 18);
    let bg = Style::default().bg(code_bg);
    let mut out = Vec::new();
    let label = if lang == "mermaid" {
        " mermaid (source) ".to_string()
    } else if lang.is_empty() {
        " code ".to_string()
    } else {
        format!(" {} ", lang)
    };
    out.push(Line::from(Span::styled(
        // 見出しの帯。`lang` は英字がふつうだが、囲いに何を書くかは書き手
        // 次第なので、桁で詰める（`pad_to` は溢れたものを切らないので、
        // いまの見え方は変わらない）。
        pad_to(&label, width),
        Style::default()
            .bg(elevate(base, 34))
            .fg(text_tone(theme().accent, elevate(base, 34)))
            .add_modifier(Modifier::BOLD),
    )));
    let code_fg = readable_on(code_bg);
    for l in lines {
        // **コードの行は日本語を含む。** `{:<w$}` は字で詰めるので、
        // 日本語のコメントや文字列がある行だけ帯が足りなかった。
        let shown = format!("  {}", pad_to(l, width.saturating_sub(2)));
        out.push(Line::from(Span::styled(shown, bg.fg(code_fg))));
    }
    out
}

/// Parse inline emphasis / code / links in `text` into styled spans, on top of
/// `base`. Wrapping is left to the caller (the text is already a chunk).
fn inline(text: &str, base: Style, _width: usize) -> Vec<Span<'static>> {
    // Inline code sits on a block of its own, lifted off the page in
    // whichever direction the page is not — it was a fixed dark box with
    // yellow text, which on a light theme is a black hole in the paragraph.
    let code_bg = elevate(surface(), 18);
    // The theme's code colour, pulled far enough off the block behind it to
    // read — some palettes put it two shades from their own paper.
    let mut code_fg = text_tone(theme().file.code, code_bg);
    if crate::render::contrast_ratio(code_fg, code_bg) < 4.0 {
        code_fg = readable_on(code_bg);
    }
    let code_style = Style::default().bg(code_bg).fg(code_fg);
    // The accent is chosen to be an accent on the theme's own page; a link
    // sits on that page and has to be read, not just noticed.
    let link_style = base
        .fg(text_tone(theme().accent, surface()))
        .add_modifier(Modifier::UNDERLINED);

    // The reading is cian-core's, shared with the window — this is only the
    // dressing. Two front ends with two parsers would be two opinions about
    // what `*a_b*` means, which reads as carelessness in a program's own
    // README.
    fn hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
        let h = hex.strip_prefix('#')?;
        if h.len() != 6 {
            return None;
        }
        Some((
            u8::from_str_radix(&h[0..2], 16).ok()?,
            u8::from_str_radix(&h[2..4], 16).ok()?,
            u8::from_str_radix(&h[4..6], 16).ok()?,
        ))
    }

    let mut spans: Vec<Span<'static>> = cian_core::markdown::inline(text)
        .into_iter()
        .map(|piece| match piece {
            cian_core::markdown::Inline::Text(t) => Span::styled(t, base),
            cian_core::markdown::Inline::Code(t) => Span::styled(t, code_style),
            cian_core::markdown::Inline::Bold(t) => {
                Span::styled(t, base.add_modifier(Modifier::BOLD))
            }
            cian_core::markdown::Inline::Italic(t) => {
                Span::styled(t, base.add_modifier(Modifier::ITALIC))
            }
            cian_core::markdown::Inline::Strike(t) => {
                Span::styled(t, base.add_modifier(Modifier::CROSSED_OUT))
            }
            cian_core::markdown::Inline::Link { text, .. } => Span::styled(text, link_style),
            // The terminal draws it in the colour that was written down.
            // A terminal that cannot do 24-bit colour degrades on its own —
            // crossterm falls back, and the words are readable either way.
            cian_core::markdown::Inline::Colored { text, color } => {
                match hex_rgb(&color) {
                    Some((r, g, b)) => Span::styled(text, base.fg(Color::Rgb(r, g, b))),
                    None => Span::styled(text, base),
                }
            }
        })
        .collect();
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}


#[cfg(test)]
mod tests {
    use super::*;


    fn lines(s: &str) -> Vec<String> {
        s.lines().map(|l| l.to_string()).collect()
    }

    #[test]
    fn headings_lists_and_code_render_to_lines() {
        let src = lines("# Title\n\nSome **bold** and `code`.\n\n- one\n- two\n\n```mermaid\ngraph TD; A-->B\n```\n");
        let out = render_mapped(&src, 40).0;
        // The title text survives (styling aside), and the mermaid flow renders
        // the A → B edge with node labels and an arrow.
        let flat: Vec<String> = out.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect();
        assert!(flat.iter().any(|l| l.contains("Title")));
        assert!(flat.iter().any(|l| l.contains("mermaid flow")));
        assert!(flat.iter().any(|l| l.contains("A") && l.contains("▶") && l.contains("B")));
        assert!(flat.iter().any(|l| l.contains("• one")));
    }

    #[test]
    fn a_pipe_table_renders_bordered_and_aligned() {
        let src = lines("| Name | Qty | Note |\n|:-----|----:|:----:|\n| apple | 3 | ok |\n| pear | 12 | **hi** |\n");
        let out = render_mapped(&src, 60).0;
        let flat: Vec<String> = out
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect();
        // Box-drawing frame present.
        assert!(flat.iter().any(|l| l.starts_with('┌') && l.ends_with('┐')), "top border: {flat:?}");
        assert!(flat.iter().any(|l| l.starts_with('├')), "header separator");
        assert!(flat.iter().any(|l| l.starts_with('└')), "bottom border");
        // Header and cells appear; emphasis markers are stripped in a cell.
        assert!(flat.iter().any(|l| l.contains("Name") && l.contains("Qty")));
        assert!(flat.iter().any(|l| l.contains("apple")));
        assert!(flat.iter().any(|l| l.contains("hi") && !l.contains("**hi**")), "markers stripped");
        // Right-aligned Qty column: "12" hugs the right padding.
        assert!(flat.iter().any(|l| l.contains("12 │")), "right-aligned qty: {flat:?}");
    }

    #[test]
    fn mermaid_flow_and_tasklist_render() {
        let src = lines("```mermaid\ngraph TD\n    A[ファイラー] -->|F3| B(ビューア)\n    B --> C{Markdown?}\n    C -->|Yes| D[プレビュー]\n    C -->|No| E[プレーン]\n```\n\n- [ ] todo\n- [x] done\n");
        let out = render_mapped(&src, 60).0;
        let flat: Vec<String> = out
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect();
        // Node labels resolved (not raw ids), arrows and edge labels shown.
        assert!(flat.iter().any(|l| l.contains("ファイラー") && l.contains("ビューア") && l.contains("F3")));
        assert!(flat.iter().any(|l| l.contains("Markdown?") && l.contains("Yes") && l.contains("プレビュー")));
        assert!(!flat.iter().any(|l| l.contains("A[ファイラー]")), "raw node syntax gone");
        // Task list: checkboxes, not bullets with literal [ ].
        assert!(flat.iter().any(|l| l.contains("☐") && l.contains("todo") && !l.contains("[ ]")));
        assert!(flat.iter().any(|l| l.contains("☑") && l.contains("done") && !l.contains("[x]")));
    }

    #[test]
    fn inline_emphasis_splits_into_spans() {
        let sp = inline("a **b** c", Style::default(), 40);
        // "a ", "b" (bold), " c" — the bold word is its own span.
        assert!(sp.iter().any(|s| s.content == "b" && s.style.add_modifier.contains(Modifier::BOLD)));
    }

    #[test]
    fn inline_code_in_parens_is_not_padded() {
        // Regression: `(`meso`)` must render the code tight, not "( meso )".
        let sp = inline("hoge(`meso`)", Style::default(), 40);
        assert!(sp.iter().any(|s| s.content == "meso"), "code content is exactly `meso`: {:?}",
            sp.iter().map(|s| s.content.as_ref()).collect::<Vec<_>>());
        let flat: String = sp.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(flat, "hoge(meso)", "no stray spaces around the code");
    }
}
