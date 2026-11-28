#!/usr/bin/env python3
"""画面に出る**言葉**の質。文言そのものを、毎回同じ基準で見る。

`i18n.py` は「両方の言葉で言えるか」を数え、`widths.py` は「桁で測っているか」
を数えます。**どちらも中身は見ていません。** 両方の言葉で言えて、桁も合って
いて、それでも読んだ人が身構える文はあります。2026-09-10 に本人から出た指摘は
そこでした（「トグルの補足文の日本語が AI 的」）。

    python3 scripts/kotoba.py          # 項目ごとの件数
    python3 scripts/kotoba.py --list   # 一行ずつ

## 基準は `tr()` の doc コメントにある

規則は誰かの好みではなく、`crates/cian-tui/src/theme.rs` の `tr()` に
書いてあります（英語は小文字始まり・日本語は敬体・末尾に句点を打たない・
**ダッシュでつながず2文にする**・「今のところ」と言わない）。**書いてあるのに
機械が見ていなかったのは、そのうち句点だけ**でした
（`nothing_cian_says_ends_in_a_full_stop`）。ここは残りを見ます。

## 天井で見る（`i18n.py` の `TUI_CEILING` と同じ作法）

①と②は**いま多い**ので、0 を要求すると初日から落ちて読まれなくなります。
`audit.py` が一度「当てにならない指摘が7件」で読まれなくなった形です。だから
**いまの数を天井として書き、増えたら落とす**。減らすのは触った箇所から少しずつ。
③だけは 0 が天井の監視で、**入り込んだ日に失敗します**。

## 項目① 英語しか言えない知らせ

**端末版の既定の言語は日本語です**（`Lang::from_opt` ── 設定が無ければ日本語）。
それなのに `self.message` と `Popup::Notice` の文字列には、`tr()` を通らず
英語だけのものが並んでいます。`copy failed: {}`、`backup failed, not saving`、
`no password or key set for {}@{}` ── **失敗の知らせほどここに多い**。

`i18n.py` は逆向き（日本語しか言えないもの）しか数えていないので、これは
**どの基準にも映っていませんでした**。

数え方: `self.message = Some(…)` と `Popup::Notice { lines: […] }` の
**釣り合った括弧の中だけ**を見て、日本語も `tr(` も無く、英字2語以上の
リテラルがあるもの。±数行の窓で見ると近くの doc コメントを拾うので、
式の中だけを読みます。

## 項目② ダッシュでつないだ一文

`tr()` の doc がいちばん長く説明している規則です:

> **Two sentences, not a dash.** State what happened, then what can be done
> about it. `unsaved changes — Ctrl+S saves` reads as one breathless thought;
> `unsaved changes. Ctrl+S saves` is two clear ones.

規則が書かれたのは 2026-08-22（`9418e5a`）で、**今日まで一度も機械が見て
いません**。doc が「悪い例」として挙げている
`unsaved changes — Ctrl+S saves` は、`mouse.rs` にその日本語版がそのまま
生きています（「未保存の変更があります — :w で保存、:q! で破棄」）。

**ダッシュが全部だめではありません。** 題（「 コミットメッセージ生成 — 編集中 」）
のように、二つの文ではなく**一つの名前の中の区切り**として使うものがあります。
それは `WAIVED` に理由つきで書きます。消すのではなく、なぜ指摘しないのかを
残す（`audit.py` の `TERM_OK` と同じ）。

## 項目③ 型どおりの言い回し

「AI っぽい」の正体は、**書き手が一度も声に出していない言い回し**です。
いまは 0 件 ── だからこの項目は**監視**で、天井は 0。増えた日に失敗します。

**辞書に足すときは変異テストを。** 「指摘が消えた」ではなく「検査が何も報告しなかった」を
cian では5回やっています（`keycover.py` が 72 → 2 種に落ちたまま百分率を
出し続けた件）。壊した版で失敗することを確かめてから戻すこと。
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TUI = ROOT / "crates" / "cian-tui" / "src"
JS = ROOT / "gui" / "renderer.js"
# 設定画面が読み上げる表。**ここもユーザが読む画面**なのに、長いあいだこの
# 検査の外だった。2026-09-12、「窓の見た目」がここに残っていて、④が
# 報告せずに通していた。拾うのは `*_ja:` の一本道だけで、テストの文字列や
# Lua の見本は入らない。
LUA = ROOT / "crates" / "cian-lua" / "src"
LUA_FILES = ("settings_schema.rs", "settings_keymap.rs")
# 窓版が画面に出す断りの言葉の**半分はエンジンが書いている。** `say()` に
# そのまま流れるので、これもユーザが読む画面。`zip へはコピー（追加）のみ`
# は端末版と窓版の両方で直したのに、ここに三つ目のコピーが残っていた
# （2026-09-12）。**入口は `bail!` / `anyhow!` / `format!` の三つだけ**にして、
# 検査の中の文字列は入れない。
SRV = ROOT / "crates" / "cian-server" / "src"

JA = re.compile(r"[぀-ヿ一-鿿]")
WORD = re.compile(r"[A-Za-z]{2,}")
JA = re.compile(r"[ぁ-んァ-ヶ一-龠]")
RS_LIT = re.compile(r'"((?:[^"\\\n]|\\.)*)"')
JA_FIELD = re.compile(
    r'\b(?:default|category|label|help|what|how)_ja\s*:\s*"((?:[^"\\\n]|\\.)*)"')
JS_LIT = re.compile(r"'((?:[^'\\\n]|\\.)*)'|\"((?:[^\"\\\n]|\\.)*)\"|`((?:[^`\\]|\\.)*)`")

# ── 天井 ──────────────────────────────────────────────────────────────
#
# **増えたら落ちます。減らすのは別の日。** 直したら、その日の数まで下げること
# （下げ忘れると、直したぶんの余白でまた増やせてしまう）。
ENGLISH_ONLY_CEILING = 2  # 端末版。既定が日本語なのに英語しか言えない知らせ
#   103 → 102（2026-09-11、`:key` の案内を tr() に通した）。**直したら天井も
#   その日の数まで下げる。** 下げ忘れると、直したぶんの余白でまた増やせる
#   102 → 100（2026-09-14、`:theme` の「覚えました／覚えられません」を
#   日本語でも言えるようにした。触った箇所から少しずつ）
#   38 → 2（2026-11-28、同じ日の続き）。**残す2本には理由がある**:
#   `chmod {arg}` は操作の名前（コマンド名そのもの）で、文ではない。
#   `key={:?} mods={:?} alt_screen={}` は `:keys` の診断で、**そのまま貼って
#   もらうためのもの**。訳すと貼られた字が読めなくなる
#   100 → 38（2026-11-28）。**59 件をまとめて両方の言葉にした。** クリップ
#   ボード・コピー・書庫・閲覧・保存・git/svn・リモートペイン・`:tar` などの
#   使い方案内。**端末版を毎日使う人の言葉は日本語**なので、失敗の知らせが
#   英語のままなのは窓版の裏返しの同じ不具合だった。残りは案内文と記録用
ENGLISH_ONLY_JS_CEILING = 0  # 窓版。片付いた（2026-11-28、`:view` の使い方案内が最後の1本）
DASH_CEILING_RS = 0        # 端末版。`tr()` の両方の言葉ぶんを数えるので、報せの数のほぼ倍
#   124 → 125（2026-09-12）。**文章が増えたのではなく、見る場所が増えた** ──
#   エンジン（`cian-server`）の断りの言葉を読むようにしたら、そこに1本あった
#   （`中身が違います — 手元 … ≠ 向こう …`）。数え上げの天井を上げるのは
#   これだけが理由で、書き手が増やしたぶんは**一つも入っていない**
DASH_CEILING_JS = 0        # 窓版
#   156 → 155（2026-09-12）。`vim（既定）／ メモ帳 ── …` を `notepad / vim` に
#   書き直したときにダッシュが1本減った。**直したら天井も下げる**
# 硬い言い方。**ここは 0 を目指す。** 数が少なく、直し方が一つに決まる。
STIFF_CEILING = 0


def strip_line_comments(text: str) -> str:
    """行頭が `//` の行だけ落とす。**賢くしない。** 文字列の中の `//` を
    消そうとして `audit.py` が一度 `'http://'` を壊している。"""
    return "\n".join("" if l.lstrip().startswith("//") else l for l in text.splitlines())


def balanced(text: str, i: int, oc: str, cc: str) -> int:
    """`i` の括弧に釣り合う閉じ括弧の位置。文字列の中の括弧は数えない。"""
    depth, j = 0, i
    while j < len(text):
        c = text[j]
        if c in "\"'`":
            q, j = c, j + 1
            while j < len(text) and text[j] != q:
                if text[j] == "\\":
                    j += 1
                j += 1
        elif c == oc:
            depth += 1
        elif c == cc:
            depth -= 1
            if depth == 0:
                return j
        j += 1
    return len(text) - 1


# ── 画面に出る文字列 ──────────────────────────────────────────────────
#
# **入口は1つにまとめる。** ②と③が「どこを見るか」で割れると、片方だけ直したときに
# もう片方が何も報告しません。定義はここ1つ:
#
#   * `tr(…)` の引数（両前端。端末版の固定文言はすべてここを通る）
#   * `self.message = Some(…)` / `Popup::Notice { lines: […] }`（端末版）
#   * `say(…)`（窓版）
#
# `cian-core` は画面を持たないので見ません。あそこの英文は `substitute.rs`
# の使い方案内のように、端末版が `tr()` に包み直して出すか、出ないかのどちらか。
def unescape_dashes(text: str) -> str:
    """`\\u2014` のようなエスケープを、その文字に戻す。

    **この検査には盲点があった。** 2026-11-28、画面の文言のダッシュを 0 件に
    したあとも、`gui/renderer.js` には `\\u2014` と書かれたダッシュが 34 本
    残っていた。日本語側だけが直り、英語側はそのままだった。ソースの字面では
    `—` に見えないため、この検査が一度も指摘していなかった。
    """
    return re.sub(r"\\u(2014|2015|2500|2501)",
                  lambda m: chr(int(m.group(1), 16)), text)


def screen_strings():
    out = []
    for path in sorted(TUI.glob("*.rs")):
        if path.name == "tests.rs":
            continue
        rel = f"crates/cian-tui/src/{path.name}"
        text = strip_line_comments(unescape_dashes(path.read_text(encoding="utf-8")))
        for start, end in _outermost(_call_spans(text, r"\btr\(") + _msg_spans(text)):
            for m in RS_LIT.finditer(text[start : end + 1]):
                out.append((rel, text[:start].count("\n") + 1, m.group(1)))
    text = strip_line_comments(unescape_dashes(JS.read_text(encoding="utf-8")))
    for start, end in _outermost(_call_spans(text, r"\btr\(") + _call_spans(text, r"\bsay\(")):
        for m in JS_LIT.finditer(text[start : end + 1]):
            lit = next((g for g in m.groups() if g is not None), "")
            out.append(("gui/renderer.js", text[:start].count("\n") + 1, lit))
    for path in sorted(SRV.glob("*.rs")):
        rel = f"crates/cian-server/src/{path.name}"
        text = strip_line_comments(unescape_dashes(path.read_text(encoding="utf-8")))
        spans = _outermost(
            _call_spans(text, r"\bbail!\(")
            + _call_spans(text, r"\banyhow!\(")
            + _call_spans(text, r"\bformat!\(")
        )
        for start, end in spans:
            for m in RS_LIT.finditer(text[start : end + 1]):
                if JA.search(m.group(1)):
                    out.append((rel, text[:start].count("\n") + 1, m.group(1)))
    for name in LUA_FILES:
        rel = f"crates/cian-lua/src/{name}"
        text = strip_line_comments(unescape_dashes((LUA / name).read_text(encoding="utf-8")))
        for m in JA_FIELD.finditer(text):
            out.append((rel, text[: m.start()].count("\n") + 1, m.group(1)))
    return out


def _outermost(spans):
    """入れ子は外側だけ。`self.message = Some(tr(...))` は一つの報せで、
    `tr(` の span と `Some(` の span で**同じ文字列を二度数える**。
    数を数える検査で二度数えると、天井が実際の倍を許してしまう。"""
    spans = sorted(set(spans))
    return [s for s in spans if not any(a <= s[0] and s[1] <= b for a, b in spans if (a, b) != s)]


def _call_spans(text, pattern):
    spans = []
    for m in re.finditer(pattern, text):
        start = m.end() - 1
        spans.append((start, balanced(text, start, "(", ")")))
    return spans


def _msg_spans(text):
    spans = []
    for m in MSG_START.finditer(text):
        if m.group(0).rstrip().endswith("("):
            start = m.end() - 1
            spans.append((start, balanced(text, start, "(", ")")))
        else:
            # `lines:` の**すぐ次**が `[` のときだけ。離れた `[` を拾うと、
            # 無関係な行のリテラルまで一つの報せとして数える。`lines:` に
            # 関数の戻り値を渡す形（`crate::util::why(&e)`）に変えた日、
            # 3件そうやって増えた。
            rest = text[m.end():]
            lead = len(rest) - len(rest.lstrip())
            head = rest[lead:]
            offset = 4 if head.startswith("vec![") else 0
            if head[offset : offset + 1] == "[":
                start = m.end() + lead + offset
                spans.append((start, balanced(text, start, "[", "]")))
    return spans


# ── 項目① 英語しか言えない知らせ ────────────────────────────────────────
MSG_START = re.compile(r"\.message\s*=\s*Some\s*\(|Popup::Notice\s*\{\s*lines\s*:")


def english_only_rs():
    """報せの中に、日本語も `tr(` も無く、英字2語以上のリテラルがあるもの。

    span は `_msg_spans` から取る。**入口は1つにまとめる**。ここに同じ理屈の
    コピーを置いていた日、`Popup::Notice { lines: … }` の形を片方だけ直して、
    もう片方が離れた `[` を拾い、無関係な3件を数えた。
    """
    out = []
    for path in sorted(TUI.glob("*.rs")):
        if path.name == "tests.rs":
            continue
        text = strip_line_comments(unescape_dashes(path.read_text(encoding="utf-8")))
        for start, end in _msg_spans(text):
            body = text[start : end + 1]
            # 日本語が一つでもあれば、両方の言葉を持つ形（`if ja {}` など）。
            if JA.search(body) or "tr(" in body:
                continue
            line = text[:start].count("\n") + 1
            for lit in sorted({x.group(1) for x in RS_LIT.finditer(body)}):
                if JA.search(lit) or "://" in lit or lit in WAIVED:
                    continue
                # 1語だけのものは値（`left` `custom` `view`）で、文ではない。
                if len(WORD.findall(lit)) >= 2:
                    out.append((f"crates/cian-tui/src/{path.name}", line, lit))
    return out


def english_only_js():
    text = strip_line_comments(unescape_dashes(JS.read_text(encoding="utf-8")))
    out = []
    for m in re.finditer(r"\bsay\(", text):
        start = m.end() - 1
        end = balanced(text, start, "(", ")")
        body = text[start : end + 1]
        if JA.search(body) or "tr(" in body:
            continue
        line = text[: m.start()].count("\n") + 1
        for x in JS_LIT.finditer(body):
            lit = x.group(1) or x.group(2) or x.group(3) or ""
            if JA.search(lit) or lit in WAIVED:
                continue
            # 差し込みだけの行（`${a} → ${b}`）は文ではない。英字2語以上を、
            # **差し込みを外してから**数える。
            bare = re.sub(r"\$\{[^}]*\}", "", lit)
            if len(WORD.findall(bare)) >= 2:
                out.append(("gui/renderer.js", line, lit))
    return out


# ── 項目② ダッシュでつないだ一文 ────────────────────────────────────────
#
# 前後に空白のあるダッシュだけを見る。`read-only` の `-` や `→` は別の話。
DASH = re.compile(r"[ 　](?:──|—|―)[ 　]")

# **ダッシュが区切りとして正しいもの。理由が要ります。**
WAIVED = {
    # タイトルの中の区切り。二つの文ではなく、一つの名前と、その状態。
    " コミットメッセージ生成 — 編集中 ",
    " commit message — editing ",
    # キーの表の左右。ダッシュは鍵と説明のあいだの罫線で、文ではない。
    "jj  /  ｊｊ  /  っｊ",
}


def dashes():
    """画面に出る文字列のうち、ダッシュで二つの文をつないだもの。"""
    return [
        (path, line, lit)
        for path, line, lit in screen_strings()
        if DASH.search(lit)
        and lit not in WAIVED
        and len(lit.strip()) >= 8
        and not set(lit.strip()) <= set("─—― ")
    ]


# ── 項目④ 硬い言い方 ────────────────────────────────────────────────────
#
# 本人（2026-09-11）:「AI 独特の表現 『題』『釦』『綱』『たったの2つです』
# みたいな違和感のある表現の洗い出しおよびリライト」。
#
# **日常語を、硬い一字の漢語で書く癖**だ。ボタンを「釦」、ページを「頁」、
# タイトルを「題」。読めるが、**その人が声に出して言わない言い方**で、それが
# 「機械が書いた」と感じさせる。
#
# 難しいのは、cian には**わざと一字の言葉がある**こと ── 枠・桁・ペイン・
# パネルは別のものを指す言葉で、揃えて「きれいに」すると意味が死ぬ
# （`skills/massara-check` に書いてある）。だから見るのは
# **「ふつうの日本語アプリが別の言い方をするもの」だけ**にする。
STIFF = {
    "釦": "ボタン",
    "頁": "ページ",
    "卓": "テーブル",
    "匣": "箱",
    "燈": "明かり",
    "綱": "ロープ / 一覧",
    # **「面」は画面の一部を指す言葉として硬い。** cian が指しているのは
    # 「画面上の四角」で、それは `枠`。`面` は端末版にも窓版にも出ていた
    # （`F12` の説明）
    "面をズーム": "枠をズーム",
    "面を広げる": "枠を広げる",
    "この面": "この枠",
    # 「窓」は**ウィンドウ版の用語**としては正しいが、画面に出す言葉としては
    # ふつうのアプリが「ウィンドウ」と言う
    "窓に合わせ": "ウィンドウに合わせ",
    "窓ぜんたい": "アプリ全体",
    "窓のもの": "ウィンドウ版のもの",
    "窓では": "ウィンドウ版では",
    "上の窓": "上のウィンドウ版",
    "窓と見た目": "ウィンドウと見た目",
    "窓の見た目": "ウィンドウの見た目",
    "窓版": "ウィンドウ版",
    # 2026-09-12 のひと通り。**読めるが、その人が声に出して言わない言い方**が
    # 出どころで、上の一字漢語と同じ癖。直した先を鍵にして、戻ったら失敗させる。
    "この機械": "このパソコン",
    "この端末を経由": "このパソコンを経由",   # 「端末」そのものは cian の言葉
    "見捨て": "強制終了",
    "縛ったキー": "割り当てたキー",
    "縛らなかった": "割り当てていない",
    "走らせ": "実行",
    "で走り": "で実行",
    "順に走り": "順に実行",
    "外で走る": "外で動く",
    "d 忘れる": "d 削除",
    "結果を言います": "結果を表示します",
    # 2026-09-12 の二巡目。本人が一つずつ名指しした直し（`打って絞る`→フィルタ、
    # `未適用`→まだ切り替えていません、`メモ帳`→notepad、`着せ替わります`→
    # その場で変わります）。**`半角ブロック` は「そのままでいい」**と言われたので
    # 辞書に入れない。入れると、残すと決めたものを毎回指摘することになる。
    "打って絞": "フィルタ",
    # **「打つ」は辞書に入れない。** 入力欄では `入力` に直したが、散文の
    # 「コマンドを打つ」「手で打つなら」「文字を打つ所」は書き手の声で、
    # 型ではない（`してしまいます` と同じ理由）。鍵にすると、残すと決めた
    # 3本を毎回指摘することになる
    "絞込": "フィルタ",        # 帯と足元。散文の「絞り込み」は別
    # **同じ機能に名前が3つあった**（入力同期／同時入力／同期入力）。
    # `parity.py` は端末版と窓版の**間**しか見ないので、両方に3つとも
    # あるこれは素通りしていた。本人が選んだのは「シンクロ入力」。
    "入力同期": "シンクロ入力",
    "同時入力": "シンクロ入力",
    "同期入力": "シンクロ入力",
    # `T` の画面の呼び名も3つあった。本人が選んだのは「トグル」
    "各種スイッチ": "トグル",
    "UIトグルメニュー": "トグル",
    "未適用": "まだ切り替えていません",
    "メモ帳": "notepad",
    "着せ替わ": "その場で変わります",
    "着きます": "その場で変わります",
    "けた待ち": "あと1けた",
    "触らず": "触りませんでした",
    "握っています": "使っています",
    "立っている側": "カーソルのある側",
    "向こうの書いたもの": "外で書き換えられた内容",
    "場合の答え": "「〜のときは、こちらです」",
    "不可": "できません",
    "未対応": "できません",
    "未記憶": "まだ覚えていません",
    "据置": "そのまま",
    # 2026-09-15、本人が名指し（「題・網・鳴る・鳴らない・正しい」の5語）。画面に
    # 残っていたのは `名前が正しくありません` の2本だけで、どちらも**何が
    # 悪いのかを言っていない。** 空だったのか、区切り文字が入っていたのか。
    # どちらの言い方もこの前端に既にあったので、そちらへ寄せた。
    #
    # **`無題` と `鳴ります` は入れない**（訊いて、本人が残すと決めた）。
    # `無題` はふつうの日本語アプリがそう書く（Word も macOS もそう）。
    # `鳴ります` は本物のベルの話で、鳴るのが仕事。
    #
    # **註とコミット文は掃かない**（同じときに本人が決めた）。`形`・`黙`・
    # `基準` はユーザの目に触れないので、そのままでよい。揃えると
    # `枠`・`桁`・`ペイン` のような、わざと分けてある言葉まで死ぬ。
    "正しくありません": "何が悪いのかを言う（空なのか、使えない字が入っているのか）",
    "正しく設定": "設定のどこが足りないかを言う",
}


def doc_words(listing=False):
    """**画面の外の読み物も、同じ言葉で書く。**

    README・CHECK・GUI.txt は画面の字をそのまま引用する。そこが古いと、
    読んだ人は画面に無い言葉を探すことになる（2026-09-15、`CHECK.ja.txt` が
    `「この端末を経由して」` を、`ROADMAP` が `この枠を広げる` を探させていた。
    どちらも、その字を画面から消した日に置き去りになっていた）。

    天井では見ない。辞書は「画面でこう言い換えた」の記録で、読み物の中には
    **その言い換えを説明している行**（この項目、`REQUESTS` の依頼 194、
    `massara-check`）が必ずあるから。数えると、説明が毎回指摘される。
    `--docs` で一覧を出し、直すかどうかは人が決める。
    """
    out = []
    for rel in DOC_FILES:
        path = ROOT / rel
        if not path.exists():
            continue
        for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            for w, better in STIFF.items():
                if w in line:
                    out.append((rel, n, line.strip(), better))
    return out


DOC_FILES = (
    "README.ja.md",
    "packaging/windows/GUI.txt",
    "packaging/windows/START.ja.txt",
    "packaging/windows/CHECK.ja.txt",
    "packaging/windows/DEPLOY.ja.txt",
    "packaging/macos/GUI.ja.txt",
)


def stiff_words():
    return [
        (path, line, lit, STIFF[w])
        for path, line, lit in screen_strings()
        for w in STIFF
        if w in lit
    ]


# ── 項目③ 型どおりの言い回し ────────────────────────────────────────────
#
# **書き手が一度も声に出していない言い回し。** どれも「間違い」ではなく、
# 「cian はこう言わない」というだけです。左が形、右がなぜ。
STOCK = {
    "ことができます": "「できます」で足りる",
    "ことが可能です": "同上",
    "が可能です": "「できます」",
    "必要があります": "「〜してください」か、何が起きるかを言う",
    "を行います": "その動詞で言う（「削除を行います」→「削除します」）",
    "以下のとおり": "画面に「以下」は無い。指すものを名で言う",
    "以下のように": "同上",
    "上記の": "同上",
    "エラーが発生しました": "何が起きたかを言っていない",
    "正常に": "成功は既定。わざわざ言わない",
    "適切に": "何をもって適切かを言っていない",
    "必要に応じて": "いつ必要かを言っていない",
    "〜する必要がある場合": "同上",
    "ご確認ください": "cian は敬語の階段を上らない。「確認してください」",
    "してしまいます": None,  # 自然な日本語。辞書には**入れない**（下の註）
    "please note": "英語も同じ。cian は `ls` の声で話す",
    "successfully": "成功は既定",
    "in order to": "`to` で足りる",
    "utilize": "`use`",
}
# `してしまいます` は**わざと外してあります。** 「シェルが解釈してしまいます」
# （actions.rs:728）は書き手の声で、型ではない。辞書に入れると、この1件のために
# 免除を書くことになる。**辞書は「見た瞬間に機械が書いたと分かる形」だけ。**
STOCK = {k: v for k, v in STOCK.items() if v is not None}


def stock_phrases():
    out = []
    for path, line, lit in screen_strings():
        low = lit.lower()
        for phrase, why in STOCK.items():
            if phrase in lit or phrase in low:
                out.append((path, line, lit, why))
    return out


# ── 項目⑤ 作った言い回し（画面の外も含む）──────────────────────────────
#
# **言い換えの表はここ1つだけである。** 直す先を変えたいときは、この表の1行を
# 直して走らせ直す。表が2か所にあると、片方だけ直ったときにもう片方が検査を
# 通してしまう。
#
# 対象は git が追跡しているすべての文字列である。画面に出る文言・マニュアル・
# 設計書・ソースのコメント・検査が印字する文を区別しない。読む相手が利用者か
# 開発者かで言葉を変えない方針である（2026-11-28 の指示「ドキュメント、
# ソース、コメント、ログ、画面上に表示されるもの、表示されないもの、すべて」）。
#
# 出どころは crmaine の指示3件である。
#   * 作った言い回しを使わない（2026-11-03「AI 的な表現じゃなく、人間の自然言語で」）
#   * ソースのコメントは 5W2H を書き、である調で「やること＋理由」の順に書く
#     （たとえ・経験談・日付・擬人化した口語を使わない）
#   * 呼び名の決定（2026-11-24 R13）
#
# 左が探す形、右が直す先である。
COINED = {
    # crmaine R13 で本人が決めた呼び名
    "関門": "検査",
    "台帳": "記録ファイル",
    r"見張[るりっら]": "監視する",
    "道具": "ツール",
    "偽物": "模擬サーバー",
    r"のぞ[くきけい]": "参照する",
    # 普通の日本語の熟語は除く（画面・前面・字面・場面・側面・表面・斜面・水面・
    # 局面・全面・反面・一面・両面・他面・対面・書面・平面・方面）。
    r"(?<![画場側表斜平水方局全反一両他対書前字文正])面(?![白積談])": "パネル / 画面",
    # 2026-11-03 に名指しされた、たとえの言葉
    "旗": "設定の名前そのもの",
    r"(?<![表值])札(?!幌)": "カード / ボタン",
    "受け口": "ハンドラ / 受信処理",
    "殻": "その物の名前",
    "相乗り": "共用",
    "写し": "コピー",
    "合言葉": "パスワード",
    # cian が自分で作った言い回し
    "扉": "入口 / 経路",
    "皿": "担当",
    "砂場": "作業用ディレクトリ",
    "掃引": "全件検査",
    # 「帯域」は普通の日本語なので除く。
    r"(?<![携世])帯(?!域)": "進捗バー",
    # 「席を離れる」「席を外す」は机から離れる意味の普通の日本語なので除く。
    r"(?<![出欠座])席(?!を離|を外)": "実行枠",
    "物差し": "基準 / 測定方法",
    "芯": "中核",
    "尻尾": "残り",
    "束ね": "まとめ",
    "家の言葉": "この前端の用語",
    # 経験談と擬人化（2026-11-23 の指示で、コメントからも外す）
    # 端末のベルは本当に鳴るので、ベルの話は `COINED_OK_LINE` で除く。
    r"鳴[るりらっ]": "検知する / 失敗する",
    r"噛[むんめ]": "検知する",
    r"踏[むんめま]": "発生する / 再発する",
    r"転[ぶんび](?!送)": "失敗する",
    r"黙[るりっら]": "警告を出さない / 通知せずに",
    "騙": "誤った結果を返す",
    r"正解|答えです": "その動作を直接書く",
    "嘘": "実態と合わない / 誤った",
    "鐘": "その物の名前",
    # 文書の構成は「章」、画面や報告の区切りは「項目」、DOM は「要素」とする。
    # 「関節」「季節」「音節」「文節」「節約」「節度」は普通の日本語なので除く。
    r"(?<![関骨季音章文])節(?![約度])": "章 / 項目 / 要素",
    # 口語の強調。判断の根拠にならない（crmaine の `docs/source-style.md`）。
    "原理的に": "仕組み上",
    "方がまし": "事実で理由を書く",
}

# 指摘しないもの。**消すのではなく、なぜ指摘しないのかを残す。**
COINED_OK = {
    "docs/kotoba-rewrite.ja.md": "言い換えの記録である",
}

# **この検査自身の文も検査する。** 辞書を持つファイルを丸ごと免除していたため、
# `kotoba.py` の説明文だけが古い言葉のまま残っていた（2026-11-28）。免除するのは
# 辞書の範囲だけとし、下の2行の間だけを読み飛ばす。
# 免除するのは2つの辞書の範囲である。`STIFF` は画面の字を探す鍵なので、
# 古い言葉そのものを持っていなければならない。
DICT_SPANS = (("COINED = {", "COINED_FILES = ("), ("STIFF = {", "def doc_words"))


def without_dictionary(rel: str, text: str) -> str:
    """`kotoba.py` から辞書の範囲だけを落とす。ほかのファイルはそのまま返す。"""
    if not rel.endswith("scripts/kotoba.py"):
        return text
    for begin, end in DICT_SPANS:
        a, b = text.find(begin), text.find(end)
        if a < 0 or b < 0 or b < a:
            raise SystemExit(
                f"kotoba.py の辞書の範囲（{begin}）が読めない。免除の目印が"
                "変わると、この検査は自分の表を指摘し続ける")
        # 行数を保つため、落とす範囲は空行に置き換える。
        text = text[:a] + "\n" * text.count("\n", a, b) + text[b:]
    return text

# 本物のベルの話は残す（2026-09-15 に本人が決定）。
COINED_OK_LINE = ("ベル", "bell", "COINED")

COINED_FILES = (".rs", ".py", ".js", ".md", ".txt", ".html", ".lua")

# いまの数を天井とし、直した分だけ下げる。下限は辞書が縮んだときに失敗する。
# **全部 0 である。** 2026-11-28 の初回は 839 箇所（doc 360・py 190・js 163・
# rs 102・その他 24）で、同じ日に 0 まで下げた。入り込んだ日に失敗する。
COINED_CEILING = {"rs": 0, "py": 0, "js": 0, "doc": 0, "other": 0}
LEAST_COINED_WORDS = 30


def tracked_text_files() -> list[str]:
    out = subprocess.run(["git", "ls-files"], cwd=ROOT, capture_output=True, text=True)
    return [f for f in out.stdout.split() if f.endswith(COINED_FILES)]


def coined_group(rel: str) -> str:
    if rel.endswith(".rs"):
        return "rs"
    if rel.endswith(".py"):
        return "py"
    if rel.endswith(".js"):
        return "js"
    if rel.endswith((".md", ".txt")):
        return "doc"
    return "other"


def coined_words():
    """作った言い回しを、追跡しているすべての文字列から拾う。

    行ごとに見る。1行に2語あれば2件と数える。
    """
    if len(COINED) < LEAST_COINED_WORDS:
        raise SystemExit(
            f"⑤ の辞書が {len(COINED)} 語しかない（{LEAST_COINED_WORDS} 語のはず）。"
            "探す語が落ちると、この検査は何も見ずに緑を返す")
    out = []
    for rel in tracked_text_files():
        if rel in COINED_OK:
            continue
        try:
            text = (ROOT / rel).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for n, line in enumerate(without_dictionary(rel, text).splitlines(), 1):
            if any(w in line for w in COINED_OK_LINE):
                continue
            for pat, better in COINED.items():
                for _ in re.finditer(pat, line):
                    out.append((rel, n, line.strip(), better))
    return out


def show(rows, listing, limit=20):
    for r in rows if listing else rows[:limit]:
        print(f"    {r[0]}:{r[1]}")
        print(f"      {r[2][:88]}")
    if not listing and len(rows) > limit:
        print(f"    …ほか {len(rows) - limit} 件（--list で全部）")


def main() -> int:
    listing = "--list" in sys.argv
    if "--docs" in sys.argv:
        rows = doc_words()
        print("=" * 72)
        print("  画面の外の読み物 ── 画面で言い換えた言葉が残っていないか")
        print("=" * 72)
        for rel, n, line, better in rows:
            print(f"    {rel}:{n}")
            print(f"      {line[:88]}")
            print(f"      → {better}")
        print(f"\n  {len(rows)} 件。**天井は無い。** 言い換えを説明している行も混ざるので、")
        print("  直すかどうかは読んで決める（`kotoba.py` 本体は画面の字だけを見る）")
        return 0
    eng_rs = english_only_rs()
    eng_js = english_only_js()
    dash = dashes()
    dash_rs = [d for d in dash if d[0].startswith("crates")]
    dash_js = [d for d in dash if d[0].startswith("gui")]
    stock = stock_phrases()
    stiff = stiff_words()

    print("=" * 72)
    print("  画面に出る言葉 ── 規則は theme.rs の tr() の doc コメント")
    print("=" * 72)
    bad = []

    print(f"\n  ① 英語しか言えない知らせ   端末版 {len(eng_rs):3} 件"
          f"（天井 {ENGLISH_ONLY_CEILING}）  窓版 {len(eng_js)} 件"
          f"（天井 {ENGLISH_ONLY_JS_CEILING}）")
    print("     端末版の既定は日本語です。失敗の知らせほどここに多い")
    if eng_rs or eng_js:
        show(eng_rs + eng_js, listing)
    if len(eng_rs) > ENGLISH_ONLY_CEILING:
        bad.append(f"① 端末版が {len(eng_rs)} 件に増えました（天井 {ENGLISH_ONLY_CEILING}）")
    if len(eng_js) > ENGLISH_ONLY_JS_CEILING:
        bad.append(f"① 窓版が {len(eng_js)} 件に増えました（天井 {ENGLISH_ONLY_JS_CEILING}）")

    print(f"\n  ② ダッシュでつないだ一文   端末版 {len(dash_rs):3} 件"
          f"（天井 {DASH_CEILING_RS}）  窓版 {len(dash_js)} 件"
          f"（天井 {DASH_CEILING_JS}）")
    print("     「〜です。〜できます」の2文に。doc の悪い例がそのまま生きています")
    if dash_rs or dash_js:
        show(dash_rs + dash_js, listing)
    if len(dash_rs) > DASH_CEILING_RS:
        bad.append(f"② 端末版が {len(dash_rs)} 件に増えました（天井 {DASH_CEILING_RS}）")
    if len(dash_js) > DASH_CEILING_JS:
        bad.append(f"② 窓版が {len(dash_js)} 件に増えました（天井 {DASH_CEILING_JS}）")

    print(f"\n  ④ 硬い言い方               {len(stiff)} 件（天井 {STIFF_CEILING}）")
    print("     日常語を、その人が声に出して言わない一字の漢語で書いている")
    if stiff:
        for path, line, lit, better in (stiff if listing else stiff[:20]):
            print(f"    {path}:{line}")
            print(f"      {lit[:70]}")
            print(f"      → {better}")
        if not listing and len(stiff) > 20:
            print(f"    …ほか {len(stiff) - 20} 件（--list で全部）")
    if len(stiff) > STIFF_CEILING:
        bad.append(f"④ 硬い言い方が {len(stiff)} 件に増えました（天井 {STIFF_CEILING}）")

    print(f"\n  ③ 型どおりの言い回し       {len(stock)} 件（天井 0 ── 監視）")
    if stock:
        for path, line, lit, why in stock if listing else stock[:20]:
            print(f"    {path}:{line}")
            print(f"      {lit[:70]}")
            print(f"      → {why}")
        bad.append(f"③ 型どおりの言い回しが {len(stock)} 件入りました")
    else:
        print("     入っていません")

    coined = coined_words()
    by = {}
    for rel, *_ in coined:
        g = coined_group(rel)
        by[g] = by.get(g, 0) + 1
    print(f"\n  ⑤ 作った言い回し           " + "  ".join(
        f"{g} {by.get(g, 0)} 件（天井 {COINED_CEILING[g]}）" for g in ("rs", "py", "js", "doc")))
    print("     たとえの言葉・擬人化・経験談。画面の外も同じ言葉で書く")
    if coined:
        for rel, n, line, better in (coined if listing else coined[:20]):
            print(f"    {rel}:{n}")
            print(f"      {line[:80]}")
            print(f"      → {better}")
        if not listing and len(coined) > 20:
            print(f"    …ほか {len(coined) - 20} 件（--list で全部）")
    for g, top in COINED_CEILING.items():
        if by.get(g, 0) > top:
            bad.append(f"⑤ {g} が {by[g]} 件に増えました（天井 {top}）")

    print()
    print("=" * 72)
    if bad:
        for b in bad:
            print(f"  ✗ {b}")
        print("=" * 72)
        return 1
    print("  天井の中です。触った箇所から少しずつ下げて、天井も一緒に下げること")
    print("=" * 72)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
