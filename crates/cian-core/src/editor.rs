//! Which external editor to start, and whether it is on `PATH`.
//!
//! **One answer for both front ends.** The terminal build has honoured
//! `cian.set_option("editor", …)` since it had one; the window's `:edit` asked
//! the engine, and the engine read only `$VISUAL` / `$EDITOR` — so the same
//! init.lua opened `code` in the terminal and `vi` in the window, silently.
//! Found on 2026-09-06 by `scripts/configcover.py`, which counts the settings
//! the engine sends that nobody acts on.
//!
//! The decision is here rather than in either front end for the usual reason:
//! a rule written twice is two rules that agree until one of them is edited.

use std::path::PathBuf;

/// The editor command, split into words, with no file argument.
///
/// An explicit editor is trusted as-is and in this order — **init.lua first**,
/// because writing it down is a stronger statement than an environment
/// variable inherited from whatever started cian. Then `$VISUAL`, then
/// `$EDITOR`, then the first of nvim → vim → vi that `found` reports.
///
/// `found` is passed in so the choosing can be tested without a `PATH`.
pub fn pick(
    configured: Option<&str>,
    visual: Option<&str>,
    editor_env: Option<&str>,
    found: impl Fn(&str) -> bool,
    beside: Option<&str>,
) -> Option<Vec<String>> {
    if let Some(cmd) = configured.or(visual).or(editor_env) {
        let words: Vec<String> = cmd.split_whitespace().map(|s| s.to_string()).collect();
        if !words.is_empty() {
            return Some(words);
        }
    }
    ["nvim", "vim", "vi"]
        .into_iter()
        .find(|n| found(n))
        .map(|n| vec![n.to_string()])
        .or_else(|| beside.map(|p| vec![p.to_string()]))
}

/// The vim sitting next to the executable, if one is there.
///
/// 会社の Windows には vim が無く、入れることもできない ── `:vim` が
/// 「PATH にありません」としか言えないのがそこだった。exe の隣に置いたものを
/// 見つけて、入っていない機械でも使えるようにする。設定ファイルと同じ
/// portable-first の置き方だ。
///
/// **PATH の vim には勝たせない。** 自分で vim を入れている人は、自分の
/// `_vimrc` が効くほうを使いたい。隣のものは「他に何も無いとき」の最後の手で、
/// [`pick`] でもその順に並べてある。
///
/// 配置は配布元の zip のまま ── `vim/vim92/vim.exe` のように、実行ファイルの
/// 隣にランタイム（`syntax`・`ftplugin`…）が並ぶ形にする。vim は自分の居場所
/// から `$VIM` を割り出すので、環境変数を立てずに済む。**版の数字は上がる**
/// ので名前で決め打ちせず、`vim/` の下から実行ファイルを持つ階を探す。
pub fn beside_exe() -> Option<PathBuf> {
    beside(std::env::current_exe().ok()?.parent()?)
}

/// `start` から上へ登って探す。[`beside_exe`] の、`current_exe` を要らない半分。
///
/// **隣から、上へ3階まで。** 窓版のエンジンは `resources/app/` に埋まって
/// いて、人が `vim/` を置く自然な場所（`cian.exe` の隣）はその2階上だ。
/// 端末版は `cian-tui.exe` の隣で1階目に当たる。**`vim/` の下に実行できる
/// vim がある**ことが条件なので、上まで登っても他人のものを拾わない。
pub fn beside(start: &std::path::Path) -> Option<PathBuf> {
    let mut here = start.to_path_buf();
    for _ in 0..3 {
        if let Some(found) = vim_in(&here.join("vim")) {
            return Some(found);
        }
        match here.parent() {
            Some(up) => here = up.to_path_buf(),
            None => break,
        }
    }
    None
}

/// `dir` の直下、または `dir/vim92/` のような階の vim。
pub fn vim_in(dir: &std::path::Path) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "vim.exe" } else { "vim" };
    if is_executable(&dir.join(exe)) {
        return Some(dir.join(exe));
    }
    // 読めない階は飛ばす ── ここで何も見つからなくても「同梱が無い」に
    // 落ちるだけで、PATH の vim は先に見終わっている。
    let mut kids: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    // 版が2つ並んでいたら、名前の大きいほう＝新しいほうを採る。
    kids.sort();
    kids.into_iter().rev().find(|d| is_executable(&d.join(exe))).map(|d| d.join(exe))
}

/// シェルに渡す1語。空白があれば括る。
///
/// **隣に置いた vim で初めて要るようになった。** これまでエディタは PATH 上の
/// 短い名前ばかりで、括っても括らなくても同じだった。`C:\Program Files\…` は
/// 括らなければ2語に割れ、まとめて括れば `code -w` が1つの名前になる。
/// 両前端が同じ形を作るよう、規則はここに1つだけ置く。
pub fn shell_word(w: &str) -> String {
    if w.contains(' ') {
        format!("\"{w}\"")
    } else {
        w.to_string()
    }
}

/// 名指しされたエディタの、実際に起動できる姿。
///
/// PATH にあればその名前。無ければ、`vi` と `vim` に限って[隣に置いたもの]
/// (beside_exe) を答える ── `nvim` は別のもので、名指ししたのと違うものが
/// 開くほうが悪い。どちらも無ければ `None`。
///
/// **両前端がここを通る。** 端末版は `edit_in_new_tab`、窓版は engine の
/// `whichedit` から。規則が二度書かれると、片方を直したときに割れる。
pub fn named(name: &str) -> Option<String> {
    let beside = beside_exe();
    let beside = beside.as_deref().and_then(std::path::Path::to_str);
    choose(name, on_path, beside)
}

/// [`named`] の、PATH もファイルシステムも要らない半分。[`pick`] と同じ形で
/// 切ってある ── **世界を触る側と、何を選ぶかを決める側は別**にしておかないと、
/// vim の入っている機械では同梱の枝を一度も通せない。
pub fn choose(name: &str, found: impl Fn(&str) -> bool, beside: Option<&str>) -> Option<String> {
    if found(name) {
        return Some(name.to_string());
    }
    if matches!(name, "vi" | "vim") {
        return beside.map(|p| p.to_string());
    }
    None
}

/// Is `name` an executable on `PATH`? On Windows the usual executable
/// extensions are tried too.
pub fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    let exts: &[&str] = if cfg!(windows) { &["", ".exe", ".cmd", ".bat"] } else { &[""] };
    std::env::split_paths(&path).any(|dir| {
        exts.iter().any(|ext| {
            let cand: PathBuf = dir.join(format!("{name}{ext}"));
            is_executable(&cand)
        })
    })
}

#[cfg(unix)]
fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &std::path::Path) -> bool {
    p.is_file()
}

/// The editor for these options, reading the environment for the two
/// variables. The half that needs a real process; [`pick`] is the judgement.
pub fn resolve(configured: Option<&str>) -> Option<Vec<String>> {
    let visual = std::env::var("VISUAL").ok().filter(|s| !s.trim().is_empty());
    let editor = std::env::var("EDITOR").ok().filter(|s| !s.trim().is_empty());
    let beside = beside_exe();
    let beside = beside.as_deref().and_then(std::path::Path::to_str);
    pick(configured, visual.as_deref(), editor.as_deref(), on_path, beside)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_down_wins_over_the_environment() {
        // init.lua first — and kept whole, arguments and all.
        let cmd = pick(Some("code -w"), Some("hx"), Some("nano"), |_| false, None);
        assert_eq!(cmd, Some(vec!["code".into(), "-w".into()]));
        assert_eq!(pick(None, Some("hx"), Some("nano"), |_| false, None), Some(vec!["hx".into()]));
        assert_eq!(pick(None, None, Some("nano"), |_| false, None), Some(vec!["nano".into()]));
    }

    #[test]
    fn with_nothing_said_it_is_the_first_one_on_the_path() {
        assert_eq!(pick(None, None, None, |n| n == "vim" || n == "vi", None), Some(vec!["vim".into()]));
        assert_eq!(pick(None, None, None, |n| n == "nvim" || n == "vim", None), Some(vec!["nvim".into()]));
        assert_eq!(pick(None, None, None, |n| n == "vi", None), Some(vec!["vi".into()]));
        // Nothing at all, and nothing is claimed.
        assert_eq!(pick(None, None, None, |_| false, None), None);
    }

    #[test]
    fn an_empty_setting_is_not_a_setting() {
        // `editor = ""` in init.lua must not become an empty command line.
        assert_eq!(pick(Some("   "), None, None, |n| n == "vi", None), Some(vec!["vi".into()]));
    }
    /// 会社の Windows に vim が無い機械の話。**PATH に何も無いときだけ**
    /// 隣のものに落ちる ── 自分で入れた vim があるなら、自分の `_vimrc` が
    /// 効くほうが勝つ。
    #[test]
    fn the_one_next_to_the_exe_is_the_last_resort() {
        let beside = Some("C:/cian/vim/vim92/vim.exe");
        // PATH に何も無い ── 隣のものを使う
        assert_eq!(
            pick(None, None, None, |_| false, beside),
            Some(vec!["C:/cian/vim/vim92/vim.exe".into()])
        );
        // PATH に vim がある ── そちらが勝つ
        assert_eq!(
            pick(None, None, None, |n| n == "vim", beside),
            Some(vec!["vim".into()])
        );
        // init.lua で名指ししてある ── いちばん強い
        assert_eq!(
            pick(Some("code -w"), None, None, |_| false, beside),
            Some(vec!["code".into(), "-w".into()])
        );
        // 隣にも無ければ、今までどおり「見つかりません」
        assert_eq!(pick(None, None, None, |_| false, None), None);
    }
    /// 配布元の zip は `vim/vim92/vim.exe` という形で、版の数字は上がる。
    /// 名前で決め打ちせずに探すこと、そして**平らに置かれていても**拾うこと。
    #[cfg(unix)]
    #[test]
    fn the_bundled_vim_is_found_whatever_the_version_directory_is_called() {
        use std::os::unix::fs::PermissionsExt;
        let put = |dir: &std::path::Path| {
            std::fs::create_dir_all(dir).unwrap();
            let exe = dir.join("vim");
            std::fs::write(&exe, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
            exe
        };
        // 版の階に入っている
        let t = tempfile::tempdir().unwrap();
        let want = put(&t.path().join("vim").join("vim92"));
        assert_eq!(vim_in(&t.path().join("vim")), Some(want));

        // 平らに置いてある
        let t = tempfile::tempdir().unwrap();
        let want = put(&t.path().join("vim"));
        assert_eq!(vim_in(&t.path().join("vim")), Some(want));

        // 版が2つ ── 新しいほうを採る
        let t = tempfile::tempdir().unwrap();
        put(&t.path().join("vim").join("vim91"));
        let want = put(&t.path().join("vim").join("vim92"));
        assert_eq!(vim_in(&t.path().join("vim")), Some(want));

        // 階はあるが中身が無い ── 「同梱が無い」に落ちる
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("vim").join("vim92")).unwrap();
        assert_eq!(vim_in(&t.path().join("vim")), None);
        assert_eq!(vim_in(&t.path().join("nothing-here")), None);
    }
    /// **窓版のエンジンは `resources/app/` に埋まっている。** 人が置くのは
    /// `cian.exe` の隣で、そこはエンジンから見て2階上だ。ここが届かないと、
    /// 端末版だけ同梱が効いて窓版だけ効かない、という形になる。
    #[cfg(unix)]
    #[test]
    fn the_window_engine_finds_a_vim_two_floors_above_it() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let top = t.path().join("cian");
        let app = top.join("resources").join("app");
        std::fs::create_dir_all(&app).unwrap();
        // 人は cian.exe の隣に置く
        let vimdir = top.join("vim").join("vim92");
        std::fs::create_dir_all(&vimdir).unwrap();
        let exe = vimdir.join("vim");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(beside(&app), Some(exe.clone()), "窓版のエンジンから2階上");
        assert_eq!(beside(&top), Some(exe), "端末版は隣で当たる");
        // 4階上は見ない ── 登りすぎて他人のものを拾わないこと
        let deep = app.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(beside(&deep), None, "3階までで止まる");
    }
    /// 名指しされたときの落ち方。**`nvim` は落とさない** ── 名指ししたのと
    /// 違うものが開くほうが、開かないより悪い。
    #[test]
    fn a_named_editor_falls_back_to_the_bundled_one_only_when_it_is_vim() {
        let beside = Some("C:/cian/vim/vim92/vim.exe");
        let none: fn(&str) -> bool = |_| false;
        assert_eq!(choose("vim", none, beside), Some("C:/cian/vim/vim92/vim.exe".into()));
        assert_eq!(choose("vi", none, beside), Some("C:/cian/vim/vim92/vim.exe".into()));
        assert_eq!(choose("nvim", none, beside), None, "nvim は vim ではない");
        // PATH にあればそちらが勝つ ── 自分の _vimrc が効くほう
        assert_eq!(choose("vim", |n| n == "vim", beside), Some("vim".into()));
        // 隣にも無ければ断る
        assert_eq!(choose("vim", none, None), None);
    }

    /// 空白のある実行ファイル名。隣に置いた vim が `C:\\Program Files\\…`
    /// に来たときだけ効く ── **両前端が同じ括り方をすること。**
    #[test]
    fn a_word_with_a_space_in_it_is_quoted() {
        assert_eq!(shell_word("vim"), "vim");
        assert_eq!(shell_word("C:/Program Files/v/vim.exe"), "\"C:/Program Files/v/vim.exe\"");
    }
}