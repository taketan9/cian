//! 設定画面が `init.lua` を書き換えるための、**外科的な**編集。
//!
//! 設定画面は「読んで、表を描いて、保存する」ものだが、**保存で丸ごと書き
//! 直してはいけない**。`init.lua` は素の Lua で、その大半は人が読むための
//! 注釈だ（同梱の見本は 406 行のうち設定行は 60 行ほどで、残りは全部説明）。
//! 値から組み直せば、その説明も、`if` も、書き手が付けた並び順も消える。
//!
//! crmaine の設定画面が同じところで一度転けている（`gui/settings.js` の
//! `save`）── 画面が知っている項目だけで JSON を組み直して上書きした結果、
//! 手で書き足した項目が消え、「書いてある既定値」（`useDb = false` は
//! 「設定があっても使わない」という**明示**）まで既定と同じだからと落とされ、
//! 機能が勝手に戻った。読めないファイルを既定値で塗り潰して 5,399 バイトが
//! 64 バイトになった日もある。**土台は元のファイル。画面が触る行だけ差し替える。**
//!
//! ## 見本ファイルの形が、そのまま道になる
//!
//! 同梱の `examples/init.lua` は**全部コメントアウトされている**。
//! 「設定する」とは、多くの場合「行頭の `-- ` を外して値を書く」ことだ。
//! だからこの編集は4つの道しか持たない:
//!
//! 1. **有効な行がある** → 値だけ差し替える（行末の注釈は残す）
//! 2. **コメントアウトされた同名の行がある** → それを有効にして値を入れる
//! 3. **どちらも無い** → 末尾の「設定画面が書いた」節に足す
//! 4. **消す** → 行を削らず、**コメントに戻す**（説明が消えるから）
//!
//! ## 同じ名前が何度も出てきたら
//!
//! Lua は上から実行するので、**最後に有効な行が勝つ**。見本にも
//! `cian.set_option("shell", …)` が3行並んでいる（Windows / pwsh / zsh）。
//! だから書き換えるのは**最後の有効な行**で、ほかは触らない ── 触ると、
//! 書き手が「こっちの機械ではこう」と並べておいた列が崩れる。
//!
//! ## 改行は元のまま
//!
//! `\r\n` のファイルを `\n` で書き戻すと、git が全行を変更として出す。
//! 会社の Windows で使うものなので、ここは黙って揃えない。

use crate::config_read_path;

/// `cian.set_option("name", …)` の行を探すときの、1行ぶんの見立て。
#[derive(Debug, PartialEq)]
struct Seen {
    /// 何行目か（0 始まり）。
    at: usize,
    /// 行頭が `--` で殺されているか。
    off: bool,
    /// `cian.set_option(` の前にある空白（インデントを保つ）。
    indent: String,
    /// 値の後ろに続くもの（`)` と、行末の注釈）。
    tail: String,
}

/// その行が `cian.set_option("name", …)` なら、見立てを返す。
///
/// **賢くしない。** Lua の構文解析はしない ── ここが当てにならない大きさに
/// 育つと、設定ファイルを壊す道具になる。見るのは「行頭（`--` を除く）が
/// `cian.set_option("name"` で始まるか」だけで、それ以外の書き方
/// （変数に入れてから呼ぶ、`for` で回す）は**見つからないほうが正しい** ──
/// 見つからなければ末尾に足すので、書き手の書いた行は無傷で残る。
fn look(line: &str, at: usize, name: &str) -> Option<Seen> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = line[..indent_len].to_string();
    let rest = &line[indent_len..];
    let (off, rest) = match rest.strip_prefix("--") {
        Some(r) => (true, r.trim_start()),
        None => (false, rest),
    };
    let head = format!("cian.set_option(\"{name}\"");
    if !rest.starts_with(&head) {
        return None;
    }
    // 値の終わりは、釣り合った閉じ括弧。文字列の中の `)` は数えない。
    let after = &rest[head.len()..];
    let comma = after.find(',')?;
    let mut depth = 1i32;
    let bytes: Vec<char> = after.chars().collect();
    let mut i = comma;
    let mut end = None;
    while i < bytes.len() {
        match bytes[i] {
            '"' | '\'' => {
                let q = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    if bytes[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            '(' | '{' => depth += 1,
            ')' | '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let end = end?;
    Some(Seen { at, off, indent, tail: bytes[end..].iter().collect() })
}

/// 見出し。**末尾に足した行が、どこから来たのか分かるように。**
///
/// 手で書いた行と機械が書いた行が混ざると、次に人がこのファイルを読むとき
/// 「自分が書いたのか」が分からなくなる ── この家では、頼まれて足した行を
/// 後のパスで自分の判断だと思って消した事故がある（`gui/REQUESTS.ja.md` の
/// 1行目）。**出どころを書いておけば、消す前に訊ける。**
pub const ADDED_HEAD: &str = "-- ここから下は設定画面が書きます（手で直しても構いません）";

/// `text` の `cian.set_option("name", …)` を `value` にする。
///
/// `value` が `None` なら**コメントに戻す**（行は消さない）。`value` は
/// **Lua の字面**をそのまま渡す ── `"8"`、`"true"`、`"\"nvim\""`、
/// `"{ \"bak\", \"dmp\" }"`。引用符を付けるのは呼び出し側の仕事で、
/// ここで型を推し量ると `"true"` という文字列が真偽値になる。
pub fn set_option_in(text: &str, name: &str, value: Option<&str>) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    // 末尾の改行で割れた空行は、書き戻すときに復元する。
    let trailing = lines.last().map(|l| l.is_empty()).unwrap_or(false);
    if trailing {
        lines.pop();
    }

    let seen: Vec<Seen> =
        lines.iter().enumerate().filter_map(|(i, l)| look(l, i, name)).collect();
    let live = seen.iter().rev().find(|s| !s.off);

    let write = |s: &Seen, v: &str| {
        format!("{}cian.set_option(\"{}\", {}{}", s.indent, name, v, s.tail)
    };

    match (value, live) {
        // ① 有効な行がある → 値だけ差し替える
        (Some(v), Some(s)) => lines[s.at] = write(s, v),
        // ② コメントアウトされた同名の行を、有効にして使う
        (Some(v), None) => match seen.first() {
            Some(s) => lines[s.at] = write(s, v),
            // ③ どこにも無い → 末尾の節へ
            None => {
                if !lines.iter().any(|l| l.trim() == ADDED_HEAD) {
                    if lines.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                        lines.push(String::new());
                    }
                    lines.push(ADDED_HEAD.to_string());
                }
                lines.push(format!("cian.set_option(\"{name}\", {v})"));
            }
        },
        // ④ 消す = コメントに戻す。**行は削らない**
        (None, Some(s)) => lines[s.at] = format!("{}-- {}", s.indent, lines[s.at].trim_start()),
        (None, None) => {}
    }

    let mut out = lines.join(nl);
    if trailing {
        out.push_str(nl);
    }
    out
}

/// `init.lua` に**実際に書いてある**値（Lua の字面のまま）。無ければ `None`。
///
/// 設定画面が「設定済み」を数えるのはこれ。crmaine は読み込んだ**あと**の値で
/// 数えて、何も書いていないのに「1件設定済み」と出した ── 計算で埋めた既定が
/// 混じっていたからで、数が嘘をつくと、何を設定したのか分からなくなる。
pub fn get_option_in(text: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let seen: Vec<Seen> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| look(l, i, name))
        .filter(|s| !s.off)
        .collect();
    let last = seen.last()?;
    let line = lines[last.at];
    let head = line.find(&format!("\"{name}\""))? + name.len() + 2;
    let rest = &line[head..];
    let comma = rest.find(',')? + 1;
    let value = &rest[comma..rest.len() - last.tail.len()];
    Some(value.trim().to_string())
}

/// ブロックの中に**実際に書いてある**値。無ければ `None`。
pub fn get_field_in(text: &str, call: &str, key: &str) -> Option<String> {
    let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let (from, to) = live_block(&lines, call)?;
    if from == to {
        let open = lines[from].find('{')?;
        let close = lines[from].rfind('}')?;
        return lines[from][open + 1..close].split(',').find_map(|p| {
            let p = p.trim();
            p.strip_prefix(key)?.trim_start().strip_prefix('=').map(|v| v.trim().to_string())
        });
    }
    for line in lines.iter().take(to).skip(from + 1) {
        let t = line.trim();
        if t.starts_with("--") {
            continue;
        }
        let Some(rest) = t.strip_prefix(key) else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('=') else { continue };
        let value = rest.trim();
        // 行末の `,` と `-- 注` を落とす。
        let value = match tail_note(t).trim().is_empty() {
            true => value,
            false => &value[..value.len() - tail_note(t).trim().len()],
        };
        return Some(value.trim().trim_end_matches(',').trim().to_string());
    }
    None
}

/// `cian.ai { … }` のような**平たい表**の、1項目を書き換える。
///
/// `set_option_in` との違いは、値が行ではなく**表の中**にあること。
/// `cian.ai`・`cian.font`・`cian.ime` がこの形で、`cian.ssh` は中に
/// `hosts = { {…}, {…} }` を抱えるので別の道が要る（まだ無い）。
///
/// ## コメントアウトされたブロックは、有効にしない
///
/// 1行の設定なら「行頭の `-- ` を外す」が正解だが、ブロックは違う。見本には
/// `cian.ai {` が**2つ**コメントアウトされていて（Azure の例と OpenAI 互換の
/// 例）、中にはさらにコメントアウトされた行が混じっている。どちらをどう外すか
/// を機械が決めると、**書き手が読むつもりだった説明が設定になる**。
///
/// だから有効なブロックが無いときは、末尾の節に**新しく書く**。見本はそのまま
/// 説明として残り、実際の設定は一か所に集まる。
///
/// `value` が `None` ならその項目をコメントに戻す（行は消さない）。
pub fn set_field_in(text: &str, call: &str, key: &str, value: Option<&str>) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> =
        text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    let trailing = lines.last().map(|l| l.is_empty()).unwrap_or(false);
    if trailing {
        lines.pop();
    }

    let out = match live_block(&lines, call) {
        Some((from, to)) => {
            edit_block(&mut lines, from, to, key, value);
            lines
        }
        None => {
            if let Some(v) = value {
                append_block(&mut lines, call, key, v);
            }
            lines
        }
    };

    let mut s = out.join(nl);
    if trailing {
        s.push_str(nl);
    }
    s
}

/// 有効な（コメントアウトされていない）`cian.<call> {` の行範囲。
///
/// 同じ呼び出しが何度も有効なら**最後のもの**。Lua は上から実行するので、
/// あとの呼び出しが前のものを上書きする。
fn live_block(lines: &[String], call: &str) -> Option<(usize, usize)> {
    let head = format!("cian.{call}");
    let mut found = None;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("--") || !t.starts_with(&head) {
            continue;
        }
        // `cian.ai` と `cian.aicmd` を取り違えない。
        let after = &t[head.len()..];
        if !after.trim_start().starts_with('{') {
            continue;
        }
        // 釣り合う `}` まで。文字列の中の括弧は数えない。
        let mut depth = 0i32;
        for (j, l) in lines.iter().enumerate().skip(i) {
            depth += braces(l);
            if depth <= 0 && j >= i {
                found = Some((i, j));
                break;
            }
        }
    }
    found
}

/// その行の `{` と `}` の差。文字列の中とコメントは数えない。
fn braces(line: &str) -> i32 {
    let mut depth = 0i32;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '-' if i + 1 < chars.len() && chars[i + 1] == '-' => break,
            '"' | '\'' => {
                let q = chars[i];
                i += 1;
                while i < chars.len() && chars[i] != q {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    depth
}

/// ブロックの中の `key = …` を書き換える。1行に畳んであるブロックも同じ扱い。
fn edit_block(lines: &mut Vec<String>, from: usize, to: usize, key: &str, value: Option<&str>) {
    // 1行に畳んであるなら、その行の中で差し替える。
    if from == to {
        if let Some(v) = value {
            lines[from] = replace_inline(&lines[from], key, v);
        }
        return;
    }
    for i in from..=to {
        let t = lines[i].trim_start();
        let bare = t.strip_prefix("--").map(|r| r.trim_start()).unwrap_or(t);
        let is_key = bare
            .strip_prefix(key)
            .map(|r| r.trim_start().starts_with('='))
            .unwrap_or(false);
        if !is_key {
            continue;
        }
        let indent = &lines[i][..lines[i].len() - t.len()];
        match value {
            Some(v) => lines[i] = format!("{indent}{key} = {v},{}", tail_note(bare)),
            None => {
                if !t.starts_with("--") {
                    lines[i] = format!("{indent}-- {t}");
                }
            }
        }
        return;
    }
    // 無ければ閉じ括弧の手前に足す。
    if let Some(v) = value {
        let indent = lines
            .get(from + 1)
            .map(|l| l[..l.len() - l.trim_start().len()].to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "  ".to_string());
        lines.insert(to, format!("{indent}{key} = {v},"));
    }
}

/// 行末に付いている `-- 注` を、あれば返す（先頭の空白ごと）。
fn tail_note(bare: &str) -> String {
    let chars: Vec<char> = bare.chars().collect();
    let mut i = 0;
    while i + 1 < chars.len() {
        match chars[i] {
            '"' | '\'' => {
                let q = chars[i];
                i += 1;
                while i < chars.len() && chars[i] != q {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            '-' if chars[i + 1] == '-' => {
                return format!("   {}", chars[i..].iter().collect::<String>());
            }
            _ => {}
        }
        i += 1;
    }
    String::new()
}

/// 1行に畳んであるブロックの中で `key = …` を差し替える。無ければ足す。
fn replace_inline(line: &str, key: &str, value: &str) -> String {
    let Some(open) = line.find('{') else { return line.to_string() };
    let Some(close) = line.rfind('}') else { return line.to_string() };
    let inner = &line[open + 1..close];
    let mut parts: Vec<String> = Vec::new();
    let mut replaced = false;
    for part in inner.split(',') {
        if part.trim().is_empty() {
            continue;
        }
        let is_key = part
            .trim_start()
            .strip_prefix(key)
            .map(|r| r.trim_start().starts_with('='))
            .unwrap_or(false);
        if is_key {
            parts.push(format!(" {key} = {value}"));
            replaced = true;
        } else {
            // 末尾の空白は落とす ── 落とさないと `face = "Cica" , size = 14` の
            // ように、カンマの前に空白が残る。
            parts.push(part.trim_end().to_string());
        }
    }
    if !replaced {
        parts.push(format!(" {key} = {value}"));
    }
    format!("{}{{{} }}{}", &line[..open], parts.join(",").trim_end(), &line[close + 1..])
}

/// 末尾の節に、新しいブロックを書く。
fn append_block(lines: &mut Vec<String>, call: &str, key: &str, value: &str) {
    if !lines.iter().any(|l| l.trim() == ADDED_HEAD) {
        if lines.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
            lines.push(String::new());
        }
        lines.push(ADDED_HEAD.to_string());
    }
    lines.push(format!("cian.{call} {{"));
    lines.push(format!("  {key} = {value},"));
    lines.push("}".to_string());
}

// ── ファイルとして読み書きする ─────────────────────────────────────────

/// `init.lua` を読む。返すのは（場所、中身、Lua として読めなかった理由）。
///
/// **読めなかったら、設定画面は保存させない。** crmaine はここで転けている ──
/// JSON に書き間違いがあると読み込みが黙って既定へ落ち、設定画面はまっさらな
/// 状態で開き、保存を押した瞬間に元の設定が消えた（5,399 バイト → 64 バイト）。
/// 中身は必ず返すので、画面は「読めません」と言ったうえで**そのまま見せられる**。
pub fn read_init() -> (Option<std::path::PathBuf>, String, Option<String>) {
    let Some(path) = config_read_path("init.lua") else {
        return (None, String::new(), None);
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let bad = syntax_error(&text);
    (Some(path), text, bad)
}

/// Lua として読めるか。読めなければその訳を返す。
///
/// **走らせない。** `mlua` に**コンパイルだけ**させる ── 設定を確かめるために
/// 設定を実行すると、書き間違いを見るだけのつもりで `os.execute` が動く。
pub fn syntax_error(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    let lua = mlua::Lua::new();
    // **名前を付ける。** 付けないと mlua は読んだ場所の名で呼ぶので、
    // 「`crates/cian-lua/src/settings_edit.rs:454: unexpected symbol』」と出る ──
    // 自分の設定を直そうとしている人に、cian の中のファイルを見せることになる。
    match lua.load(text).set_name("init.lua").into_function() {
        Ok(_) => None,
        Err(e) => Some(e.to_string()),
    }
}

/// `init.lua` を書く。**控えを取ってから、一時ファイル経由で置き換える。**
///
/// 設定は人が何ヶ月もかけて育てるもので、上書きの失敗を取り返す手が要る。
/// 書き方を2つ持つと、片方だけ直したときにそちらだけ壊れる ── だから
/// 保存はこの1つの扉だけを通る。
///
/// 控えは `init.lua.bak`。**世代は持たない** ── 設定画面から何度も保存する
/// 人が、いつの控えか分からない `.bak.3` を並べても使えない。直前の1つが
/// 取り返せればいい。
pub fn write_init(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if path.exists() {
        std::fs::copy(path, path.with_extension("lua.bak"))?;
    }
    let tmp = path.with_extension("lua.cian-tmp");
    std::fs::write(&tmp, text)?;
    // 置き換えは rename。書いている途中で電源が落ちても、**半分書けた
    // init.lua** にはならない。
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
-- 設定の見本
-- ----------------------------------------------------------------------------
-- タブの桁数。既定は 4
-- cian.set_option(\"tab_width\", 8)

cian.set_option(\"editor\", \"nvim\")   -- お気に入り
";

    #[test]
    fn turns_a_commented_line_on() {
        let out = set_option_in(SAMPLE, "tab_width", Some("8"));
        assert!(out.contains("\ncian.set_option(\"tab_width\", 8)\n"), "{out}");
        // **説明は残る。**
        assert!(out.contains("-- タブの桁数。既定は 4"), "{out}");
    }

    #[test]
    fn changes_a_live_line_and_keeps_its_note() {
        let out = set_option_in(SAMPLE, "editor", Some("\"code -w\""));
        assert!(out.contains("cian.set_option(\"editor\", \"code -w\")   -- お気に入り"), "{out}");
    }

    #[test]
    fn unsetting_comments_out_rather_than_deleting() {
        let out = set_option_in(SAMPLE, "editor", None);
        assert!(out.contains("-- cian.set_option(\"editor\", \"nvim\")   -- お気に入り"), "{out}");
        // 行数は変わらない ── 説明ごと消さない。
        assert_eq!(SAMPLE.lines().count(), out.lines().count());
    }

    #[test]
    fn an_unknown_option_lands_in_a_section_that_says_who_wrote_it() {
        let out = set_option_in(SAMPLE, "borders", Some("\"rounded\""));
        assert!(out.contains(ADDED_HEAD), "{out}");
        assert!(out.trim_end().ends_with("cian.set_option(\"borders\", \"rounded\")"), "{out}");
        // 二度足しても見出しは1つ。
        let again = set_option_in(&out, "home", Some("\"~/Desktop\""));
        assert_eq!(again.matches(ADDED_HEAD).count(), 1, "{again}");
    }

    /// **最後に有効な行が勝つ。** 見本の `shell` は3行並んでいる。
    #[test]
    fn the_last_live_line_is_the_one_that_counts() {
        let text = "\
-- cian.set_option(\"shell\", \"powershell.exe\")
cian.set_option(\"shell\", \"pwsh.exe\")
cian.set_option(\"shell\", \"/bin/zsh\")
";
        let out = set_option_in(text, "shell", Some("\"/bin/bash\""));
        assert!(out.contains("-- cian.set_option(\"shell\", \"powershell.exe\")"), "{out}");
        assert!(out.contains("cian.set_option(\"shell\", \"pwsh.exe\")"), "{out}");
        assert!(out.contains("cian.set_option(\"shell\", \"/bin/bash\")"), "{out}");
        assert!(!out.contains("/bin/zsh"), "最後の行が書き換わる: {out}");
    }

    /// 値の中に `)` があっても、行の終わりを見誤らない。
    #[test]
    fn a_bracket_inside_the_value_does_not_end_the_line() {
        let text = "cian.set_option(\"editor\", \"code (insiders) -w\")   -- 注\n";
        let out = set_option_in(text, "editor", Some("\"nvim\""));
        assert_eq!(out, "cian.set_option(\"editor\", \"nvim\")   -- 注\n");
    }

    /// 表も書ける。
    #[test]
    fn a_table_value_round_trips() {
        let text = "-- cian.set_option(\"preview_skip\", { \"bak\" })\n";
        let out = set_option_in(text, "preview_skip", Some("{ \"bak\", \"dmp\" }"));
        assert_eq!(out, "cian.set_option(\"preview_skip\", { \"bak\", \"dmp\" })\n");
    }

    /// **改行は元のまま。** `\\r\\n` のファイルを `\\n` にすると、git が全行を
    /// 変更として出す ── 会社の Windows で使うものなので、黙って揃えない。
    #[test]
    fn crlf_stays_crlf() {
        let text = "-- cian.set_option(\"tab_width\", 8)\r\ncian.set_option(\"lang\", \"ja\")\r\n";
        let out = set_option_in(text, "tab_width", Some("4"));
        assert!(!out.contains("\n\n"), "{out:?}");
        assert_eq!(out.matches("\r\n").count(), 2, "{out:?}");
        assert!(out.starts_with("cian.set_option(\"tab_width\", 4)\r\n"), "{out:?}");
    }

    /// インデントされた行（`if` の中など）は、そのまま桁を保つ。
    #[test]
    fn indentation_survives() {
        let text = "if windows then\n    cian.set_option(\"shell\", \"pwsh.exe\")\nend\n";
        let out = set_option_in(text, "shell", Some("\"cmd.exe\""));
        assert!(out.contains("\n    cian.set_option(\"shell\", \"cmd.exe\")\n"), "{out}");
    }

    /// **見つからない書き方は、見つからないのが正しい。**
    ///
    /// 変数に入れてから呼ぶ書き方を「賢く」拾おうとすると、書き手の行を
    /// 壊す道具になる。拾わなければ末尾に足すだけで、元の行は無傷。
    #[test]
    fn a_clever_spelling_is_left_alone() {
        let text = "local k = \"tab_width\"\ncian.set_option(k, 8)\n";
        let out = set_option_in(text, "tab_width", Some("4"));
        assert!(out.contains("cian.set_option(k, 8)"), "元の行は無傷: {out}");
        assert!(out.contains(ADDED_HEAD), "{out}");
    }

    // ── ファイルとして ───────────────────────────────────────────

    /// **読めないファイルでは保存させない**ための判定。
    #[test]
    fn a_broken_file_says_why() {
        assert_eq!(syntax_error("cian.set_option(\"a\", 1)\n"), None);
        assert_eq!(syntax_error(""), None);
        let bad = syntax_error("cian.set_option(\"a\", \n").expect("読めないはず");
        assert!(!bad.is_empty(), "{bad}");
    }

    /// **設定を確かめるために設定を走らせない。**
    ///
    /// コンパイルするだけなので、副作用のある行があっても何も起きない。
    #[test]
    fn checking_does_not_run_the_file() {
        let dir = std::env::temp_dir().join("cian-settings-edit-must-not-run");
        let _ = std::fs::remove_file(&dir);
        let lua = format!("local f = io.open({:?}, \"w\") f:write(\"x\") f:close()", dir);
        assert_eq!(syntax_error(&lua), None, "読めるはず");
        assert!(!dir.exists(), "走ってしまっています: {dir:?}");
    }

    /// 控えを取り、一時ファイル経由で置き換える。
    #[test]
    fn writing_leaves_a_copy_of_what_was_there() {
        let dir = std::env::temp_dir().join(format!("cian-se-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("砂場");
        let path = dir.join("init.lua");
        std::fs::write(&path, "-- もとの中身\n").expect("書けるはず");

        write_init(&path, "-- 新しい中身\n").expect("書けるはず");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "-- 新しい中身\n");
        assert_eq!(
            std::fs::read_to_string(path.with_extension("lua.bak")).unwrap(),
            "-- もとの中身\n",
            "控えが要る",
        );
        // 一時ファイルは残らない。
        assert!(!path.with_extension("lua.cian-tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── 読む側 ───────────────────────────────────────────────────

    #[test]
    fn reads_back_what_is_actually_written() {
        assert_eq!(get_option_in(SAMPLE, "editor").as_deref(), Some("\"nvim\""));
        // コメントアウトされている行は「書いてある」ではない。
        assert_eq!(get_option_in(SAMPLE, "tab_width"), None);
        // 書いたら読める。
        let out = set_option_in(SAMPLE, "tab_width", Some("8"));
        assert_eq!(get_option_in(&out, "tab_width").as_deref(), Some("8"));
        // 戻したら読めない。
        let back = set_option_in(&out, "tab_width", None);
        assert_eq!(get_option_in(&back, "tab_width"), None);
    }

    /// **最後の有効な行を読む** ── 書き換えるのと同じ行でなければ、画面に
    /// 出ている値と、保存が触る行がずれる。
    #[test]
    fn the_value_read_is_the_value_that_wins() {
        let text = "cian.set_option(\"shell\", \"pwsh.exe\")\ncian.set_option(\"shell\", \"/bin/zsh\")\n";
        assert_eq!(get_option_in(text, "shell").as_deref(), Some("\"/bin/zsh\""));
    }

    #[test]
    fn reads_a_field_out_of_a_block() {
        assert_eq!(get_field_in(AI, "ai", "model").as_deref(), Some("\"gpt-5-mini\""));
        // 行末の注は値ではない。
        assert_eq!(get_field_in(AI, "ai", "endpoint").as_deref(), Some("\"https://old/llmaoai\""));
        assert_eq!(get_field_in(AI, "ai", "api_key"), None);
        // 1行に畳んであっても読める。
        let one = "cian.font{ face = \"Cica\", size = 14 }\n";
        assert_eq!(get_field_in(one, "font", "face").as_deref(), Some("\"Cica\""));
        assert_eq!(get_field_in(one, "font", "size").as_deref(), Some("14"));
    }

    // ── ブロック（`cian.ai { … }`）─────────────────────────────────

    const AI: &str = "\
-- 設定例:
-- cian.ai {
--   endpoint = \"https://example/llmaoai\",
-- }

cian.ai {
  endpoint  = \"https://old/llmaoai\",   -- 社内
  model     = \"gpt-5-mini\",
}
";

    #[test]
    fn a_field_in_a_live_block_changes_and_keeps_its_note() {
        let out = set_field_in(AI, "ai", "endpoint", Some("\"https://new/llmaoai\""));
        assert!(out.contains("  endpoint = \"https://new/llmaoai\",   -- 社内"), "{out}");
        assert!(out.contains("  model     = \"gpt-5-mini\","), "{out}");
    }

    #[test]
    fn a_missing_field_lands_inside_the_block() {
        let out = set_field_in(AI, "ai", "auth_mode", Some("\"broker\""));
        let body = &out[out.rfind("cian.ai {").unwrap()..];
        assert!(body.contains("  auth_mode = \"broker\","), "{out}");
        // 閉じ括弧の手前に入る。
        assert!(body.find("auth_mode").unwrap() < body.find('}').unwrap(), "{out}");
    }

    #[test]
    fn unsetting_a_field_comments_it_out() {
        let out = set_field_in(AI, "ai", "model", None);
        assert!(out.contains("  -- model     = \"gpt-5-mini\","), "{out}");
    }

    /// **コメントアウトされたブロックは有効にしない。**
    ///
    /// 見本には `cian.ai {` が2つコメントアウトされていて、中にはさらに
    /// コメントアウトされた行が混じっている。どちらをどう外すかを機械が
    /// 決めると、説明のつもりで書いた行が設定になる。
    #[test]
    fn a_commented_block_stays_a_comment_and_a_new_one_is_written() {
        let only_comment = "-- cian.ai {\n--   endpoint = \"https://example\",\n-- }\n";
        let out = set_field_in(only_comment, "ai", "model", Some("\"gpt-5-mini\""));
        assert!(out.contains("-- cian.ai {"), "見本はそのまま: {out}");
        assert!(out.contains(ADDED_HEAD), "{out}");
        assert!(out.contains("cian.ai {\n  model = \"gpt-5-mini\",\n}"), "{out}");
    }

    /// 1行に畳んであるブロック（見本の `cian.font{ … }`）。
    #[test]
    fn a_one_line_block_is_edited_in_place() {
        let text = "cian.font{ face = \"JetBrainsMono Nerd Font\" }\n";
        let out = set_field_in(text, "font", "face", Some("\"Cica\""));
        assert_eq!(out, "cian.font{ face = \"Cica\" }\n");
        let more = set_field_in(&out, "font", "size", Some("14"));
        assert_eq!(more, "cian.font{ face = \"Cica\", size = 14 }\n");
    }

    /// **`cian.ai` と `cian.aicmd` を取り違えない。**
    #[test]
    fn a_longer_name_is_not_mistaken_for_this_one() {
        let text = "cian.aicmd { x = 1 }\n";
        let out = set_field_in(text, "ai", "model", Some("\"m\""));
        assert!(out.contains("cian.aicmd { x = 1 }"), "元の行は無傷: {out}");
        assert!(out.contains(ADDED_HEAD), "{out}");
    }

    /// **あとの呼び出しが勝つ。** Lua は上から実行する。
    #[test]
    fn the_last_live_block_is_the_one_edited() {
        let text = "cian.ai {\n  model = \"a\",\n}\ncian.ai {\n  model = \"b\",\n}\n";
        let out = set_field_in(text, "ai", "model", Some("\"c\""));
        assert!(out.contains("  model = \"a\","), "{out}");
        assert!(out.contains("  model = \"c\","), "{out}");
        assert!(!out.contains("\"b\""), "{out}");
    }

    /// 鍵も同じ道を通る。**本人の判断で init.lua に書く**（2026-09-11）──
    /// もともと `password` も `api_key` もこのファイルに書く作りで、見本自身が
    /// 「平文で保存されます。可能なら `password_cmd` を」と註を付けている。
    #[test]
    fn a_secret_is_written_like_any_other_field() {
        let out = set_field_in(AI, "ai", "api_key", Some("\"sk-xxx\""));
        assert!(out.contains("  api_key = \"sk-xxx\","), "{out}");
    }

    /// 同梱の見本を通しても、説明は消えない。
    #[test]
    fn the_shipped_sample_survives_block_edits() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("the workspace root")
            .join("examples/init.lua");
        let before = std::fs::read_to_string(&path).expect("the sample init.lua");
        let mut after = before.clone();
        for (key, v) in [
            ("endpoint", "\"https://x/llmaoai\""),
            ("model", "\"gpt-5-mini\""),
            ("auth_mode", "\"broker\""),
        ] {
            after = set_field_in(&after, "ai", key, Some(v));
        }
        for line in before.lines() {
            assert!(after.contains(line), "消えた: {line}");
        }
        assert!(after.contains("cian.ai {"), "{}", &after[after.len() - 200..]);
    }

    /// 同梱の見本を、まるごと通す。**説明は1行も減らない。**
    #[test]
    fn the_shipped_sample_keeps_every_comment() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("the workspace root")
            .join("examples/init.lua");
        let before = std::fs::read_to_string(&path).expect("the sample init.lua");
        // 数ではなく**中身**で見る。数だけだと、1行消えて1行増えたときに
        // 釣り合って通る ── この家の検査が黙る形。
        let prose = |s: &str| -> Vec<String> {
            s.lines()
                .map(|l| l.trim().to_string())
                .filter(|t| {
                    t.starts_with("--")
                        && !t.trim_start_matches('-').trim_start().starts_with("cian.")
                })
                .collect()
        };
        let mut after = before.clone();
        for (name, value) in [
            ("tab_width", "8"),
            ("editor", "\"nvim\""),
            ("show_hidden", "false"),
            ("borders", "\"rounded\""),
            ("home", "\"~/Desktop\""),
        ] {
            after = set_option_in(&after, name, Some(value));
        }
        for name in ["tab_width", "editor"] {
            after = set_option_in(&after, name, None);
        }
        let (a, b) = (prose(&before), prose(&after));
        for line in &a {
            assert!(b.contains(line), "説明が消えた: {line}");
        }
        // 増えたのは、設定画面が書いた節の見出しだけ。
        let added: Vec<_> = b.iter().filter(|l| !a.contains(l)).collect();
        assert_eq!(added, vec![&ADDED_HEAD.to_string()], "見出し以外を足さない");
        // 中身も減らない。
        assert!(after.len() > before.len() - 200, "{} → {}", before.len(), after.len());
    }
}
