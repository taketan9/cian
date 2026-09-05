//! Reading Markdown, once, for two front ends that draw it differently.
//!
//! The terminal build had all of this fused into its renderer: recognise a
//! heading and emit a styled ratatui line in the same breath. That works while
//! there is one way to draw, and the window is a second way — so the
//! recognising moved down here and the drawing stayed up there.
//!
//! **The alternative was a second parser**, and a second parser is a second
//! opinion about what `*a_b*` means. Two front ends of one program disagreeing
//! about their own README is a small thing that reads as carelessness.
//!
//! Deliberately not CommonMark. cian reads the Markdown people write in
//! READMEs and notes — headings, lists, fences, tables, task boxes, and the
//! four inline marks — and stops there. A full implementation is a large
//! dependency and most of it would never be reached.

/// A run of text with one meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    /// `` `code` `` — one span, never nested.
    Code(String),
    Bold(String),
    Italic(String),
    Strike(String),
    Link {
        text: String,
        url: String,
    },
    /// `<span style="color:#rrggbb">…</span>` — the one piece of HTML cian
    /// reads, because Markdown has no colour and this is the notation the
    /// most other tools already understand. **Only a validated hex colour
    /// ever gets through**, so the promise made at `html` — that everything
    /// from the file is escaped — still holds.
    Colored { text: String, color: String },
}

impl Inline {
    /// The characters, with the marks dropped. For a plain-text need — a
    /// width measurement, a search — where the emphasis does not matter.
    pub fn text(&self) -> &str {
        match self {
            Inline::Text(t)
            | Inline::Code(t)
            | Inline::Bold(t)
            | Inline::Italic(t)
            | Inline::Strike(t) => t,
            Inline::Link { text, .. } => text,
            Inline::Colored { text, .. } => text,
        }
    }
}

/// How a table column is lined up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Split one line into its runs.
///
/// A scanner rather than a grammar: Markdown's inline marks are not nested in
/// practice — nobody writes bold inside a link inside italics — and a scanner
/// that gives up on an unclosed mark leaves it as text, which is what a reader
/// expects from a stray asterisk.
pub fn inline(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Inline> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    let flush = |out: &mut Vec<Inline>, buf: &mut String| {
        if !buf.is_empty() {
            out.push(Inline::Text(std::mem::take(buf)));
        }
    };

    while i < chars.len() {
        let c = chars[i];

        // A colour span, `<span style="color:#rrggbb">…</span>`.
        if c == '<' {
            let rest: String = chars[i..].iter().collect();
            if let Some((inner, color, took)) = first_color(&rest) {
                flush(&mut out, &mut buf);
                out.push(Inline::Colored { text: inner, color });
                i += took;
                continue;
            }
        }

        // Inline code first: everything inside a backtick pair is literal, so
        // a `*` in there is an asterisk and not the start of emphasis.
        if c == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&x| x == '`') {
                flush(&mut out, &mut buf);
                out.push(Inline::Code(chars[i + 1..i + 1 + end].iter().collect()));
                i += end + 2;
                continue;
            }
        }

        // Bold **…** or __…__
        if (c == '*' || c == '_') && i + 1 < chars.len() && chars[i + 1] == c {
            if let Some(end) = find_run(&chars, i + 2, [c, c]) {
                flush(&mut out, &mut buf);
                out.push(Inline::Bold(chars[i + 2..end].iter().collect()));
                i = end + 2;
                continue;
            }
        }

        // Strikethrough ~~…~~
        if c == '~' && i + 1 < chars.len() && chars[i + 1] == '~' {
            if let Some(end) = find_run(&chars, i + 2, ['~', '~']) {
                flush(&mut out, &mut buf);
                out.push(Inline::Strike(chars[i + 2..end].iter().collect()));
                i = end + 2;
                continue;
            }
        }

        // Italic *…* or _…_. A leading space rules it out, which is what stops
        // `a * b * c` from turning half a sum into emphasis.
        if c == '*' || c == '_' {
            if let Some(end) = chars[i + 1..].iter().position(|&x| x == c) {
                let inner: String = chars[i + 1..i + 1 + end].iter().collect();
                if !inner.is_empty() && !inner.starts_with(' ') {
                    flush(&mut out, &mut buf);
                    out.push(Inline::Italic(inner));
                    i += end + 2;
                    continue;
                }
            }
        }

        // Link [text](url)
        if c == '[' {
            if let Some(close) = chars[i + 1..].iter().position(|&x| x == ']') {
                let after = i + 1 + close + 1;
                if chars.get(after) == Some(&'(') {
                    if let Some(paren) = chars[after + 1..].iter().position(|&x| x == ')') {
                        flush(&mut out, &mut buf);
                        out.push(Inline::Link {
                            text: chars[i + 1..i + 1 + close].iter().collect(),
                            url: chars[after + 1..after + 1 + paren].iter().collect(),
                        });
                        i = after + 1 + paren + 1;
                        continue;
                    }
                }
            }
        }

        buf.push(c);
        i += 1;
    }
    flush(&mut out, &mut buf);
    out
}

/// The start of a two-character `marker` run at or after `from`.
fn find_run(chars: &[char], from: usize, marker: [char; 2]) -> Option<usize> {
    let mut i = from;
    while i + 1 < chars.len() {
        if chars[i] == marker[0] && chars[i + 1] == marker[1] {
            return Some(i);
        }
        i += 1;
    }
    None
}

// ---- Block recognisers ----
//
// Each answers one question about one line and nothing else. They were already
// separate in the terminal build, which is why they could come down here
// unchanged.

/// `## Heading` → `(2, "Heading")`.
/// A heading's anchor, GitHub's way: lowercased, spaces to hyphens, and
/// punctuation dropped.
///
/// **Runs of hyphens are not collapsed, and that is deliberate.** GitHub's
/// slugger turns `v1.2 — notes` into `v12--notes` — the dash is dropped and
/// the two spaces around it each become a hyphen — and the links inside a
/// README were written against *that*. A tidier anchor would be a prettier
/// string that none of the document's own links point at.
///
/// Japanese is *kept*, not stripped. GitHub percent-encodes it in the href and
/// leaves the characters in the id — strip them and every heading in a
/// Japanese document collapses to the same empty anchor, which is worse than
/// no anchor at all. The window decodes the href before it looks the id up.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().chars() {
        if c.is_whitespace() {
            out.push('-');
        } else if c.is_alphanumeric() || c == '-' || c == '_' {
            out.extend(c.to_lowercase());
        }
        // Everything else — `.`, `(`, `:`, an emoji — is dropped, as GitHub
        // drops it.
    }
    out
}

pub fn heading(line: &str) -> Option<(usize, String)> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = t[hashes..].trim_start();
    // `#hashtag` is not a heading: a heading has a space after its hashes.
    if rest.len() == t.len() - hashes {
        return None;
    }
    Some((hashes, rest.to_string()))
}

/// `---`, `***`, `___` on their own.
pub fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

/// ` ```rust ` → `Some("rust")`; ` ``` ` → `Some("")`.
pub fn fence_lang(line: &str) -> Option<String> {
    let t = line.trim_start();
    t.strip_prefix("```").map(|rest| rest.trim().to_string())
}

/// `- item` / `1. item` → `(marker, text, indent)`.
pub fn list_item(raw: &str) -> Option<(String, String, usize)> {
    let indent = raw.len() - raw.trim_start().len();
    let t = raw.trim_start();
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(m) {
            return Some(("•".to_string(), rest.to_string(), indent));
        }
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let after = &t[digits..];
        for m in [". ", ") "] {
            if let Some(rest) = after.strip_prefix(m) {
                return Some((format!("{}{}", &t[..digits], m.trim_end()), rest.to_string(), indent));
            }
        }
    }
    None
}

/// `[ ] thing` / `[x] thing` → `(done, text)`.
pub fn task_item(text: &str) -> Option<(bool, String)> {
    let t = text.trim_start();
    for (mark, done) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
        if let Some(rest) = t.strip_prefix(mark) {
            return Some((done, rest.to_string()));
        }
    }
    None
}

/// `| --- | :-: |` — the line under a table's header.
pub fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    if !t.contains('-') || !t.starts_with('|') {
        return false;
    }
    t.trim_matches('|')
        .split('|')
        .all(|c| {
            let c = c.trim();
            !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')
        })
}

/// The cells of `| a | b |`.
pub fn split_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

/// `:-:` → centre, `--:` → right, anything else → left.
pub fn cell_align(sep: &str) -> Align {
    let s = sep.trim();
    match (s.starts_with(':'), s.ends_with(':')) {
        (true, true) => Align::Center,
        (false, true) => Align::Right,
        _ => Align::Left,
    }
}

// ---- Rendering to HTML ----
//
// The window's half. The terminal build draws the same parse as styled lines;
// this turns it into a document, which is the one thing a window can do that a
// terminal cannot — real proportional type, real tables, a real code block.
//
// **Every piece of text is escaped.** A README is a file from somewhere, and a
// preview that runs what it finds is a preview that runs whatever was in the
// repository somebody cloned.

/// Escape the five characters that would otherwise be markup.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A URL safe to put in an `href`.
///
/// `javascript:` in a link is the oldest trick there is, and a README is a
/// file from somewhere. Anything that is not plainly http, https, mailto or a
/// relative path becomes no link at all — shown as text, so nothing is hidden,
/// just not clickable.
fn safe_url(url: &str) -> Option<String> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
        return Some(esc(u));
    }
    // A relative path: no scheme at all. `foo:bar` might be one, so anything
    // with a colon before the first slash is refused.
    let scheme_ish = u.split('/').next().unwrap_or("").contains(':');
    if !scheme_ish && !u.is_empty() {
        return Some(esc(u));
    }
    None
}

fn inline_html(text: &str) -> String {
    let mut out = String::new();
    for piece in inline(text) {
        match piece {
            Inline::Text(t) => out.push_str(&esc(&t)),
            Inline::Code(t) => {
                out.push_str("<code>");
                out.push_str(&esc(&t));
                out.push_str("</code>");
            }
            Inline::Bold(t) => {
                out.push_str("<strong>");
                out.push_str(&esc(&t));
                out.push_str("</strong>");
            }
            Inline::Italic(t) => {
                out.push_str("<em>");
                out.push_str(&esc(&t));
                out.push_str("</em>");
            }
            Inline::Strike(t) => {
                out.push_str("<del>");
                out.push_str(&esc(&t));
                out.push_str("</del>");
            }
            // The colour was validated as six hex digits before it got
            // here, so this is the one place a style attribute is written
            // and it cannot carry anything else.
            Inline::Colored { text, color } => {
                out.push_str(&format!(
                    "<span style=\"color:{color}\">{}</span>",
                    esc(&text)
                ));
            }
            Inline::Link { text, url } => match safe_url(&url) {
                Some(href) => {
                    out.push_str(&format!("<a href=\"{href}\">{}</a>", esc(&text)));
                }
                // Shown, not hidden — and not clickable.
                None => out.push_str(&esc(&text)),
            },
        }
    }
    out
}

/// Render Markdown as HTML.
///
/// Line-based, like the terminal renderer it shares a parser with: cian reads
/// the Markdown people write, not the Markdown a specification describes.
pub fn to_html(lines: &[String]) -> String {
    let mut out = String::new();
    // The front matter goes, if there is one: it is how a document describes
    // itself, not something it says. `front_matter_lines` decides whether the
    // leading `---` is a front matter or a horizontal rule.
    let mut i = front_matter_lines(lines);
    // Which list levels are open, by indent. Markdown's nesting is indentation
    // and nothing else, so this is the whole of it.
    let mut open_lists: Vec<usize> = Vec::new();

    // A list item stays open until something ends it, because a deeper list
    // belongs *inside* the item above it. Closing each `<li>` as it is written
    // put the nested `<ul>` next to its parent rather than in it — which
    // browsers tolerate and then indent wrongly.
    //
    // `li_open` means: the innermost open list has an item that has not been
    // closed. Going deeper leaves it open on purpose; coming back out closes
    // it, and closing a nested list re-opens the question for its parent,
    // whose own item was never closed either.
    let mut li_open = false;
    /// Close every open list. Anything that is not a list item ends all of
    /// them — a paragraph after a list is not inside it.
    fn close_all_lists(out: &mut String, open: &mut Vec<usize>, li: &mut bool) {
        while !open.is_empty() {
            if *li {
                out.push_str("</li>\n");
            }
            out.push_str("</ul>\n");
            open.pop();
            *li = !open.is_empty();
        }
        *li = false;
    }

    fn close_lists_to(out: &mut String, open: &mut Vec<usize>, li: &mut bool, indent: usize) {
        while open.last().is_some_and(|d| *d > indent) {
            if *li {
                out.push_str("</li>\n");
            }
            out.push_str("</ul>\n");
            open.pop();
            // The item this list was nested inside is still open.
            *li = !open.is_empty();
        }
        if *li && !open.is_empty() {
            out.push_str("</li>\n");
            *li = false;
        }
    }

    while i < lines.len() {
        let raw = &lines[i];
        let t = raw.trim();

        // A fence takes everything to its close, verbatim.
        if let Some(lang) = fence_lang(raw) {
            close_all_lists(&mut out, &mut open_lists, &mut li_open);
            let mut body = String::new();
            i += 1;
            while i < lines.len() && fence_lang(&lines[i]).is_none() {
                body.push_str(&esc(&lines[i]));
                body.push('\n');
                i += 1;
            }
            i += 1; // the closing fence
            let class = if lang.is_empty() {
                String::new()
            } else {
                format!(" class=\"language-{}\"", esc(&lang))
            };
            out.push_str(&format!("<pre><code{class}>{body}</code></pre>\n"));
            continue;
        }

        if t.is_empty() {
            close_all_lists(&mut out, &mut open_lists, &mut li_open);
            i += 1;
            continue;
        }

        if is_rule(raw) {
            close_all_lists(&mut out, &mut open_lists, &mut li_open);
            out.push_str("<hr>\n");
            i += 1;
            continue;
        }

        if let Some((level, text)) = heading(raw) {
            close_all_lists(&mut out, &mut open_lists, &mut li_open);
            // With an anchor, so `[…](#usage)` in the same file has somewhere
            // to land. A README is mostly links to itself and its neighbours,
            // and a preview that cannot follow either opens almost nothing
            // the document points at.
            out.push_str(&format!(
                "<h{level} id=\"{}\">{}</h{level}>\n",
                slug(&text),
                inline_html(&text)
            ));
            i += 1;
            continue;
        }

        // A table: a header row, a separator, then rows until they stop.
        if t.starts_with('|') && i + 1 < lines.len() && is_table_separator(&lines[i + 1]) {
            close_all_lists(&mut out, &mut open_lists, &mut li_open);
            let head = split_cells(raw);
            let aligns: Vec<Align> = split_cells(&lines[i + 1]).iter().map(|c| cell_align(c)).collect();
            let at = |n: usize| match aligns.get(n).copied().unwrap_or_default() {
                Align::Left => "",
                Align::Center => " style=\"text-align:center\"",
                Align::Right => " style=\"text-align:right\"",
            };
            out.push_str("<table>\n<thead><tr>");
            for (n, c) in head.iter().enumerate() {
                out.push_str(&format!("<th{}>{}</th>", at(n), inline_html(c)));
            }
            out.push_str("</tr></thead>\n<tbody>\n");
            i += 2;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                out.push_str("<tr>");
                for (n, c) in split_cells(&lines[i]).iter().enumerate() {
                    out.push_str(&format!("<td{}>{}</td>", at(n), inline_html(c)));
                }
                out.push_str("</tr>\n");
                i += 1;
            }
            out.push_str("</tbody>\n</table>\n");
            continue;
        }

        if t.starts_with("> ") || t == ">" {
            close_all_lists(&mut out, &mut open_lists, &mut li_open);
            let mut body = Vec::new();
            while i < lines.len() {
                let q = lines[i].trim();
                let Some(rest) = q.strip_prefix('>') else { break };
                body.push(rest.trim_start().to_string());
                i += 1;
            }
            out.push_str("<blockquote>\n");
            out.push_str(&to_html(&body));
            out.push_str("</blockquote>\n");
            continue;
        }

        if let Some((_, text, indent)) = list_item(raw) {
            if open_lists.last().is_some_and(|d| indent > *d) {
                // Deeper: the parent's item stays open and this list goes in it.
                open_lists.push(indent);
                out.push_str("<ul>\n");
            } else {
                close_lists_to(&mut out, &mut open_lists, &mut li_open, indent);
                if open_lists.is_empty() {
                    open_lists.push(indent);
                    out.push_str("<ul>\n");
                }
            }
            match task_item(&text) {
                // The line it came from travels with it. A checkbox you can
                // see and not press is a checkbox that makes you go and find
                // the line yourself — and `note::set_check` takes a line
                // number, so this is the whole of what a window needs to
                // make it work.
                Some((done, rest)) => out.push_str(&format!(
                    "<li class=\"task\"><span class=\"box\" data-line=\"{}\">{}</span>{}",
                    i,
                    if done { "☑" } else { "☐" },
                    inline_html(&rest),
                )),
                None => out.push_str(&format!("<li>{}", inline_html(&text))),
            }
            li_open = true;
            i += 1;
            continue;
        }

        // A paragraph: this line and the ones after it that are not something
        // else. Joined with a space, because a hard-wrapped paragraph is one
        // paragraph and a window can wrap it itself.
        close_all_lists(&mut out, &mut open_lists, &mut li_open);
        let mut para = Vec::new();
        while i < lines.len() {
            let p = &lines[i];
            let pt = p.trim();
            if pt.is_empty()
                || heading(p).is_some()
                || is_rule(p)
                || fence_lang(p).is_some()
                || list_item(p).is_some()
                || pt.starts_with('|')
                || pt.starts_with('>')
            {
                break;
            }
            para.push(pt.to_string());
            i += 1;
        }
        out.push_str(&format!("<p>{}</p>\n", inline_html(&para.join(" "))));
    }
    close_all_lists(&mut out, &mut open_lists, &mut li_open);
    out
}

/// Whether `[ ] x` / `[x] x` starts this bullet's text, and which.
///
/// `[X]` counts too: a file written on somebody else's machine is still a file
/// cian is being asked to read.
fn ticked(rest: &str) -> Option<bool> {
    let b = rest.as_bytes();
    if b.len() < 3 || b[0] != b'[' || b[2] != b']' {
        return None;
    }
    match b[1] {
        b' ' => Some(false),
        b'x' | b'X' => Some(true),
        _ => None,
    }
}

/// Tick or untick the checkbox on one line, and hand the whole document back.
///
/// **By line number, not by which checkbox it is.** The preview on screen was
/// drawn from a parse that may be a moment old; counting boxes would tick the
/// wrong one the first time a file gains a task above the one that was
/// pressed — which is why `to_html` writes the line number onto each box. A
/// line that is not a checkbox is left exactly as it was: the screen and the
/// file can disagree, and when they do nothing should happen.
pub fn set_check(text: &str, line: usize, done: bool) -> String {
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let Some(row) = lines.get_mut(line) else { return text.to_string() };
    let indent: String = row.chars().take_while(|c| c.is_whitespace()).collect();
    let t = row.trim_start();
    let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) else {
        return text.to_string();
    };
    if ticked(rest).is_none() {
        return text.to_string();
    }
    let lead = &t[..t.len() - rest.len()];
    *row = format!("{indent}{lead}[{}]{}", if done { "x" } else { " " }, &rest[3..]);
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// How many leading lines the YAML front matter takes, if there is one.
///
/// A `---` on the first line opens one and the next `---` or `...` closes it;
/// anything else is a horizontal rule and the document starts at line zero.
/// Only the *extent* is decided here — this module never reads the fields.
///
/// **Counting it rather than parsing it.** What a front matter *means* is a
/// notes application's question and left with amber; what a Markdown renderer
/// needs is where the prose begins, and an unterminated `---` must not eat the
/// file.
fn front_matter_lines(lines: &[String]) -> usize {
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return 0;
    }
    match lines.iter().skip(1).position(|l| {
        let t = l.trim_end();
        t == "---" || t == "..."
    }) {
        Some(end) => end + 2,
        None => 0,
    }
}

/// A colour span at the very start of `s`: its text, its colour, and how
/// many **characters** it took.
///
/// For the scanner in [`inline`], which walks a line character by character
/// and needs to know whether *this* is the start of one.
///
/// **Characters and not bytes.** `find` counts bytes; the caller counts
/// characters. With Japanese inside the span the two are three times apart,
/// so the scanner jumped past the span *and* the text after it — which showed
/// up as a second coloured word coming out as
/// `e="color:#0E93A8">シアン</span>` in the middle of a sentence.
fn first_color(s: &str) -> Option<(String, String, usize)> {
    if !s.starts_with("<span") {
        return None;
    }
    let gt = s.find('>')?;
    let color = color_of(&s[..=gt])?;
    let after = &s[gt + 1..];
    let end = after.find("</span>")?;
    let bytes = gt + 1 + end + "</span>".len();
    Some((after[..end].to_string(), color, s[..bytes].chars().count()))
}

/// `#rrggbb` out of `<span style="color:#0e93a8">`, if that is what this is.
///
/// Only hex, and only six digits: a name like `red` means a different colour
/// in every renderer. Anything else is left exactly as typed, including a
/// `<span>` carrying some other style — cian is not an HTML renderer and
/// should not pretend to be one.
fn color_of(open: &str) -> Option<String> {
    let lower = open.to_ascii_lowercase();
    let at = lower.find("color:")?;
    let rest = lower[at + "color:".len()..].trim_start();
    let hex = rest.strip_prefix('#')?;
    let digits: String = hex.chars().take(6).collect();
    if digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{digits}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn two_coloured_words_on_one_line_both_survive() {
        // 一度これで壊れた: `find` はバイトを数え、走査は文字を数えていたので、
        // 日本語を挟むと span の**先まで**飛び越えて、次の span の途中から
        // 字が出ていた。
        let line = "ふつうの字と<span style=\"color:#D9822B\">だいだいの字</span>と、\
<span style=\"color:#0E93A8\">シアン</span>。";
        let out = to_html(&lines(line));
        assert!(out.contains("<span style=\"color:#d9822b\">だいだいの字</span>"), "{out}");
        assert!(out.contains("<span style=\"color:#0e93a8\">シアン</span>"), "{out}");
        assert!(!out.contains("e=&quot;color"), "span の途中から字が出ている: {out}");
        assert!(out.contains("と、"), "間の字が食われた: {out}");
    }

    #[test]
    fn front_matter_is_how_a_note_describes_itself_not_something_it_says() {
        let out = to_html(&lines("---\ntitle: 週報\ntags: [仕事]\n---\n\n# 見出し\n"));
        assert!(!out.contains("title:"), "{out}");
        assert!(out.contains("見出し"), "{out}");
        // 先頭の `---` が前書きでないなら、これまで通り区切り線。
        let rule = to_html(&lines("---\n\n本文。\n"));
        assert!(rule.contains("<hr"), "{rule}");
    }

    #[test]
    fn a_colour_survives_the_escaping_and_nothing_else_does() {
        let out = to_html(&lines("ふつうと<span style=\"color:#0E93A8\">シアン</span>。"));
        assert!(out.contains("<span style=\"color:#0e93a8\">シアン</span>"), "{out}");
        // 他の HTML は、これまで通り字にする。
        let out = to_html(&lines("<span onclick=\"x\">あ</span>"));
        assert!(out.contains("&lt;span"), "{out}");
        assert!(!out.contains("onclick=\"x\""), "{out}");
        // 色でない span も字のまま。
        let out = to_html(&lines("<span class=\"x\">あ</span>"));
        assert!(out.contains("&lt;span"), "{out}");
    }
    #[test]
    fn pressing_a_task_changes_that_line_and_nothing_else() {
        let text = "- [ ] 牛乳\n- [ ] 珈琲\n";
        let on = set_check(text, 1, true);
        assert_eq!(on, "- [ ] 牛乳\n- [x] 珈琲\n");
        assert_eq!(set_check(&on, 1, false), text);
        // 行がずれていた・そこは箇条書きだった、のときは何もしない ──
        // 画面とファイルが食い違っているのに書き込むのが一番悪い。
        assert_eq!(set_check(text, 9, true), text);
        assert_eq!(set_check("- ふつう\n", 0, true), "- ふつう\n");
        // 字下げは字下げのまま
        assert_eq!(set_check("  - [ ] 中\n", 0, true), "  - [x] 中\n");
    }

    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn inline_marks_are_read_once() {
        assert_eq!(
            inline("a **b** c `d` [e](http://x) ~~f~~"),
            vec![
                Inline::Text("a ".into()),
                Inline::Bold("b".into()),
                Inline::Text(" c ".into()),
                Inline::Code("d".into()),
                Inline::Text(" ".into()),
                Inline::Link { text: "e".into(), url: "http://x".into() },
                Inline::Text(" ".into()),
                Inline::Strike("f".into()),
            ]
        );
    }

    #[test]
    fn an_unclosed_mark_stays_text() {
        // A stray asterisk is an asterisk. Treating it as the start of
        // emphasis that never ends would swallow the rest of the line.
        assert_eq!(inline("2 * 3 = 6"), vec![Inline::Text("2 * 3 = 6".into())]);
    }

    #[test]
    fn code_wins_over_emphasis() {
        // Inside backticks a `*` is an asterisk, which is the whole reason to
        // write `*` in backticks.
        assert_eq!(inline("`a*b*c`"), vec![Inline::Code("a*b*c".into())]);
    }

    #[test]
    fn html_in_the_source_is_shown_not_run() {
        let html = to_html(&lines("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
    }

    #[test]
    fn a_javascript_link_is_not_a_link() {
        // The oldest trick there is, and a README is a file from somewhere.
        // The text still shows; it just does not go anywhere.
        let html = to_html(&lines("[click](javascript:alert(1))"));
        assert!(html.contains("click"), "{html}");
        assert!(!html.contains("<a "), "{html}");
    }

    #[test]
    fn relative_links_still_work() {
        let html = to_html(&lines("[readme](docs/README.md)"));
        assert!(html.contains(r#"<a href="docs/README.md">readme</a>"#), "{html}");
    }

    #[test]
    fn a_table_keeps_its_alignment() {
        let html = to_html(&lines("| a | b |\n| :- | --: |\n| 1 | 2 |"));
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains(r#"<th style="text-align:right">b</th>"#), "{html}");
    }

    #[test]
    fn nested_lists_close_in_order() {
        let html = to_html(&lines("- one\n  - deep\n- two"));
        assert_eq!(html.matches("<ul>").count(), 2, "{html}");
        assert_eq!(html.matches("</ul>").count(), 2, "{html}");
        assert_eq!(html.matches("<li>").count(), 3, "{html}");
        assert_eq!(html.matches("</li>").count(), 3, "{html}");
    }

    #[test]
    fn a_nested_list_sits_inside_its_parent_item() {
        // `<li>one</li><ul>…</ul>` is what the first version produced:
        // tolerated by browsers and then indented as though the nesting were
        // not there.
        let html = to_html(&lines("- one\n  - deep"));
        let li = html.find("<li>one").unwrap();
        let ul = html[li..].find("<ul>").unwrap();
        let close = html[li..].find("</li>").unwrap();
        assert!(ul < close, "nested <ul> must come before its parent's </li>\n{html}");
    }

    #[test]
    fn a_fence_is_verbatim() {
        let html = to_html(&lines("```rust\nlet x = *p;\n```"));
        assert!(html.contains(r#"<code class="language-rust">"#), "{html}");
        assert!(html.contains("let x = *p;"), "{html}");
        // Not turned into emphasis on the way past.
        assert!(!html.contains("<em>"), "{html}");
    }

    #[test]
    fn a_list_closes_before_what_follows_it() {
        // The first version dropped the record of the outermost list without
        // emitting its `</ul>`, so the paragraph after a list arrived inside
        // it — indented for ever, in every document that has a list.
        let html = to_html(&lines("- one\n  - deep\n- two\n\npara"));
        assert_eq!(html.matches("<ul>").count(), html.matches("</ul>").count(), "{html}");
        assert!(html.find("</ul>").unwrap() < html.find("<p>para").unwrap(), "{html}");
    }

    #[test]
    fn a_hard_wrapped_paragraph_is_one_paragraph() {
        let html = to_html(&lines("one\ntwo\n\nthree"));
        assert_eq!(html.matches("<p>").count(), 2, "{html}");
        assert!(html.contains("<p>one two</p>"), "{html}");
    }

    #[test]
    fn task_boxes_are_marked_and_carry_the_line_they_came_from() {
        let html = to_html(&lines("- [x] done\n- [ ] not"));
        assert!(html.contains("☑"), "{html}");
        assert!(html.contains("☐"), "{html}");
        // 押せるようにするのに要るのはこれだけ ── `note::set_check` は
        // 行番号を取る。前書きの分もちゃんと数える。
        assert!(html.contains("data-line=\"0\""), "{html}");
        assert!(html.contains("data-line=\"1\""), "{html}");
        let with_front = to_html(&lines("---\ntitle: x\n---\n\n- [ ] a\n"));
        assert!(with_front.contains("data-line=\"4\""), "{with_front}");
    }

    #[test]
    fn headings_get_an_anchor_to_link_to() {
        assert_eq!(slug("Usage"), "usage");
        assert_eq!(slug("Getting started!"), "getting-started");
        // Two spaces are two hyphens, and so is a dropped dash between them.
        // GitHub's own slugger does this, and a README's links were written
        // against GitHub's.
        assert_eq!(slug("v1.2 — notes (draft)"), "v12--notes-draft");
        // Japanese is kept: stripping it would collapse every heading in a
        // Japanese document to the same empty anchor.
        assert_eq!(slug("使い方"), "使い方");
        assert_eq!(slug("  trailing  "), "trailing", "the ends are trimmed first");
        assert_eq!(slug("###"), "");

        let html = to_html(&["# 使い方".to_string(), "## Getting started".to_string()]);
        assert!(html.contains("<h1 id=\"使い方\">"), "{html}");
        assert!(html.contains("<h2 id=\"getting-started\">"), "{html}");
    }
}
