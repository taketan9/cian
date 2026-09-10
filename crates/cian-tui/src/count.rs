//! `:count` — cian's built-in file/step counter (kazoechao). Tallies files,
//! lines and *steps* (source lines) under the target, on a worker thread so a
//! big tree never freezes the UI, and shows the breakdown in a notice. What
//! counts as a step is configured in `count.lua` — see [`cian_core::count`].

use std::path::PathBuf;

use crate::{pad_to, tr, App, Popup};

/// Load `count.lua` (portable-aware); fall back to the built-in defaults if it
/// is absent or unparseable.
pub(crate) fn load_count_opts() -> cian_core::count::Options {
    cian_lua::config_read_path("count.lua")
        .filter(|p| p.exists())
        .and_then(|p| cian_lua::count::load(&p).ok())
        .unwrap_or_default()
}

impl App {
    /// `:count` — tally the target (marked entries, else the active pane's whole
    /// directory) on a worker thread.
    pub(crate) fn start_count(&mut self) {
        if self.count_job.is_some() {
            self.message = Some(tr(self.lang, "already counting…", "カウント中です…").into());
            return;
        }
        let targets = self.count_targets();
        if targets.is_empty() {
            self.message = Some(tr(self.lang, "nothing to count", "カウント対象がありません").into());
            return;
        }
        let opts = self.count_opts.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(cian_core::count::count(&targets, &opts));
        });
        self.count_job = Some(rx);
        self.message = Some(tr(self.lang, "counting…", "カウント中…").into());
    }

    /// What to count: the marked entries, or — with nothing marked — the entry
    /// under the cursor (a directory is walked recursively). `target_paths`
    /// gives exactly that (marks, else the cursor, never `..`).
    fn count_targets(&self) -> Vec<PathBuf> {
        self.target_paths()
    }

    /// Install a finished count as a notice. Returns true if a report landed.
    pub(crate) fn poll_count(&mut self) -> bool {
        let Some(rx) = &self.count_job else { return false };
        match rx.try_recv() {
            Ok(report) => {
                self.count_job = None;
                let lines = self.format_count(&report);
                self.open_popup(Popup::Notice { lines });
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.count_job = None;
                self.message = Some(tr(self.lang, "count failed", "カウントに失敗").into());
                true
            }
        }
    }

    fn format_count(&self, r: &cian_core::count::Report) -> Vec<String> {
        let o = &self.count_opts;
        let lang = self.lang;
        let mut lines = Vec::new();
        lines.push(tr(lang, "file / step count", "ファイル／ステップ数").to_string());

        // What "steps" means under the current options, said plainly.
        let rule = match (o.count_blank, o.count_comments) {
            (false, false) => tr(lang, "steps = code lines (blank & comment excluded)", "ステップ = コード行（空行・コメント除外）"),
            (true, false) => tr(lang, "steps = code + blank lines", "ステップ = コード + 空行"),
            (false, true) => tr(lang, "steps = code + comment lines", "ステップ = コード + コメント行"),
            (true, true) => tr(lang, "steps = all physical lines", "ステップ = 全物理行"),
        };
        lines.push(rule.to_string());
        if !o.extensions.is_empty() {
            lines.push(format!("{}: {}", tr(lang, "extensions", "対象拡張子"), o.extensions.join(", ")));
        }
        lines.push(String::new());

        let t = &r.total;
        lines.push(format!("  {}{:>10}", pad_to(tr(lang, "files", "ファイル"), 9), group(t.files)));
        lines.push(format!("  {}{:>10}", pad_to(tr(lang, "lines", "行"), 9), group(t.total)));
        lines.push(format!("  {}{:>10}", pad_to(tr(lang, "blank", "空行"), 9), group(t.blank)));
        lines.push(format!("  {}{:>10}", pad_to(tr(lang, "comment", "コメント"), 9), group(t.comment)));
        lines.push(format!("  {}{:>10}", pad_to(tr(lang, "STEPS", "ステップ"), 9), group(t.steps(o))));

        if !r.by_ext.is_empty() {
            lines.push(String::new());
            lines.push(tr(lang, "by extension:", "拡張子別:").to_string());
            for (ext, c) in r.by_ext.iter().take(20) {
                let name = if ext.is_empty() { "(none)" } else { ext.as_str() };
                lines.push(format!(
                    "  {}{:>6} {}{:>9} {}",
                    pad_to(name, 10),
                    group(c.files),
                    tr(lang, "files", "件"),
                    group(c.steps(o)),
                    tr(lang, "steps", "step"),
                ));
            }
            if r.by_ext.len() > 20 {
                lines.push(format!("  … +{} more", r.by_ext.len() - 20));
            }
        }
        if r.truncated {
            lines.push(String::new());
            lines.push(tr(lang, "(stopped at the file cap)", "（ファイル数上限で打ち切り）").to_string());
        }
        lines
    }
}

/// Group a number with thousands separators: 1234567 → "1,234,567".
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands() {
        assert_eq!(group(0), "0");
        assert_eq!(group(42), "42");
        assert_eq!(group(1234), "1,234");
        assert_eq!(group(1234567), "1,234,567");
    }
}
