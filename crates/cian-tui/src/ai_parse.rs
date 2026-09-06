//! Parsing and sanitising of AI replies: pulling a command, a commit message,
//! or a bounded payload out of free-form model output. Pure functions.
//!
//! There used to be a second half here — JSON plans for junk detection, folder
//! restructuring, bulk renaming and semantic search, each validated back
//! against the caller's real paths so a hallucinated name matched nothing. The
//! validation was sound; the features were not worth their keep, and they went
//! with them.

/// Clean an AI-generated shell command: drop ``` fences and surrounding
/// backticks, and take the first non-empty line (models sometimes add prose).
pub(crate) fn clean_ai_command(raw: &str) -> String {
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("```") {
            continue;
        }
        return t.trim_matches('`').trim().to_string();
    }
    String::new()
}

/// Strip code fences and leading/trailing blank lines from an AI-drafted commit
/// message. Models sometimes wrap the whole thing in ```; the content inside is
/// what we want.
pub(crate) fn clean_ai_commit_message(raw: &str) -> String {
    let mut lines: Vec<&str> = raw.lines().collect();
    // Drop a leading ```… fence and its matching close, if present.
    if lines.first().map(|l| l.trim_start().starts_with("```")).unwrap_or(false) {
        lines.remove(0);
        if let Some(pos) = lines.iter().rposition(|l| l.trim() == "```") {
            lines.truncate(pos);
        }
    }
    let text = lines.join("\n");
    text.trim().to_string()
}

/// Cap a diff at roughly `max_bytes` on a line boundary so the AI request stays
/// within budget, appending a marker when truncated.
pub(crate) fn truncate_diff_for_ai(diff: &str, max_bytes: usize) -> String {
    if diff.len() <= max_bytes {
        return diff.to_string();
    }
    let mut out = String::with_capacity(max_bytes + 64);
    for line in diff.lines() {
        if out.len() + line.len() + 1 > max_bytes {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("\n[diff truncated — summarise from what is shown above]\n");
    out
}

/// Cap arbitrary text at roughly `max_bytes` on a line boundary (a char
/// boundary if a single line is longer), appending a marker when truncated.
pub(crate) fn truncate_text_for_ai(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut out = String::with_capacity(max_bytes + 64);
    for line in text.lines() {
        if out.len() + line.len() + 1 > max_bytes {
            // A single over-long line: take a char-boundary prefix of it.
            if out.is_empty() {
                let mut end = max_bytes.min(line.len());
                while end > 0 && !line.is_char_boundary(end) {
                    end -= 1;
                }
                out.push_str(&line[..end]);
            }
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("\n[truncated — summarise from what is shown above]\n");
    out
}
