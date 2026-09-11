//! 設定画面から SSH ホストを足す・直す・消す。
//!
//! `set_option_in` は1行、`set_field_in` は平たい表。こちらは**入れ子の配列**で、
//! crmaine が「object の配列を『1行に1つ』で編集させるな、保存すると設定が
//! 壊れる」と書いていたのがちょうどこの形だ（`gui/settings.js` の
//! `isPlainList`）。だからホスト1つにつき1つのフォームを出し、ここは
//! **名前で引いて、そのホストの行だけ**を書き換える。
//!
//! ## どのファイルに書くか
//!
//! `cian.ssh{…}` は `init.lua` にも `ssh.lua` にも書ける ── cian は
//! `init.lua` の直後に `ssh.lua` を**同じ設定として**読む。だから設定画面が
//! いつも `init.lua` へ書くと、`ssh.lua` を使っている人には**二つ目の水源**が
//! できる。この家で最頻のバグ（設定を直したのに効かない）に、新しい源を
//! 足すことになる。
//!
//! 規則:
//!
//! 1. `ssh.lua` に有効な `cian.ssh{}` があれば、そこ
//! 2. `init.lua` にあれば、そこ ── **手で書いた人の場所を、勝手に移さない**
//! 3. どちらにも無ければ **`ssh.lua`**（無ければ作る）
//!
//! 3 が `init.lua` だった日がある。**`ssh.lua` は SSH ホストの置き場として
//! 用意された別ファイル**で、同梱の見本も「init.lua を表示 / Git・SVN / AI
//! まわりに集中させておくための分割です」と書いている。そこへ置かずに
//! `init.lua` を太らせるのは、この分割を無かったことにするのと同じ。
//!
//! 画面はどのファイルに書くかを出す。
//!
//! ## 触るのは name / host / port / notes まで
//!
//! `users` は文字列でもテーブルでも書けて、中にパスワードや鍵が入る
//! （`{ name = "ci", password_cmd = "pass show ci/stage" }`）。**畳んで
//! 書き直すと、その形が失われる**ので、ここでは users の字面を**そのまま
//! 運ぶ**。画面は読めるように出すが、直すのは init.lua で。

use std::path::{Path, PathBuf};

use crate::settings_edit::{braces, live_block};

/// 一覧に出す、ホスト1つぶん。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HostRow {
    pub name: String,
    pub host: String,
    /// 書いてなければ空。
    pub port: String,
    pub notes: String,
    /// `users = …` の**字面のまま**。設定画面は読めるように出すだけで、
    /// 書き換えない ── 鍵やパスワードの形を畳むと失われる。
    pub users: String,
}

/// どのファイルに書くか。**ファイルを読まずに決められる部分だけ**を
/// ここに出してあるので、検査が当てられる（`ssh_files` は実際の設定
/// ディレクトリを触るので、単体では確かめにくい）。
pub fn pick_target(ssh_has_block: bool, init_has_block: bool) -> &'static str {
    if ssh_has_block {
        "ssh.lua"
    } else if init_has_block {
        // **手で書いた人の場所を、勝手に移さない。**
        "init.lua"
    } else {
        // **SSH ホストの家は `ssh.lua`。** 同梱の見本も「init.lua を
        // 表示 / Git・SVN / AI まわりに集中させておくための分割です」と書いて
        // いる。無ければ作る。
        "ssh.lua"
    }
}

/// `cian.ssh{ hosts = { … } }` を持っているファイル。
///
/// 返すのは（読む場所、書く場所）。読む場所が `None` なら、まだどこにも
/// 書かれていない。
pub fn ssh_files() -> (Option<PathBuf>, Option<PathBuf>) {
    let has = |name: &str| {
        crate::config_read_path(name)
            .and_then(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
            .map(|(p, t)| {
                let lines: Vec<String> = t.lines().map(|l| l.to_string()).collect();
                (p, live_block(&lines, "ssh").is_some())
            })
    };
    for name in [pick_target(true, false), "init.lua"] {
        if let Some((p, true)) = has(name) {
            // 読んだのと**同じファイル**へ書き戻す。`config_write_path` は
            // ポータブル構成で場所が変わるので、読み書きで別の答えになりうる。
            return (Some(p.clone()), Some(p));
        }
    }
    // **どちらにも無ければ `ssh.lua`。** ここが SSH ホストの家で、
    // 置いてあれば cian が `init.lua` の直後に読む（`SPLIT_CONFIG_FILES`）。
    (None, crate::config_write_path(pick_target(false, false)))
}

/// まだ `ssh.lua` が無いときに、最初に書く見出し。
///
/// **何のファイルかを、ファイル自身に書いておく。** 設定画面が黙って作った
/// `cian.ssh{}` だけのファイルは、あとで開いた人に「これは何で、手で直して
/// いいのか」を答えない。
pub fn ssh_head() -> String {
    format!(
        "-- ssh.lua — SSH ホスト。cian は init.lua の直後に、同じ設定として読みます。\n\
         --\n\
         -- users の中に鍵やパスワードを書くときは、このファイルを chmod 600 に。\n\
         \n\
         {}\n",
        crate::settings_edit::ADDED_HEAD,
    )
}

/// `ssh.lua` を新しく作るときの権限。
///
/// **パスワードを置ける場所は、置ける権限で作る。** cian は誰でも読める
/// ファイルにパスワードがあると起動時に警告するが、警告より先に、作る側が
/// 正しく作ればいい。Windows にはこの考え方が無い。
pub fn tighten(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// `hosts = {` の中身の行範囲（`{` の行と `}` の行）。
fn hosts_span(lines: &[String]) -> Option<(usize, usize)> {
    let (from, to) = live_block(lines, "ssh")?;
    let start = (from..=to).find(|&i| {
        let t = lines[i].trim_start();
        !t.starts_with("--") && t.starts_with("hosts") && t.contains('{')
    })?;
    let mut depth = 0i32;
    for (j, l) in lines.iter().enumerate().take(to + 1).skip(start) {
        depth += braces(l);
        if depth <= 0 {
            return Some((start, j));
        }
    }
    None
}

/// そのホストの項目1つが占める行範囲。`{` で始まり、釣り合う `}` まで。
fn entries(lines: &[String]) -> Vec<(usize, usize)> {
    let Some((start, end)) = hosts_span(lines) else { return Vec::new() };
    let mut out = Vec::new();
    let mut i = start;
    // `hosts = {` の行に最初の `{` があるので、次の行から見る。
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
fn field_of(lines: &[String], from: usize, to: usize, key: &str) -> String {
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
            if chars[i] == '\\' && i + 1 < chars.len() {
                i += 1;
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

/// 書いてあるホストを、書いてある順に。
pub fn hosts_in(text: &str) -> Vec<HostRow> {
    let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    entries(&lines)
        .into_iter()
        .map(|(from, to)| HostRow {
            name: field_of(&lines, from, to, "name"),
            host: field_of(&lines, from, to, "host"),
            port: field_of(&lines, from, to, "port"),
            notes: field_of(&lines, from, to, "notes"),
            users: field_of(&lines, from, to, "users"),
        })
        .filter(|h| !h.name.is_empty() || !h.host.is_empty())
        .collect()
}

/// Lua の文字列に入れられる形に。**バックスラッシュを先に。**
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// ホスト1つを、1行の Lua に。`users` は受け取った字面のまま置く。
fn render(h: &HostRow, indent: &str) -> String {
    let mut parts = vec![format!("name = {}", quote(&h.name)), format!("host = {}", quote(&h.host))];
    if !h.port.trim().is_empty() {
        parts.push(format!("port = {}", h.port.trim()));
    }
    if !h.users.trim().is_empty() {
        parts.push(format!("users = {}", h.users.trim()));
    }
    if !h.notes.trim().is_empty() {
        parts.push(format!("notes = {}", quote(&h.notes)));
    }
    format!("{indent}{{ {} }},", parts.join(", "))
}

/// `name` のホストを、足す・直す・消す。
///
/// * 同じ名前があれば**その項目だけ**を書き直す（ほかの行は読まない）
/// * 無ければ `hosts = {` の閉じ括弧の手前に足す
/// * `row` が `None` なら、その項目を消す ── **ここは行を消してよい。**
///   1行の設定と違って、ホストの項目は説明ではなく台帳の一行で、
///   コメントに戻すと「消したのに繋げてしまう」ほうの事故が無い代わりに、
///   消したはずのものが一覧に残る
/// * `cian.ssh{}` がどこにも無ければ、末尾に新しく書く
pub fn set_host_in(text: &str, name: &str, row: Option<&HostRow>) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> =
        text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    let trailing = lines.last().map(|l| l.is_empty()).unwrap_or(false);
    if trailing {
        lines.pop();
    }

    let found = entries(&lines)
        .into_iter()
        .find(|&(from, to)| field_of(&lines, from, to, "name") == name);

    match (row, found) {
        (Some(r), Some((from, to))) => {
            let indent = lines[from][..lines[from].len() - lines[from].trim_start().len()].to_string();
            lines.splice(from..=to, [render(r, &indent)]);
        }
        (None, Some((from, to))) => {
            lines.drain(from..=to);
        }
        (Some(r), None) => match hosts_span(&lines) {
            Some((start, end)) => {
                let indent = lines
                    .get(start + 1)
                    .map(|l| l[..l.len() - l.trim_start().len()].to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "    ".to_string());
                lines.insert(end, render(r, &indent));
            }
            None => {
                if !lines.iter().any(|l| l.trim() == crate::settings_edit::ADDED_HEAD) {
                    if lines.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                        lines.push(String::new());
                    }
                    lines.push(crate::settings_edit::ADDED_HEAD.to_string());
                }
                lines.push("cian.ssh {".to_string());
                lines.push("  hosts = {".to_string());
                lines.push(render(r, "    "));
                lines.push("  },".to_string());
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

/// ホストの居場所を読む。返すのは（読んだファイル、中身、読めなかった訳）。
pub fn read_hosts() -> (Option<PathBuf>, String, Option<String>, Vec<HostRow>) {
    let (read_from, _) = ssh_files();
    let Some(path) = read_from else {
        return (None, String::new(), None, Vec::new());
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let bad = crate::settings_edit::syntax_error(&text);
    let rows = if bad.is_none() { hosts_in(&text) } else { Vec::new() };
    (Some(path), text, bad, rows)
}

/// 書く先。読む場所があればそこ、無ければ `init.lua`。
pub fn write_target() -> Option<PathBuf> {
    let (read_from, write_to) = ssh_files();
    read_from.or(write_to)
}

/// ホストの一覧を保存する前に、その場所が**誰でも読める**かを見る。
///
/// cian は起動時にも同じことを言うが、**書く瞬間に言うほうが早い** ──
/// 平文のパスワードを置いた人が、置いた直後に知れる。Windows には
/// この権限の考え方が無いので、Unix でだけ見る。
pub fn world_readable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).map(|m| m.permissions().mode() & 0o077 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SSH: &str = "\
-- 見本のつもり
cian.ssh {
  users = { \"root\", \"deploy\" },
  hosts = {
    { name = \"web1\", host = \"10.0.1.11\" },
    { name = \"db1\",  host = \"10.0.2.31\", port = 2222 },
    { name = \"stage\", host = \"stage.example.com\",
      users = {
        \"readonly\",
        { name = \"ci\", password_cmd = \"pass show ci/stage\" },
      },
    },
  },
}
";

    #[test]
    fn reads_the_hosts_that_are_written() {
        let rows = hosts_in(SSH);
        assert_eq!(rows.len(), 3, "{rows:#?}");
        assert_eq!(rows[0].name, "web1");
        assert_eq!(rows[0].host, "10.0.1.11");
        assert_eq!(rows[0].port, "");
        assert_eq!(rows[1].port, "2222");
        assert_eq!(rows[2].name, "stage");
        // **users は字面のまま。** 畳むと鍵やパスワードの形が失われる。
        assert!(rows[2].users.contains("password_cmd"), "{:?}", rows[2].users);
        assert!(rows[0].users.is_empty());
    }

    /// `key_pass` の中の `key` を `key` と読まない。
    #[test]
    fn a_longer_name_is_not_mistaken_for_a_shorter_one() {
        let text = "cian.ssh {\n  hosts = {\n    { name = \"a\", host = \"h\", notes = \"x\" },\n  },\n}\n";
        assert_eq!(hosts_in(text)[0].notes, "x");
        let odd = "cian.ssh {\n  hosts = {\n    { hostname = \"no\", name = \"a\", host = \"yes\" },\n  },\n}\n";
        assert_eq!(hosts_in(odd)[0].host, "yes");
    }

    #[test]
    fn changing_one_host_leaves_the_others_alone() {
        let row = HostRow {
            name: "db1".into(),
            host: "10.0.2.99".into(),
            port: "2222".into(),
            ..Default::default()
        };
        let out = set_host_in(SSH, "db1", Some(&row));
        assert!(out.contains("{ name = \"db1\", host = \"10.0.2.99\", port = 2222 },"), "{out}");
        assert!(out.contains("{ name = \"web1\", host = \"10.0.1.11\" },"), "{out}");
        assert!(out.contains("password_cmd"), "stage の users が消えていない: {out}");
        assert!(out.contains("-- 見本のつもり"), "{out}");
    }

    /// **users は運ぶ。** 名前と住所だけ直しても、鍵の設定は残る。
    #[test]
    fn the_users_table_is_carried_through_untouched() {
        let mut row = hosts_in(SSH)[2].clone();
        row.host = "stage2.example.com".into();
        let out = set_host_in(SSH, "stage", Some(&row));
        assert!(out.contains("stage2.example.com"), "{out}");
        assert!(out.contains("password_cmd = \"pass show ci/stage\""), "{out}");
        assert_eq!(hosts_in(&out).len(), 3);
    }

    #[test]
    fn a_new_host_lands_inside_the_hosts_table() {
        let row = HostRow { name: "new1".into(), host: "192.0.2.1".into(), ..Default::default() };
        let out = set_host_in(SSH, "new1", Some(&row));
        let rows = hosts_in(&out);
        assert_eq!(rows.len(), 4, "{out}");
        assert_eq!(rows[3].name, "new1");
        // 閉じ括弧の手前 ── `hosts` の中に入っている。
        assert!(out.contains("    { name = \"new1\", host = \"192.0.2.1\" },\n  },"), "{out}");
    }

    #[test]
    fn removing_a_host_takes_its_whole_entry() {
        let out = set_host_in(SSH, "stage", None);
        let rows = hosts_in(&out);
        assert_eq!(rows.len(), 2, "{out}");
        assert!(!out.contains("password_cmd"), "users ごと消える: {out}");
        assert!(out.contains("web1") && out.contains("db1"), "{out}");
    }

    /// `cian.ssh{}` がどこにも無いところに、1つ目を足す。
    #[test]
    fn the_first_host_writes_a_whole_block() {
        let row = HostRow { name: "web1".into(), host: "10.0.0.1".into(), ..Default::default() };
        let out = set_host_in("-- 何も無い\n", "web1", Some(&row));
        assert!(out.contains(crate::settings_edit::ADDED_HEAD), "{out}");
        assert!(out.contains("cian.ssh {\n  hosts = {\n    { name = \"web1\", host = \"10.0.0.1\" },\n  },\n}"), "{out}");
        assert_eq!(hosts_in(&out).len(), 1);
        assert_eq!(crate::settings_edit::syntax_error(&out), None, "書いたものが Lua として読める");
    }

    /// **コメントアウトされた見本は、ホストではない。**
    #[test]
    fn the_commented_sample_is_not_a_host() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("the workspace root")
            .join("examples/ssh.lua");
        let text = std::fs::read_to_string(&path).expect("the sample ssh.lua");
        assert!(hosts_in(&text).is_empty(), "{:#?}", hosts_in(&text));
        // 足しても、見本の説明は残る。
        let row = HostRow { name: "web9".into(), host: "10.9.9.9".into(), ..Default::default() };
        let out = set_host_in(&text, "web9", Some(&row));
        for line in text.lines() {
            assert!(out.contains(line), "消えた: {line}");
        }
        assert_eq!(hosts_in(&out).len(), 1);
    }

    /// **どのファイルに書くか。**
    ///
    /// 一度 `init.lua` を既定にして本人に見つかった（2026-09-11）──
    /// 「あれ・・？ ssh って別ファイルじゃなかったっけ？」。`ssh.lua` は
    /// SSH ホストの置き場として用意された別ファイルで、そこへ置かずに
    /// `init.lua` を太らせるのは、この分割を無かったことにするのと同じ。
    #[test]
    fn new_hosts_go_to_the_file_that_is_for_them() {
        // 何も無いところ ── **`ssh.lua` を作る。**
        assert_eq!(pick_target(false, false), "ssh.lua");
        // `ssh.lua` に書いてあるなら、そこ。
        assert_eq!(pick_target(true, false), "ssh.lua");
        // **`init.lua` に手で書いた人の場所は、勝手に移さない。**
        assert_eq!(pick_target(false, true), "init.lua");
        // 両方にあるなら `ssh.lua`（cian もそちらを後に読む）。
        assert_eq!(pick_target(true, true), "ssh.lua");
    }

    /// 新しく作る `ssh.lua` は、**何のファイルかを自分で言う**。
    #[test]
    fn a_fresh_ssh_lua_explains_itself() {
        let head = ssh_head();
        assert!(head.contains("ssh.lua"), "{head}");
        assert!(head.contains("chmod 600"), "鍵を置く場所だと言う: {head}");
        // 見出しの中に「設定画面が書きます」が入っているので、足すときに
        // **二度書かれない**。
        assert!(head.contains(crate::settings_edit::ADDED_HEAD), "{head}");
        let row = HostRow { name: "web1".into(), host: "10.0.0.1".into(), ..Default::default() };
        let out = set_host_in(&head, "web1", Some(&row));
        assert_eq!(out.matches(crate::settings_edit::ADDED_HEAD).count(), 1, "{out}");
        assert_eq!(crate::settings_edit::syntax_error(&out), None, "{out}");
        assert_eq!(hosts_in(&out).len(), 1);
    }

    /// **生きているブロックの中の、コメントアウトされたホストは数えない。**
    ///
    /// 一時的に1台だけ外す、はよくある。数えてしまうと一覧に幽霊が出るし、
    /// その名前で保存すると**コメントの中の行を書き換える**。
    #[test]
    fn a_commented_host_inside_a_live_block_is_not_counted() {
        let text = "\
cian.ssh {
  hosts = {
    { name = \"web1\", host = \"10.0.1.11\" },
    -- { name = \"old\", host = \"10.0.9.99\" },
  },
}
";
        let rows = hosts_in(text);
        assert_eq!(rows.len(), 1, "{rows:#?}");
        assert_eq!(rows[0].name, "web1");
        // その名前で足すと、コメントは触られず**新しい行**が入る。
        let row = HostRow { name: "old".into(), host: "10.0.9.1".into(), ..Default::default() };
        let out = set_host_in(text, "old", Some(&row));
        assert!(out.contains("-- { name = \"old\", host = \"10.0.9.99\" },"), "{out}");
        assert!(out.contains("{ name = \"old\", host = \"10.0.9.1\" },"), "{out}");
        assert_eq!(hosts_in(&out).len(), 2);
    }

    /// 書いたものは、必ず Lua として読める。
    #[test]
    fn everything_written_parses() {
        let mut out = SSH.to_string();
        for (name, host) in [("web1", "1.1.1.1"), ("new2", "2.2.2.2")] {
            let row = HostRow {
                name: name.into(),
                host: host.into(),
                notes: "引用符 \" と \\ が入っている".into(),
                ..Default::default()
            };
            out = set_host_in(&out, name, Some(&row));
            assert_eq!(crate::settings_edit::syntax_error(&out), None, "{out}");
        }
        out = set_host_in(&out, "db1", None);
        assert_eq!(crate::settings_edit::syntax_error(&out), None, "{out}");
        assert_eq!(hosts_in(&out).len(), 3);
        assert_eq!(hosts_in(&out)[0].notes, "引用符 \" と \\ が入っている");
    }

    #[test]
    fn crlf_stays_crlf() {
        let text = SSH.replace('\n', "\r\n");
        let row = HostRow { name: "web1".into(), host: "9.9.9.9".into(), ..Default::default() };
        let out = set_host_in(&text, "web1", Some(&row));
        assert!(!out.contains("\n\n"), "{out:?}");
        assert_eq!(out.matches("\r\n").count(), text.matches("\r\n").count());
    }
}
