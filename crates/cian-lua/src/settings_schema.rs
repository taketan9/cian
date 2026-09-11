//! 設定画面に並べる項目の表。
//!
//! **手で並べない。** crmaine の設定画面は `package.json` の
//! `contributes.configuration` から項目を作る ── 手で並べると、設定を足した日に
//! 取り残されるからだ（あちらでは `targetExtensions` が3箇所で食い違い、
//! リーダーは対応しているのに索引されない形式が生まれた）。
//!
//! cian に `package.json` に当たるものは無く、Rust は doc コメントを実行時に
//! 読めない。だから表はここに手で書く ── **そのかわり、`Options` の項目と
//! 1対1であることを検査が見る**（下の `every_option_has_a_row`）。増えたら
//! 落ちるので、取り残されない。
//!
//! ## 言葉も、ここに両方持つ
//!
//! 窓版の文言はふつう `tr()` を通すが、この表は**両方の言葉をここに置く**。
//! 項目の名前・説明・分類を2つのファイルに分けると、片方だけ直したときに
//! ずれる ── crmaine が1つのファイルに全部置いているのと同じ理由。

/// 何を入れる欄か。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// 自由な文字列。
    Text,
    /// ディレクトリやコマンド。窓版は「参照」を添える。
    Path,
    /// 入／切。
    Bool,
    /// 数。
    Int,
    /// いくつかの中から1つ。`(init.lua に書く値, 画面に出す名前)`。
    ///
    /// 二つに分けたのは**シェル**のため ── 画面に出すのは
    /// 「Windows PowerShell」で、`init.lua` に書くのは `powershell.exe`。
    /// 同じにできるもの（`classic`、`vim`）は両方に同じ字を書く。
    Choice(&'static [(&'static str, &'static str)]),
    /// 文字列の並び（`{ "bak", "dmp" }`）。
    List,
}

/// どちらの前端で効くか。
///
/// **効かない設定を、効くふりをして並べない。** この家で最頻のバグは
/// 「設定を直したのに効かない」で、設定画面は放っておくとその水源になる ──
/// `Options` の全部を並べれば、窓版が読みもしない `nerd_fonts` にも欄が付く。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Applies {
    /// 両方。
    Both,
    /// 端末版だけ。窓版は書体も枠も自前なので、届いても使い道が無い。
    Tui,
    /// 窓版だけ。端末版には「一覧の形」という選択肢が無い。
    Gui,
}

/// 設定画面の1項目。
#[derive(Debug, Clone, Copy)]
pub struct Field {
    /// `cian.set_option("…")` に渡す名前。
    pub name: &'static str,
    pub kind: Kind,
    /// 既定が**一つの値に決まる**なら、その値。
    ///
    /// 決まるなら、選ぶ欄には**その値を選んだ状態で出す** ──「（既定）」と
    /// 書くのをやめた（2026-09-11、本人）。既定が classic と分かっている
    /// ものを「（既定）」と書くのは、**答えを持っているのに黙っている**のと
    /// 同じ。決まらないもの（枠の角は端末ごと、メニューの言葉は画面に従う）は
    /// `None` で、そのとき先頭に出るのは `default_ja` の**言葉**になる。
    pub default_value: Option<&'static str>,
    /// どちらの前端で効くか。
    pub applies: Applies,
    /// **何も書かなかったときどうなるか**を、人の言葉で。値ではない ──
    /// `home` の既定は「デスクトップ、無ければ作業ディレクトリ」で、
    /// 字面にできない。
    pub default_en: &'static str,
    pub default_ja: &'static str,
    pub category_en: &'static str,
    pub category_ja: &'static str,
    pub label_en: &'static str,
    pub label_ja: &'static str,
    pub help_en: &'static str,
    pub help_ja: &'static str,
}

/// 画面に出す順。**分類ごとにまとまっている** ── 45 項目をフラットに並べない
/// のが crmaine の教訓で、こちらは 20 項目だが同じことをする。
pub fn fields() -> &'static [Field] {
    &[
        // ── 表示 ──
        Field {
            name: "view",
            default_value: Some("classic"),
            applies: Applies::Gui,
            kind: Kind::Choice(&[("classic", "classic"), ("details", "details")]),
            default_en: "classic (two panes)",
            default_ja: "classic（2画面）",
            category_en: "Look",
            category_ja: "表示",
            label_en: "Display mode",
            label_ja: "表示モード",
            help_en: "classic is the two-pane file manager with a terminal. details is one pane at a time",
            help_ja: "classic は2画面ファイラ・ターミナルです。details は1画面ずつ表示します",
        },
        Field {
            name: "borders",
            default_value: None,
            applies: Applies::Tui,
            kind: Kind::Choice(&[("rounded", "rounded"), ("plain", "plain")]),
            default_en: "chosen per terminal",
            default_ja: "端末ごとに自動",
            category_en: "Look",
            category_ja: "表示",
            label_en: "Frame corners",
            label_ja: "枠の角",
            help_en: "plain on the legacy Windows console, which has no rounded glyphs",
            help_ja: "古い Windows コンソールには丸い角の文字が無いので plain になります",
        },
        Field {
            name: "nerd_fonts",
            default_value: Some("false"),
            applies: Applies::Tui,
            kind: Kind::Bool,
            default_en: "off",
            default_ja: "切",
            category_en: "Look",
            category_ja: "表示",
            label_en: "Nerd Font glyphs",
            label_ja: "Nerd Font の記号",
            help_en: "file-type icons and the branch/disk symbols. off by default: most terminals have no Nerd Font",
            help_ja: "ファイル種別のアイコンや枝・ディスクの記号です。既定は切です。持っていない環境のほうが多いためです",
        },
        Field {
            name: "key_hints",
            default_value: Some("true"),
            applies: Applies::Both,
            kind: Kind::Bool,
            default_en: "on",
            default_ja: "入",
            category_en: "Look",
            category_ja: "表示",
            label_en: "Key hints",
            label_ja: "キーヒントの表示",
            help_en: "the row of keys above the status line",
            help_ja: "状態行の上に出る、いま押せるキーの並びです",
        },
        Field {
            name: "show_hidden",
            default_value: Some("true"),
            applies: Applies::Both,
            kind: Kind::Bool,
            default_en: "on",
            default_ja: "入",
            category_en: "Look",
            category_ja: "表示",
            label_en: "Dotfiles",
            label_ja: "隠しファイルの表示",
            help_en: "T and :hidden flip it while cian runs",
            help_ja: "動かしている間は T と :hidden で切り替えられます",
        },
        Field {
            name: "animation_ms",
            default_value: None,
            applies: Applies::Tui,
            kind: Kind::Int,
            default_en: "on, a short transition",
            default_ja: "入（短い動き）",
            category_en: "Look",
            category_ja: "表示",
            label_en: "Transition length (ms)",
            label_ja: "画面の動きの長さ（ミリ秒）",
            help_en: "split, zoom and close. 0 turns the motion off",
            help_ja: "分割・ズーム・閉じるの動きです。0 で動きません",
        },
        // ── ファイル ──
        Field {
            name: "home",
            default_value: None,
            applies: Applies::Both,
            kind: Kind::Path,
            default_en: "the Desktop, then the working directory",
            default_ja: "デスクトップ、無ければ作業ディレクトリ",
            category_en: "Files",
            category_ja: "ファイル",
            label_en: "Directory to open in",
            label_ja: "起動時に開く場所",
            help_en: "used when cian is started with no path. ~ and $VAR expand",
            help_ja: "パスを付けずに起動したときの場所です。~ と $VAR は展開されます",
        },
        Field {
            name: "shell",
            default_value: None,
            applies: Applies::Both,
            // **プルダウンにした**（2026-09-11、本人）。自由入力だと、綴りを
            // 間違えたシェルが黙って起動しないだけになる ──「パスはデフォルト
            // パスでいい」とのことなので、値は素のコマンド名で持つ。
            //
            // 既定は `None` のまま ── **Mac で「Windows PowerShell」が既定に
            // 見えてはいけない**。何も書かなければ、その OS の既定のシェルが
            // 起動する（決めるのはエンジン）。並びの先頭が PowerShell なのは、
            // 会社の機械がそれだから。
            kind: Kind::Choice(&[
                ("powershell.exe", "Windows PowerShell"),
                ("pwsh.exe", "PowerShell 7"),
                ("cmd.exe", "コマンドプロンプト"),
                ("bash", "bash"),
                ("sh", "sh"),
                ("fish", "fish"),
                ("zsh", "zsh"),
            ]),
            default_en: "the system shell",
            default_ja: "OS の既定のシェル",
            category_en: "Files",
            category_ja: "ファイル",
            label_en: "Default shell",
            label_ja: "デフォルトシェル",
            help_en: "run in the shell panel. the name is looked up on PATH",
            help_ja: "シェル枠で動かすものです。名前は PATH から探します",
        },
        Field {
            name: "read_cloud_files",
            default_value: Some("false"),
            applies: Applies::Both,
            kind: Kind::Bool,
            default_en: "off",
            default_ja: "切",
            category_en: "Files",
            category_ja: "ファイル",
            label_en: "Include cloud files",
            label_ja: "クラウドファイルを対象とするか",
            help_en: "grep, :count, :hash and :dupes download the file to read it",
            help_ja: "grep・:count・:hash・:dupes が、読むために実際にダウンロードします",
        },
        Field {
            name: "preview",
            default_value: Some("true"),
            applies: Applies::Both,
            kind: Kind::Bool,
            default_en: "on",
            default_ja: "入",
            category_en: "Files",
            category_ja: "ファイル",
            label_en: "Cursor-follow preview",
            label_ja: "カーソル追従プレビュー",
            help_en: "the shell panel shows whatever the cursor is on",
            help_ja: "シェル枠に、カーソルの下のファイルを出します",
        },
        Field {
            name: "preview_skip",
            default_value: None,
            applies: Applies::Both,
            kind: Kind::List,
            default_en: "for example: vsix, tar",
            default_ja: "例: vsix, tar",
            category_en: "Files",
            category_ja: "ファイル",
            label_en: "Extensions the preview leaves alone",
            label_ja: "プレビュー対象外拡張子",
            help_en: "added to the forty-odd kinds cian already leaves alone (vsix, iso, pdf). no dot, case ignored",
            help_ja: "cian が初めから対象外にしている40種ほど（vsix・iso・pdf など）に足します。点は付けません。大文字小文字は問いません",
        },
        // ── 編集 ──
        Field {
            name: "edit_style",
            default_value: Some("vim"),
            applies: Applies::Both,
            kind: Kind::Choice(&[("vim", "vim"), ("notepad", "notepad")]),
            default_en: "vim",
            default_ja: "vim",
            category_en: "Editing",
            category_ja: "編集",
            label_en: "Editor keys",
            label_ja: "エディタのキー操作",
            help_en: "notepad is for handing the same build to someone who has never used vi",
            help_ja: "notepad は、vi を使ったことがない人に同じものを渡すためのものです",
        },
        Field {
            name: "editor",
            default_value: None,
            applies: Applies::Both,
            kind: Kind::Path,
            default_en: "$VISUAL, $EDITOR, then nvim / vim / vi",
            default_ja: "$VISUAL・$EDITOR、無ければ nvim / vim / vi",
            category_en: "Editing",
            category_ja: "編集",
            label_en: "External editor",
            label_ja: "外部エディタ",
            help_en: "a command line, for example nvim or code -w",
            help_ja: "コマンド行で書きます。たとえば nvim、code -w です",
        },
        Field {
            name: "tab_width",
            default_value: None,
            applies: Applies::Both,
            kind: Kind::Int,
            default_en: "4",
            default_ja: "4",
            category_en: "Editing",
            category_ja: "編集",
            label_en: "Tab width",
            label_ja: "タブの桁数",
            help_en: "4 or 8 is the recommendation for tab-separated data",
            help_ja: "タブ区切りのデータは4または8を推奨",
        },
        // ── 転送 ──
        Field {
            name: "transfer_limit",
            default_value: None,
            applies: Applies::Both,
            kind: Kind::Text,
            default_en: "as fast as the link allows",
            default_ja: "回線の速さのまま",
            category_en: "Transfers",
            category_ja: "転送",
            label_en: "Speed ceiling",
            label_ja: "転送速度の上限",
            help_en: "2M, 500k, 1.5MB/s",
            help_ja: "2M・500k・1.5MB/s のように書きます",
        },
        Field {
            name: "verify_transfers",
            default_value: Some("false"),
            applies: Applies::Both,
            kind: Kind::Bool,
            default_en: "off",
            default_ja: "切",
            category_en: "Transfers",
            category_ja: "転送",
            label_en: "Verify after transfer",
            label_ja: "転送後にベリファイ",
            help_en: "reads the file back and compares checksums. needs SFTP, and doubles the reading",
            help_ja: "送ったあと読み直してチェックサムを照合します。SFTP が要り、読む量は倍になります",
        },
        // ── 通知 ──
        Field {
            name: "notify",
            default_value: Some("true"),
            applies: Applies::Both,
            kind: Kind::Bool,
            default_en: "on",
            default_ja: "入",
            category_en: "Notifications",
            category_ja: "通知",
            label_en: "Tell me when a long job finishes",
            label_ja: "時間のかかる処理が終わったら知らせる",
            help_en: "a bell and a desktop notification, when cian is not the window in front",
            help_ja: "ベルとデスクトップ通知です。cian が前面に無いときに鳴ります",
        },
        Field {
            name: "notify_min_secs",
            default_value: None,
            applies: Applies::Both,
            kind: Kind::Int,
            default_en: "5",
            default_ja: "5",
            category_en: "Notifications",
            category_ja: "通知",
            label_en: "Notification threshold (seconds)",
            label_ja: "完了通知の閾値",
            help_en: "notifies when a job runs longer than this",
            help_ja: "対象秒数を超過した場合に通知します",
        },
        // ── 言葉 ──
        Field {
            name: "lang",
            default_value: Some("ja"),
            applies: Applies::Both,
            kind: Kind::Choice(&[("ja", "ja（日本語）"), ("en", "en（English）")]),
            default_en: "ja",
            default_ja: "ja（日本語）",
            category_en: "Language",
            category_ja: "言葉",
            label_en: "Interface language",
            label_ja: "共通言語設定",
            help_en: "sets the language of the interface",
            help_ja: "画面の言語設定を変更します",
        },
        Field {
            name: "menu_lang",
            default_value: None,
            applies: Applies::Both,
            kind: Kind::Choice(&[("ja", "ja（日本語）"), ("en", "en（English）")]),
            default_en: "follows the interface language",
            default_ja: "画面の言葉に従う",
            category_en: "Language",
            category_ja: "言葉",
            label_en: "Manual / help language",
            label_ja: "マニュアル / ヘルプ言語設定",
            help_en: "changes the language of the manual and help only",
            help_ja: "マニュアル / ヘルプの言語のみ変更します",
        },
    ]
}

/// 画面が受け取った値を、`init.lua` に書く **Lua の字面**にする。
///
/// `set_option_in` が字面を要るのは、ここで型を推し量らせないため ──
/// `"true"` という**文字列**を真偽値に化けさせない。空は「書かない」なので
/// `None`。
pub fn to_lua(kind: Kind, value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    Some(match kind {
        Kind::Bool => {
            if matches!(v, "true" | "1" | "on") { "true".into() } else { "false".into() }
        }
        Kind::Int => v.parse::<i64>().ok()?.to_string(),
        Kind::List => {
            let items: Vec<String> = v
                .split(['\n', ','])
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| format!("\"{}\"", escape(s)))
                .collect();
            if items.is_empty() {
                return None;
            }
            format!("{{ {} }}", items.join(", "))
        }
        Kind::Text | Kind::Path | Kind::Choice(_) => format!("\"{}\"", escape(v)),
    })
}

/// Lua の文字列に入れられる形に。**バックスラッシュを先に。**
/// 順を逆にすると、自分が足した `\"` の `\` をもう一度escapeする。
/// Windows のパス（`C:\Users\…`）が毎回ここを通る。
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **表と `Options` が1対1であること。**
    ///
    /// 手で並べた表は、設定を足した日に取り残される ── crmaine がそこで
    /// 転けたので、こちらは機械で数える。Rust は doc コメントを実行時に
    /// 読めないので、`lib.rs` の字を読む（マニュアルと README を縛っている
    /// 端末版のテストと同じ手）。
    #[test]
    fn every_option_has_a_row_and_no_row_is_invented() {
        let src = include_str!("lib.rs");
        let start = src.find("pub struct Options {").expect("the Options struct");
        let body = &src[start..start + src[start..].find("\n}").expect("its end")];
        let mut in_struct: Vec<&str> = Vec::new();
        // 見出しの行（`pub struct Options {`）も `pub ` で始まる。項目は
        // `pub 名前: 型,` の形だけ ── 名前に空白は入らない。
        for line in body.lines() {
            let t = line.trim();
            let Some(rest) = t.strip_prefix("pub ") else { continue };
            let Some((name, _)) = rest.split_once(':') else { continue };
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                in_struct.push(name);
            }
        }
        in_struct.sort_unstable();
        assert!(in_struct.len() >= 15, "読めていない: {in_struct:?}");

        let mut in_table: Vec<&str> = fields().iter().map(|f| f.name).collect();
        in_table.sort_unstable();

        let missing: Vec<_> = in_struct.iter().filter(|n| !in_table.contains(n)).collect();
        let invented: Vec<_> = in_table.iter().filter(|n| !in_struct.contains(n)).collect();
        assert!(missing.is_empty(), "設定画面に出ない設定があります: {missing:?}");
        assert!(invented.is_empty(), "Options に無いものが表にあります: {invented:?}");
    }

    /// 型のとおりの字面になる。**`"true"` を真偽値に化けさせない。**
    #[test]
    fn values_become_the_lua_they_should() {
        assert_eq!(to_lua(Kind::Bool, "true").as_deref(), Some("true"));
        assert_eq!(to_lua(Kind::Bool, "いいえ").as_deref(), Some("false"));
        assert_eq!(to_lua(Kind::Int, "8").as_deref(), Some("8"));
        assert_eq!(to_lua(Kind::Text, "true").as_deref(), Some("\"true\""));
        assert_eq!(to_lua(Kind::List, "bak, dmp").as_deref(), Some("{ \"bak\", \"dmp\" }"));
        assert_eq!(to_lua(Kind::List, "bak\ndmp").as_deref(), Some("{ \"bak\", \"dmp\" }"));
        // 空は「書かない」。
        assert_eq!(to_lua(Kind::Text, "   "), None);
        assert_eq!(to_lua(Kind::Int, "yes"), None);
    }

    /// **Windows のパスが毎回ここを通る。**
    #[test]
    fn a_windows_path_survives_the_quoting() {
        let lua = to_lua(Kind::Path, r"C:\Users\you\Desktop").expect("a value");
        assert_eq!(lua, r#""C:\\Users\\you\\Desktop""#);
        // 引用符も。
        assert_eq!(to_lua(Kind::Text, "a\"b").as_deref(), Some(r#""a\"b""#));
    }

    /// 分類は、表に並べた順のままひとまとまりで出る。
    #[test]
    fn categories_are_not_scattered() {
        let mut seen: Vec<&str> = Vec::new();
        for f in fields() {
            if seen.last() != Some(&f.category_ja) {
                assert!(!seen.contains(&f.category_ja), "分類が飛んでいます: {}", f.category_ja);
                seen.push(f.category_ja);
            }
        }
        assert!(seen.len() >= 4, "{seen:?}");
    }
}
