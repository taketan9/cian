//! `{ … }, { … }` が並ぶところを読む ── SSH ホストとスニペットで同じ形。
//!
//! **括ったのは「読む」ところだけ。** 書き戻す形（どの項目をどう並べるか）は
//! 二つで違うので、そちらは括っていない ── 「似ている」だけで括ると、片方
//! だけ将来変わったときに分けるより高くつく。ここで括ったのは**壊れ方が同じ**
//! ところ: 釣り合った括弧を数える、コメントの中を数えない、行末の注を値に
//! 混ぜない。三つとも一度ずつ間違えている。

use crate::settings_edit::braces;

/// `start` から `end` のあいだに並ぶ `{ … }` の、それぞれの行範囲。
///
/// `start` は開き括弧のある行（`hosts = {` や `cian.snippets{`）で、その行に
/// 最初の `{` があるので**次の行から**見る。
pub(crate) fn entries_in(lines: &[String], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        i += 1;
        let t = lines[i].trim_start();
        if t.starts_with("--") || !t.starts_with('{') {
            continue;
        }
        let mut depth = 0i32;
        for (j, l) in lines.iter().enumerate().take(end).skip(i) {
            depth += braces(l);
            if depth <= 0 {
                out.push((i, j));
                i = j;
                break;
            }
        }
    }
    out
}
/// その範囲の `key = 値` を取り出す。`users` は入れ子なので字面のまま。
pub(crate) fn field_of(lines: &[String], from: usize, to: usize, key: &str) -> String {
    let body = lines[from..=to].join("\n");
    let head = format!("{key} =");
    let mut at = 0;
    while let Some(i) = body[at..].find(&head) {
        let i = at + i;
        // 直前が英字なら別の名前（`key_pass` の中の `key`）。
        let before = body[..i].chars().next_back();
        if before.map(|c| c.is_alphanumeric() || c == '_').unwrap_or(false) {
            at = i + head.len();
            continue;
        }
        // コメントの中は数えない。
        //
        // **`entries` の同じ判定と二重になっている。** 片方を外しても検査は
        // 黙る（もう片方が拾う）ので、変異テストで確かめるときは**両方**を
        // 外すこと ── 片方だけ外して「検査が効いていない」と読みかけた。
        // 片方だけ消すのも駄目で、そのときは黙って通る。
        let line_start = body[..i].rfind('\n').map(|b| b + 1).unwrap_or(0);
        if body[line_start..i].trim_start().starts_with("--") {
            at = i + head.len();
            continue;
        }
        let rest = body[i + head.len()..].trim_start();
        return value_at(rest);
    }
    String::new()
}
/// 値ひとつ。文字列なら引用符を外し、表なら釣り合う `}` まで字面のまま。
fn value_at(rest: &str) -> String {
    let chars: Vec<char> = rest.chars().collect();
    if chars.first() == Some(&'"') || chars.first() == Some(&'\'') {
        let q = chars[0];
        let mut out = String::new();
        let mut i = 1;
        while i < chars.len() && chars[i] != q {
            // **逃がした文字を、戻す。** `\\n` を「`n` という字」として読んで
            // いたので、複数行のスニペットを書いて読み直すと `cd /var/logntail`
            // になった ── 書く側だけ直しても、往復で壊れる。
            if chars[i] == '\\' && i + 1 < chars.len() {
                i += 1;
                out.push(match chars[i] {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
                i += 1;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
        return out;
    }
    if chars.first() == Some(&'{') {
        let mut depth = 0i32;
        let mut out = String::new();
        for (i, c) in chars.iter().enumerate() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            out.push(*c);
            if depth == 0 && i > 0 {
                break;
            }
        }
        return out;
    }
    // 数やそのほか。`,`・行末・**閉じ括弧**まで ── `port = 2222 }` を
    // `2222 }` と読んで、そのまま書き戻したことがある。
    rest.split([',', '\n', '}']).next().unwrap_or("").trim().to_string()
}
/// Lua の文字列に入れられる形に。**行をまたげないことを忘れない。**
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            // **改行を生のまま入れない。** Lua の `"…"` は行をまたげないので、
            // 生の改行を書くと `unfinished string` で落ちる ── 複数行の
            // スニペット（`cd /var/log` してから `tail -f`）や、複数行の
            // サーバのメモが、そのまま壊れた設定になる。
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}
