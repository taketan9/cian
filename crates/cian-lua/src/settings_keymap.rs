//! 設定画面からキー割当を直す。
//!
//! 本人（2026-09-11）:「keymap.lua って設定画面から変更できないかな？
//! よくあるアプリケーションって自分のアプリの中でキーマップの設定を
//! 見直しできるよね？」
//!
//! そのとおりで、前の日に僕が書いた免除（「押して決めるものなので、欄に綴りを
//! 打たせる形が合わない」）は**逃げ**だった。綴りを打たせるのが合わないなら、
//! **押して決めさせればいい**。窓版は物理キーを読めるので（`e.code`）、
//! 欄に焦点があるあいだの打鍵を、そのまま割当にする。
//!
//! ## 足すのであって、置き換えるのではない
//!
//! `keys.rs` は**人が明示的に縛ったキーだけ**を先に見て、そこに無ければ既定の
//! 手当てに落ちる。だから割当は「既定を消す」ものではなく「そのキーを先に
//! 取る」もの ── 画面もそう言う。既定のキー一覧は `?` にある。
//!
//! ## どのファイルに書くか
//!
//! `ssh.lua` と同じ三つの規則。`keymap.lua` は `SPLIT_CONFIG_FILES` の一員で、
//! cian が init.lua の直後に同じ設定として読む。
//!
//! 1. `keymap.lua` に `cian.set_keymap(…)` があれば、そこ
//! 2. `init.lua` にあれば、そこ（**手で書いた人の場所を、勝手に移さない**）
//! 3. どちらにも無ければ **`keymap.lua`**（無ければ作る）

use std::path::PathBuf;

/// 縛れる動作ひとつ。
///
/// **既定のキーは持たない。** 持つと、`keys.rs` の巨大な match と二つ目の
/// 真実ができて、必ずずれる ── 画面が出すのは「あなたが縛ったキー」で、
/// 既定は `?` のキー一覧が答える。
#[derive(Debug, Clone, Copy)]
pub struct Bind {
    /// `cian.set_keymap("j", …)` の第二引数。
    pub action: &'static str,
    pub label_en: &'static str,
    pub label_ja: &'static str,
}

pub fn binds() -> &'static [Bind] {
    &[
        Bind { action: "cursor_down", label_en: "cursor down", label_ja: "カーソルを下へ" },
        Bind { action: "cursor_up", label_en: "cursor up", label_ja: "カーソルを上へ" },
        Bind { action: "cursor_top", label_en: "jump to the top", label_ja: "先頭へジャンプ" },
        Bind { action: "cursor_bottom", label_en: "jump to bottom", label_ja: "末尾へジャンプ" },
        Bind { action: "page_up", label_en: "move 10 lines up", label_ja: "10行上へ" },
        Bind { action: "page_down", label_en: "move 10 lines down", label_ja: "10行下へ" },
        Bind { action: "parent", label_en: "up one level", label_ja: "1階層上へ" },
        Bind { action: "enter", label_en: "enter folder / read the file / go into an archive (Ctrl+Enter launches it)", label_ja: "ディレクトリに入る／ファイルを読む／アーカイブの中へ（Ctrl+Enter でアプリ起動）" },
        Bind { action: "quit", label_en: "quit (confirms)", label_ja: "終了（確認あり）" },
        Bind { action: "search", label_en: "search in this folder", label_ja: "このディレクトリ内を検索" },
        Bind { action: "search_next", label_en: "next match", label_ja: "次のマッチ" },
        Bind { action: "search_prev", label_en: "previous match", label_ja: "前のマッチ" },
        Bind { action: "history", label_en: "history popup", label_ja: "履歴ポップアップ" },
        Bind { action: "shortcuts", label_en: "shortcuts menu", label_ja: "ショートカットメニュー" },
        Bind { action: "copy", label_en: "copy to opposite pane", label_ja: "反対ペインへコピー" },
        Bind { action: "move", label_en: "move to opposite pane", label_ja: "反対ペインへ移動" },
        Bind { action: "paste", label_en: "paste the file clipboard here", label_ja: "ファイルクリップボードをここに貼り付け" },
        Bind { action: "cut", label_en: "cut to the file clipboard", label_ja: "ファイルクリップボードへ切り取り" },
        Bind { action: "delete", label_en: "delete (to trash)", label_ja: "削除（ゴミ箱へ）" },
        Bind { action: "rename", label_en: "rename", label_ja: "リネーム" },
        Bind { action: "new_file", label_en: "new file", label_ja: "新規ファイル" },
        Bind { action: "new_dir", label_en: "new directory", label_ja: "新規ディレクトリ" },
        Bind { action: "open_other", label_en: "a folder → the opposite pane; a file → your own app", label_ja: "ディレクトリは反対ペインで開く／ファイルは既定のアプリで開く" },
        Bind { action: "open_other_tab", label_en: "open in a new tab in the other pane", label_ja: "反対ペインの新しいタブで開く" },
        Bind { action: "sync_from_other", label_en: "this pane → other pane's directory", label_ja: "このペインを反対ペインと同じ場所に" },
        Bind { action: "sync_to_other", label_en: "other pane → this pane's directory", label_ja: "反対ペインをこのペインと同じ場所に" },
        Bind { action: "open_external", label_en: "open with the OS", label_ja: "OS の関連付けで開く" },
        Bind { action: "copy_path", label_en: "copy path text to clipboard", label_ja: "パス文字列をクリップボードにコピー" },
        Bind { action: "copy_file_ref", label_en: "put the selection on the clipboard for Finder/Explorer to paste", label_ja: "選択をクリップボードへ（Finder/エクスプローラで貼り付け）" },
        Bind { action: "mark_down", label_en: "toggle mark, move down", label_ja: "マーク切替して下へ" },
        Bind { action: "mark_up", label_en: "toggle mark, move up", label_ja: "マーク切替して上へ" },
        Bind { action: "invert_marks", label_en: "invert all marks", label_ja: "全マークを反転" },
        Bind { action: "select_all", label_en: "mark every file", label_ja: "すべてマーク" },
        Bind { action: "visual", label_en: "visual select", label_ja: "ビジュアル選択" },
        Bind { action: "command", label_en: "command mode (:q, :shell, :man)", label_ja: "コマンドモード（:q, :shell, :man）" },
        Bind { action: "filter", label_en: "narrow the listing as you type", label_ja: "打ちながら一覧を絞り込む" },
        Bind { action: "find_recursive", label_en: "find files by name, below here", label_ja: "名前でファイルを再帰検索" },
        Bind { action: "grep_recursive", label_en: "grep inside files, below here", label_ja: "ファイルの中を再帰検索（grep）" },
        Bind { action: "sort", label_en: "the sort picker", label_ja: "並べ替えのピッカー" },
        Bind { action: "jump_path", label_en: "jump to a path you type", label_ja: "入力したパスへジャンプ" },
        Bind { action: "view", label_en: "view the file", label_ja: "ファイルを閲覧" },
        Bind { action: "diff", label_en: "compare the two panes' files", label_ja: "両ペインのファイルを比較" },
        Bind { action: "refresh", label_en: "read the listing again", label_ja: "一覧を読み直す" },
        Bind { action: "menu", label_en: "context menu for the entry (also :menu)", label_ja: "エントリのコンテキストメニュー（:menu でも）" },
        Bind { action: "ssh", label_en: "the SSH host picker", label_ja: "SSH ホストのピッカー" },
        Bind { action: "new_tab", label_en: "new tab", label_ja: "新しいタブ" },
        Bind { action: "close_tab", label_en: "close the tab", label_ja: "タブを閉じる" },
        Bind { action: "manual", label_en: "the key manual", label_ja: "キーのマニュアル" },
        Bind { action: "unbind", label_en: "bind nothing (turn a key off)", label_ja: "何もしない（キーを無効にする）" },
    ]
}

/// どのファイルに書くか。`settings_ssh::pick_target` と同じ三つの規則。
pub fn pick_target(keymap_has: bool, init_has: bool) -> &'static str {
    if keymap_has {
        "keymap.lua"
    } else if init_has {
        "init.lua"
    } else {
        "keymap.lua"
    }
}

/// まだ `keymap.lua` が無いときの見出し。
pub fn keymap_head() -> String {
    format!(
        "-- keymap.lua — キー割当。cian は init.lua の直後に、同じ設定として読みます。\n\
         --\n\
         -- ここに書いたキーが**先に**見られます。書かなかったキーは既定のまま動きます。\n\
         -- 既定のキー一覧は cian の中で `?`。\n\
         \n\
         {}\n",
        crate::settings_edit::ADDED_HEAD,
    )
}

/// 書いてある `cian.set_keymap("k", "action")` を、書いてある順に。
///
/// コメントアウトされた行は数えない ── 同梱の見本は 40 行ぜんぶ
/// コメントアウトされていて、それを「割り当て済み」と読むと画面が嘘をつく。
pub fn keymaps_in(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(binding_on).collect()
}

/// その行が有効な `cian.set_keymap("k", "action")` なら `(キー, 動作)`。
///
/// **行末の注を値に混ぜない。** 最初 `split(',')` で切って
/// `"enter")   -- 入る` を動作の名前として読んだ ── 引用符の中だけを見る。
fn binding_on(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    if t.starts_with("--") {
        return None;
    }
    let rest = t.strip_prefix("cian.set_keymap")?.trim_start().strip_prefix('(')?;
    let mut lits = Vec::new();
    let chars: Vec<char> = rest.chars().collect();
    let mut i = 0;
    while i < chars.len() && lits.len() < 2 {
        match chars[i] {
            '"' | '\'' => {
                let q = chars[i];
                i += 1;
                let mut s = String::new();
                while i < chars.len() && chars[i] != q {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                lits.push(s);
            }
            // 閉じ括弧より先には行かない ── その先は注。
            ')' => break,
            _ => {}
        }
        i += 1;
    }
    match lits.as_slice() {
        [k, a] if !k.is_empty() && !a.is_empty() => Some((k.clone(), a.clone())),
        _ => None,
    }
}

/// その動作の割当を、足す・差し替える・消す。
///
/// `key` が `None` なら、その動作に付いている**有効な行をコメントに戻す**
/// （行は消さない ── 1行の設定と同じで、そこに説明が付いていることがある）。
pub fn set_keymap_in(text: &str, action: &str, key: Option<&str>) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> =
        text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    let trailing = lines.last().map(|l| l.is_empty()).unwrap_or(false);
    if trailing {
        lines.pop();
    }

    let hit = |l: &str| binding_on(l).map(|(_, a)| a == action).unwrap_or(false);
    let found: Vec<usize> = (0..lines.len()).filter(|&i| hit(&lines[i])).collect();

    match key {
        Some(k) => {
            let line = format!("cian.set_keymap({:?}, {:?})", k, action);
            match found.last() {
                // **最後の有効な行が勝つ**ので、そこを書き換える。
                Some(&at) => {
                    let indent =
                        lines[at][..lines[at].len() - lines[at].trim_start().len()].to_string();
                    lines[at] = format!("{indent}{line}");
                }
                None => {
                    if !lines.iter().any(|l| l.trim() == crate::settings_edit::ADDED_HEAD) {
                        if lines.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                            lines.push(String::new());
                        }
                        lines.push(crate::settings_edit::ADDED_HEAD.to_string());
                    }
                    lines.push(line);
                }
            }
        }
        None => {
            for &at in &found {
                let t = lines[at].trim_start();
                let indent = lines[at][..lines[at].len() - t.len()].to_string();
                lines[at] = format!("{indent}-- {t}");
            }
        }
    }

    let mut out = lines.join(nl);
    if trailing {
        out.push_str(nl);
    }
    out
}

/// 読む場所と書く場所。
pub fn keymap_files() -> (Option<PathBuf>, Option<PathBuf>) {
    let has = |name: &str| {
        crate::config_read_path(name)
            .and_then(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
            .map(|(p, t)| (p, !keymaps_in(&t).is_empty()))
    };
    for name in [pick_target(true, false), "init.lua"] {
        if let Some((p, true)) = has(name) {
            return (Some(p.clone()), Some(p));
        }
    }
    (None, crate::config_write_path(pick_target(false, false)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KM: &str = "\
-- 見本のつもり
-- cian.set_keymap(\"j\", \"cursor_down\")
cian.set_keymap(\"l\", \"enter\")   -- 入る
";

    #[test]
    fn a_commented_binding_is_not_a_binding() {
        let got = keymaps_in(KM);
        assert_eq!(got, vec![("l".to_string(), "enter".to_string())], "{got:?}");
    }

    #[test]
    fn binding_an_action_that_has_none_adds_a_line() {
        let out = set_keymap_in(KM, "cursor_down", Some("j"));
        assert!(out.contains(crate::settings_edit::ADDED_HEAD), "{out}");
        assert!(out.contains("cian.set_keymap(\"j\", \"cursor_down\")"), "{out}");
        // 見本のコメントは残る。
        assert!(out.contains("-- cian.set_keymap(\"j\", \"cursor_down\")"), "{out}");
        assert_eq!(keymaps_in(&out).len(), 2);
    }

    #[test]
    fn rebinding_replaces_the_live_line() {
        let out = set_keymap_in(KM, "enter", Some("Ctrl+m"));
        assert!(out.contains("cian.set_keymap(\"Ctrl+m\", \"enter\")"), "{out}");
        assert!(!out.contains("\"l\", \"enter\""), "{out}");
        assert_eq!(keymaps_in(&out), vec![("Ctrl+m".to_string(), "enter".to_string())]);
    }

    /// **消すのは、コメントに戻すこと。** 行末の説明が消えない。
    #[test]
    fn unbinding_comments_the_line_out() {
        let out = set_keymap_in(KM, "enter", None);
        assert!(out.contains("-- cian.set_keymap(\"l\", \"enter\")   -- 入る"), "{out}");
        assert!(keymaps_in(&out).is_empty(), "{out}");
        assert_eq!(KM.lines().count(), out.lines().count());
    }

    /// 押して決めた綴りが、そのまま Lua になる。
    #[test]
    fn what_the_key_capture_produces_round_trips() {
        let mut out = String::new();
        for (key, action) in [("j", "cursor_down"), ("alt+g", "jump_path"), ("\"", "search")] {
            out = set_keymap_in(&out, action, Some(key));
            assert_eq!(crate::settings_edit::syntax_error(&out), None, "{out}");
        }
        let got = keymaps_in(&out);
        assert_eq!(got.len(), 3, "{out}");
        assert!(got.iter().any(|(k, a)| k == "alt+g" && a == "jump_path"), "{got:?}");
    }

    /// **どのファイルに書くか。** `ssh.lua` と同じ三つの規則。
    #[test]
    fn new_bindings_go_to_the_file_that_is_for_them() {
        assert_eq!(pick_target(false, false), "keymap.lua");
        assert_eq!(pick_target(true, false), "keymap.lua");
        assert_eq!(pick_target(false, true), "init.lua");
    }

    #[test]
    fn a_fresh_keymap_lua_explains_itself() {
        let head = keymap_head();
        assert!(head.contains("keymap.lua"), "{head}");
        // **既定を消すのではなく、先に取る**と書いてある。
        assert!(head.contains("既定のまま"), "{head}");
        assert!(head.contains(crate::settings_edit::ADDED_HEAD), "{head}");
        let out = set_keymap_in(&head, "cursor_down", Some("j"));
        assert_eq!(out.matches(crate::settings_edit::ADDED_HEAD).count(), 1, "{out}");
        assert_eq!(crate::settings_edit::syntax_error(&out), None, "{out}");
    }

    #[test]
    fn crlf_stays_crlf() {
        let text = KM.replace('\n', "\r\n");
        let out = set_keymap_in(&text, "enter", Some("x"));
        assert!(!out.contains("\n\n"), "{out:?}");
        assert_eq!(out.matches("\r\n").count(), text.matches("\r\n").count());
    }
}
