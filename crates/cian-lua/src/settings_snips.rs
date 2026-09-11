//! 設定画面からスニペットを足す・直す・消す。
//!
//! スニペットは「シェルへ送る一行」に名前を付けたもの（`Ctrl+Shift+Enter` /
//! `:snip` で選ぶ）。`cian.snippets{ { name = …, cmd = … }, … }` と書く。
//!
//! **同梱の見本には、これが一行も無い。** マニュアルは名前を挙げているのに
//! `examples/init.lua` に例が無く、書き方を知る道が「ソースを読む」しか
//! なかった ── 設定画面がその穴を埋める側になる。
//!
//! ## 置き場は `init.lua`
//!
//! SSH とキー割当には分割ファイル（`ssh.lua` / `keymap.lua`）があるが、
//! スニペットには**無い**（`SPLIT_CONFIG_FILES` は2つだけ）。増やすのは
//! cian 側の話で、設定画面が勝手に決めることではないので、`init.lua` に書く。
//!
//! ## `enter` と `confirm` は、既定と違うときだけ書く
//!
//! `enter` は既定 `true`（送ってすぐ走る）、`confirm` は既定 `false`。
//! 既定と同じ値をいちいち書くと、`init.lua` が「読む価値のない行」で太る ──
//! ただし**書いてあったものは消さない**（crmaine の教訓: 「書いてある既定値」は
//! 明示であって、既定と同じだからと落とすと機能が勝手に戻る）。

use std::path::PathBuf;

use crate::settings_edit::live_block;
use crate::settings_list::{entries_in, field_of, quote};

/// スニペット1つ。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snip {
    pub name: String,
    pub cmd: String,
    /// 送ってすぐ走らせるか。既定 `true`。
    pub enter: bool,
    /// 送る前に訊くか。既定 `false`。
    pub confirm: bool,
    /// もとの行に `enter` / `confirm` が**書いてあったか**。書いてあったものは
    /// 既定と同じでも書き戻す。
    pub wrote_enter: bool,
    pub wrote_confirm: bool,
}

/// `cian.snippets{ … }` の中身の行範囲。
fn snips_span(lines: &[String]) -> Option<(usize, usize)> {
    live_block(lines, "snippets")
}

fn snip_entries(lines: &[String]) -> Vec<(usize, usize)> {
    match snips_span(lines) {
        Some((start, end)) => entries_in(lines, start, end),
        None => Vec::new(),
    }
}

/// 書いてあるスニペットを、書いてある順に。
pub fn snips_in(text: &str) -> Vec<Snip> {
    let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    snip_entries(&lines)
        .into_iter()
        .map(|(from, to)| {
            let enter = field_of(&lines, from, to, "enter");
            let confirm = field_of(&lines, from, to, "confirm");
            Snip {
                name: field_of(&lines, from, to, "name"),
                cmd: field_of(&lines, from, to, "cmd"),
                // 書いていなければ既定。
                enter: enter.is_empty() || enter == "true",
                confirm: confirm == "true",
                wrote_enter: !enter.is_empty(),
                wrote_confirm: !confirm.is_empty(),
            }
        })
        .filter(|s| !s.cmd.is_empty())
        .collect()
}

/// スニペット1つを、1行の Lua に。
fn render(s: &Snip, indent: &str) -> String {
    let mut parts = vec![format!("name = {}", quote(&s.name)), format!("cmd = {}", quote(&s.cmd))];
    // 既定と違うときと、もとから書いてあったときだけ。
    if !s.enter || s.wrote_enter {
        parts.push(format!("enter = {}", s.enter));
    }
    if s.confirm || s.wrote_confirm {
        parts.push(format!("confirm = {}", s.confirm));
    }
    format!("{indent}{{ {} }},", parts.join(", "))
}

/// `name` のスニペットを、足す・直す・消す。`row` が `None` なら消す。
pub fn set_snip_in(text: &str, name: &str, row: Option<&Snip>) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> =
        text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    let trailing = lines.last().map(|l| l.is_empty()).unwrap_or(false);
    if trailing {
        lines.pop();
    }

    let found = snip_entries(&lines)
        .into_iter()
        .find(|&(from, to)| field_of(&lines, from, to, "name") == name);

    match (row, found) {
        (Some(r), Some((from, to))) => {
            let indent =
                lines[from][..lines[from].len() - lines[from].trim_start().len()].to_string();
            lines.splice(from..=to, [render(r, &indent)]);
        }
        (None, Some((from, to))) => {
            lines.drain(from..=to);
        }
        (Some(r), None) => match snips_span(&lines) {
            Some((start, end)) => {
                let indent = lines
                    .get(start + 1)
                    .map(|l| l[..l.len() - l.trim_start().len()].to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "  ".to_string());
                lines.insert(end, render(r, &indent));
            }
            None => {
                if !lines.iter().any(|l| l.trim() == crate::settings_edit::ADDED_HEAD) {
                    if lines.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                        lines.push(String::new());
                    }
                    lines.push(crate::settings_edit::ADDED_HEAD.to_string());
                }
                lines.push("cian.snippets {".to_string());
                lines.push(render(r, "  "));
                lines.push("}".to_string());
            }
        },
        (None, None) => {}
    }

    let mut out = lines.join(nl);
    if trailing {
        out.push_str(nl);
    }
    out
}

/// 読む場所と書く場所。**分割ファイルは無い**ので、どちらも `init.lua`。
pub fn snip_files() -> (Option<PathBuf>, Option<PathBuf>) {
    let read = crate::config_read_path("init.lua").filter(|p| {
        std::fs::read_to_string(p)
            .map(|t| {
                let lines: Vec<String> = t.lines().map(|l| l.to_string()).collect();
                live_block(&lines, "snippets").is_some()
            })
            .unwrap_or(false)
    });
    (read, crate::config_write_path("init.lua"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SN: &str = "\
-- 見本のつもり
cian.snippets {
  { name = \"ログ\", cmd = \"tail -f /var/log/messages\" },
  { name = \"再起動\", cmd = \"systemctl restart app\", confirm = true },
  { name = \"下書き\", cmd = \"docker ps -a\", enter = false },
}
";

    #[test]
    fn reads_what_is_written_including_the_defaults() {
        let got = snips_in(SN);
        assert_eq!(got.len(), 3, "{got:#?}");
        assert_eq!(got[0].name, "ログ");
        assert_eq!(got[0].cmd, "tail -f /var/log/messages");
        // 書いていなければ既定（すぐ走る、訊かない）。
        assert!(got[0].enter && !got[0].confirm);
        assert!(!got[0].wrote_enter && !got[0].wrote_confirm);
        assert!(got[1].confirm && got[1].wrote_confirm);
        assert!(!got[2].enter && got[2].wrote_enter);
    }

    /// **書いてあったものは、既定と同じでも書き戻す。**
    ///
    /// crmaine が実機で踏んだ形 ── 「書いてある既定値」は明示であって、
    /// 既定と同じだからと落とすと機能が勝手に戻る。
    #[test]
    fn a_written_default_is_kept_when_the_row_is_edited() {
        let mut row = snips_in(SN)[2].clone();
        row.enter = true; // 既定と同じ値に戻した
        let out = set_snip_in(SN, "下書き", Some(&row));
        assert!(out.contains("enter = true"), "書いてあったので書き戻す: {out}");
        // もとから書いていない行は、既定のまま静かに。
        let mut plain = snips_in(SN)[0].clone();
        plain.cmd = "tail -f /var/log/syslog".into();
        let out2 = set_snip_in(SN, "ログ", Some(&plain));
        assert!(out2.contains("{ name = \"ログ\", cmd = \"tail -f /var/log/syslog\" },"), "{out2}");
    }

    #[test]
    fn a_new_snippet_lands_inside_the_block() {
        let row = Snip { name: "空き".into(), cmd: "df -h".into(), enter: true, ..Default::default() };
        let out = set_snip_in(SN, "空き", Some(&row));
        assert_eq!(snips_in(&out).len(), 4, "{out}");
        assert!(out.contains("  { name = \"空き\", cmd = \"df -h\" },\n}"), "{out}");
        assert_eq!(crate::settings_edit::syntax_error(&out), None, "{out}");
    }

    #[test]
    fn removing_takes_the_whole_entry() {
        let out = set_snip_in(SN, "再起動", None);
        let got = snips_in(&out);
        assert_eq!(got.len(), 2, "{out}");
        assert!(!out.contains("systemctl"), "{out}");
        assert!(out.contains("-- 見本のつもり"), "{out}");
    }

    #[test]
    fn the_first_snippet_writes_a_whole_block() {
        let row = Snip { name: "空き".into(), cmd: "df -h".into(), enter: true, ..Default::default() };
        let out = set_snip_in("-- 何も無い\n", "空き", Some(&row));
        assert!(out.contains(crate::settings_edit::ADDED_HEAD), "{out}");
        assert!(out.contains("cian.snippets {\n  { name = \"空き\", cmd = \"df -h\" },\n}"), "{out}");
        assert_eq!(crate::settings_edit::syntax_error(&out), None, "{out}");
        assert_eq!(snips_in(&out).len(), 1);
    }

    /// **訊くやつを、訊かないやつに変えない。**
    #[test]
    fn confirm_survives_a_round_trip() {
        let mut out = SN.to_string();
        for s in snips_in(SN) {
            out = set_snip_in(&out, &s.name, Some(&s));
        }
        let got = snips_in(&out);
        assert_eq!(got.len(), 3, "{out}");
        assert!(got[1].confirm, "{out}");
        assert!(!got[2].enter, "{out}");
        assert_eq!(crate::settings_edit::syntax_error(&out), None, "{out}");
    }

    /// シェルへ送る一行に、引用符もバックスラッシュも入る。
    #[test]
    fn a_command_with_quotes_survives() {
        let row = Snip {
            name: "探す".into(),
            cmd: r#"grep -r "foo\bar" /var"#.into(),
            enter: true,
            ..Default::default()
        };
        let out = set_snip_in(SN, "探す", Some(&row));
        assert_eq!(crate::settings_edit::syntax_error(&out), None, "{out}");
        assert_eq!(snips_in(&out).last().unwrap().cmd, r#"grep -r "foo\bar" /var"#);
    }

    #[test]
    fn crlf_stays_crlf() {
        let text = SN.replace('\n', "\r\n");
        let mut row = snips_in(&text)[0].clone();
        row.cmd = "tail -n 100 /var/log/messages".into();
        let out = set_snip_in(&text, "ログ", Some(&row));
        assert!(!out.contains("\n\n"), "{out:?}");
        assert_eq!(out.matches("\r\n").count(), text.matches("\r\n").count());
    }
}
