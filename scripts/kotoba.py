#!/usr/bin/env python3
"""画面に出る**言葉**の質。文言そのものを、毎回同じ物差しで見る。

`i18n.py` は「両方の言葉で言えるか」を数え、`widths.py` は「桁で測っているか」
を数えます。**どちらも中身は見ていません** ── 両方の言葉で言えて、桁も合って
いて、それでも読んだ人が身構える文はあります。2026-09-10 に本人から出た指摘は
そこでした（「トグルの補足文の日本語が AI 的」）。

    python3 scripts/kotoba.py          # 節ごとの件数
    python3 scripts/kotoba.py --list   # 一行ずつ

## 物差しは `tr()` の doc コメントにある

規則は誰かの好みではなく、`crates/cian-tui/src/theme.rs` の `tr()` に
書いてあります（英語は小文字始まり・日本語は敬体・末尾に句点を打たない・
**ダッシュでつながず2文にする**・「今のところ」と言わない）。**書いてあるのに
機械が見ていなかったのは、そのうち句点だけ**でした
（`nothing_cian_says_ends_in_a_full_stop`）。ここは残りを見ます。

## 天井で見る（`i18n.py` の `TUI_CEILING` と同じ作法）

①と②は**いま多い**ので、0 を要求すると初日から落ちて読まれなくなります。
`audit.py` が一度「当てにならない指摘が7件」で読まれなくなった形です。だから
**いまの数を天井として書き、増えたら落とす**。減らすのは触った面から少しずつ。
③だけは 0 が天井の見張りで、**入り込んだ日に鳴ります**。

## 節① 英語しか言えない知らせ

**端末版の既定の言語は日本語です**（`Lang::from_opt` ── 設定が無ければ日本語）。
それなのに `self.message` と `Popup::Notice` の文字列には、`tr()` を通らず
英語だけのものが並んでいます。`copy failed: {}`、`backup failed, not saving`、
`no password or key set for {}@{}` ── **失敗の知らせほどここに多い**。

`i18n.py` は逆向き（日本語しか言えないもの）しか数えていないので、これは
**どの物差しにも映っていませんでした**。

数え方: `self.message = Some(…)` と `Popup::Notice { lines: […] }` の
**釣り合った括弧の中だけ**を見て、日本語も `tr(` も無く、英字2語以上の
リテラルがあるもの。±数行の窓で見ると近くの doc コメントを拾うので、
式の中だけを読みます。

## 節② ダッシュでつないだ一文

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
それは `WAIVED` に理由つきで書きます ── 消すのではなく、なぜ指摘しないのかを
残す（`audit.py` の `TERM_OK` と同じ）。

## 節③ 型どおりの言い回し

「AI っぽい」の正体は、**書き手が一度も声に出していない言い回し**です。
いまは 0 件 ── だからこの節は**見張り**で、天井は 0。増えた日に鳴ります。

**辞書に足すときは変異テストを。** 「指摘が消えた」ではなく「検査が黙った」を
cian では5回やっています（`keycover.py` が 72 → 2 種に落ちたまま百分率を
出し続けた件）。壊した版で鳴るのを見てから戻すこと。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TUI = ROOT / "crates" / "cian-tui" / "src"
JS = ROOT / "gui" / "renderer.js"
# 設定画面が読み上げる表。**ここもユーザが読む画面**なのに、長いあいだこの
# 検査の外だった ── 2026-09-12、「窓の見た目」がここに残っていて、④が
# 黙ったまま通していた。拾うのは `*_ja:` の一本道だけで、テストの文字列や
# Lua の見本は入らない。
LUA = ROOT / "crates" / "cian-lua" / "src"
LUA_FILES = ("settings_schema.rs", "settings_keymap.rs")
# 窓版が画面に出す断りの言葉の**半分はエンジンが書いている** ── `say()` に
# そのまま流れるので、これもユーザが読む画面。`zip へはコピー（追加）のみ`
# は端末版と窓版の両方で直したのに、ここに三つ目の写しが残っていた
# （2026-09-12）。**扉は `bail!` / `anyhow!` / `format!` の三つだけ**にして、
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
ENGLISH_ONLY_CEILING = 102  # 端末版。既定が日本語なのに英語しか言えない知らせ
#   103 → 102（2026-09-11、`:key` の案内を tr() に通した）。**直したら天井も
#   その日の数まで下げる** ── 下げ忘れると、直したぶんの余白でまた増やせる
ENGLISH_ONLY_JS_CEILING = 1  # 窓版。ここはほぼ片付いている
DASH_CEILING_RS = 125        # 端末版。`tr()` の両方の言葉ぶんを数えるので、報せの数のほぼ倍
#   124 → 125（2026-09-12）。**文章が増えたのではなく、見る場所が増えた** ──
#   エンジン（`cian-server`）の断りの言葉を読むようにしたら、そこに1本あった
#   （`中身が違います — 手元 … ≠ 向こう …`）。数え上げの天井を上げるのは
#   これだけが理由で、書き手が増やしたぶんは**一つも入っていない**
DASH_CEILING_JS = 155        # 窓版
#   156 → 155（2026-09-12）。`vim（既定）／ メモ帳 ── …` を `notepad / vim` に
#   書き直したときにダッシュが1本減った。**直したら天井も下げる**
# 硬い言い方。**ここは 0 を目指す** ── 数が少なく、直し方が一つに決まる。
STIFF_CEILING = 0


def strip_line_comments(text: str) -> str:
    """行頭が `//` の行だけ落とす。**賢くしない** ── 文字列の中の `//` を
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
# **扉は1つにする。** ②と③が「どこを見るか」で割れると、片方だけ直したときに
# もう片方が黙ります。定義はここ1つ:
#
#   * `tr(…)` の引数（両前端。端末版の固定文言はすべてここを通る）
#   * `self.message = Some(…)` / `Popup::Notice { lines: […] }`（端末版）
#   * `say(…)`（窓版）
#
# `cian-core` は画面を持たないので見ません ── あそこの英文は `substitute.rs`
# の使い方案内のように、端末版が `tr()` に包み直して出すか、出ないかのどちらか。
def screen_strings():
    out = []
    for path in sorted(TUI.glob("*.rs")):
        if path.name == "tests.rs":
            continue
        rel = f"crates/cian-tui/src/{path.name}"
        text = strip_line_comments(path.read_text(encoding="utf-8"))
        for start, end in _outermost(_call_spans(text, r"\btr\(") + _msg_spans(text)):
            for m in RS_LIT.finditer(text[start : end + 1]):
                out.append((rel, text[:start].count("\n") + 1, m.group(1)))
    text = strip_line_comments(JS.read_text(encoding="utf-8"))
    for start, end in _outermost(_call_spans(text, r"\btr\(") + _call_spans(text, r"\bsay\(")):
        for m in JS_LIT.finditer(text[start : end + 1]):
            lit = next((g for g in m.groups() if g is not None), "")
            out.append(("gui/renderer.js", text[:start].count("\n") + 1, lit))
    for path in sorted(SRV.glob("*.rs")):
        rel = f"crates/cian-server/src/{path.name}"
        text = strip_line_comments(path.read_text(encoding="utf-8"))
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
        text = strip_line_comments((LUA / name).read_text(encoding="utf-8"))
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
            # 無関係な行のリテラルまで一つの報せとして数える ── `lines:` に
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


# ── 節① 英語しか言えない知らせ ────────────────────────────────────────
MSG_START = re.compile(r"\.message\s*=\s*Some\s*\(|Popup::Notice\s*\{\s*lines\s*:")


def english_only_rs():
    """報せの中に、日本語も `tr(` も無く、英字2語以上のリテラルがあるもの。

    span は `_msg_spans` から取る ── **扉は1つにする**。ここに同じ理屈の
    写しを置いていた日、`Popup::Notice { lines: … }` の形を片方だけ直して、
    もう片方が離れた `[` を拾い、無関係な3件を数えた。
    """
    out = []
    for path in sorted(TUI.glob("*.rs")):
        if path.name == "tests.rs":
            continue
        text = strip_line_comments(path.read_text(encoding="utf-8"))
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
    text = strip_line_comments(JS.read_text(encoding="utf-8"))
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


# ── 節② ダッシュでつないだ一文 ────────────────────────────────────────
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
    """画面に出る文字列のうち、ダッシュで二つの節をつないだもの。"""
    return [
        (path, line, lit)
        for path, line, lit in screen_strings()
        if DASH.search(lit)
        and lit not in WAIVED
        and len(lit.strip()) >= 8
        and not set(lit.strip()) <= set("─—― ")
    ]


# ── 節④ 硬い言い方 ────────────────────────────────────────────────────
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
    # 「窓」は**窓版という家の言葉**としては正しいが、画面に出す言葉としては
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
    # 出どころで、上の一字漢語と同じ癖。直した先を鍵にして、戻ったら鳴らす。
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
    # 辞書に入れない ── 入れると、残すと決めたものを毎回鳴らすことになる。
    "打って絞": "フィルタ",
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
}


def stiff_words():
    return [
        (path, line, lit, STIFF[w])
        for path, line, lit in screen_strings()
        for w in STIFF
        if w in lit
    ]


# ── 節③ 型どおりの言い回し ────────────────────────────────────────────
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
# `してしまいます` は**わざと外してあります** ── 「シェルが解釈してしまいます」
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


def show(rows, listing, limit=20):
    for r in rows if listing else rows[:limit]:
        print(f"    {r[0]}:{r[1]}")
        print(f"      {r[2][:88]}")
    if not listing and len(rows) > limit:
        print(f"    …ほか {len(rows) - limit} 件（--list で全部）")


def main() -> int:
    listing = "--list" in sys.argv
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

    print(f"\n  ③ 型どおりの言い回し       {len(stock)} 件（天井 0 ── 見張り）")
    if stock:
        for path, line, lit, why in stock if listing else stock[:20]:
            print(f"    {path}:{line}")
            print(f"      {lit[:70]}")
            print(f"      → {why}")
        bad.append(f"③ 型どおりの言い回しが {len(stock)} 件入りました")
    else:
        print("     入っていません")

    print()
    print("=" * 72)
    if bad:
        for b in bad:
            print(f"  ✗ {b}")
        print("=" * 72)
        return 1
    print("  天井の中です。触った面から少しずつ下げて、天井も一緒に下げること")
    print("=" * 72)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
