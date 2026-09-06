#!/usr/bin/env python3
"""窓版のうち、まだ日本語しか話せない部分がどれだけ残っているか。

端末版は `tr(en, ja)` で書かれていて、窓版は日本語直書きから始まりました。
`言語` を入れるというのは、**画面に出る文字列を1つずつ両方の言葉にする**
作業で、途中で止まると「切り替えたのに半分そのまま」になります。だから
残りを数えます。

数え方: `gui/renderer.js` の文字列リテラルのうち日本語を含むものを取り、
`tr(` の引数になっているものを引きます。**コメントは数えません**（経緯は
日本語で書いてあり、それは画面に出ません）。

    python3 scripts/i18n.py          # 残りの件数
    python3 scripts/i18n.py --list   # 残っている文字列
    python3 scripts/i18n.py --list 40

**端末版も同じ物差しで見ます**（2026-09-06 に足した）。あちらは
`tr(lang, en, ja)` のほかに `entry(key, action, en, ja)` や
`(Lang::Ja) => "…"` など**別の書き方で両言語を持っている**ので、`tr(` の外に
あるかどうかでは数えられません ── 素朴に数えると 586 件出て、その大半が
相方を持っていました。**近くに英語の相方がいるか**で見ます。

見つかったのは、シェルパネルの案内文が `tr()` の外にあった件と同じ形です。
`tr()` の外にあると、**句点の検査（`nothing_cian_says_ends_in_a_full_stop`）
にも掛かりません**。
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
JA = re.compile(r"[぀-ヿ一-鿿]")
# `'…'` `"…"` `` `…` ``、ただし改行をまたぐ単引用符は文字列ではない。
LIT = re.compile(r"'((?:[^'\\\n]|\\.)*)'|\"((?:[^\"\\\n]|\\.)*)\"|`((?:[^`\\]|\\.)*)`")


def source_without_comments(text):
    """行コメントを落とす。文字列の中の `//` を消さないよう、素朴に
    「行頭が // の行だけ」にする ── audit.py が一度これで `'http://'` を
    壊しているので、賢くしない。"""
    out = []
    for line in text.splitlines():
        t = line.lstrip()
        out.append("" if t.startswith("//") else line)
    return "\n".join(out)


def spans_inside_tr(text):
    """`tr(` の丸括弧の中。入れ子とテンプレート文字列があるので、括弧を数える。"""
    spans = []
    for m in re.finditer(r"\btr\(", text):
        depth = 0
        i = m.end() - 1
        while i < len(text):
            c = text[i]
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
                if depth == 0:
                    spans.append((m.end(), i))
                    break
            i += 1
    return spans


# 訳さないもの。**理由が要ります。**
#
# どちらも「その言語で書いてあること自体が意味」の文字列です。配色は名前で
# （dracula や nord を訳さないのと同じ）、切替のラベルは*切り替わる先*の言葉で
# 書きます ── 端末版の `MenuItem::Lang` も `tr` ではなく match です。
KEEP = {
    "白磁": "窓の配色の名前。dracula や nord と同じで、名前は訳さない",
    "陰翳": "同上",
    "端末譲り": "同上",
    "日本語": "スイッチの値。いまの言語の名を、その言語で言う",
    "日本語に切替": "切り替わる先の言葉で書く（英語のときだけ出る）",
    # キーの名前。IME オンで j を2回押すと出る文字そのもので、
    # 「訳した ｊｊ」は存在しない ── 配色の名前と同じ理由。
    "っｊ": "キーの名前。IME が j×2 から作る文字そのもの",
    "jj  /  ｊｊ  /  っｊ": "同上。ヘルプのキー列",
}


def untranslated(path):
    text = source_without_comments((ROOT / path).read_text(encoding="utf-8"))
    inside = spans_inside_tr(text)

    def covered(at):
        return any(a <= at < b for a, b in inside)

    out = []
    for m in LIT.finditer(text):
        v = m.group(1) or m.group(2) or m.group(3) or ""
        if JA.search(v) and not covered(m.start()) and v not in KEEP:
            out.append((text[: m.start()].count("\n") + 1, v))
    return out


def nested(path):
    """`tr(en, tr(en, ja))` ── 一語を一括で包むと、既に包んであるものの中の
    日本語まで包みます。二度やりました。英語側が二重になるので画面には
    出ませんが、次に誰かが英語を直すとき片方しか直りません。"""
    text = source_without_comments((ROOT / path).read_text(encoding="utf-8"))
    return [
        (text[: m.start()].count("\n") + 1, m.group(0)[:80])
        for m in re.finditer(r"\btr\([^,()]*,\s*tr\(", text)
    ]


def frozen(rel):
    """読み込み時に一度だけ `tr()` を評価してしまう定数。

    `tr()` は文字列を返すので、`const X = tr(...)` が持っているのは
    **ファイルを読んだときの言語**です。あとで `T → 言語` を切り替えても
    そこだけ変わりません ── 実際 SORTS・VIEW_NAMES・STYLES・2つのメニューの
    foot で、英語にしたのに 名前 / サイズ / 日時 / メモ帳 / クラシック /
    アイコン が残りました。**訳し忘れではなく、訳したものが凍っていた**ので、
    「日本語が残っている」を数える上の検査には一件も映りませんでした。

    数え方は素朴に: 桁 0 から始まる `const`/`let` の宣言を、括弧の釣り合いが
    取れるまで読み、その中に `tr(` があれば凍っている。`=>` の右側にある
    `tr(` は呼ばれるたびに評価されるので数えません。
    """
    text = source_without_comments((ROOT / rel).read_text(encoding="utf-8"))
    lines = text.split("\n")
    out, i = [], 0
    while i < len(lines):
        m = re.match(r"^(const|let)\s+(\w+)\s*=", lines[i])
        if not m:
            i += 1
            continue
        buf, j, depth = [], i, 0
        while j < len(lines):
            buf.append(lines[j])
            depth += sum(lines[j].count(c) for c in "([{")
            depth -= sum(lines[j].count(c) for c in ")]}")
            if depth <= 0 and lines[j].rstrip().endswith(";"):
                break
            if j - i > 60:
                break
            j += 1
        body = "\n".join(buf)
        # `=>` より前に出る `tr(` だけが凍る。矢印の右側は毎回評価される。
        head = body.split("=>")[0] if "=>" in body else body
        if "tr(" in head:
            out.append((i + 1, m.group(2)))
        i = j + 1
    return out


# ── 端末版 ────────────────────────────────────────────────────────────
#
# あちらは書き方が1つではない。`tr(lang, en, ja)` のほかに:
#
#     entry("q", Some(Quit), "quit (confirms)", "終了（確認あり）")
#     ("General", "基本")
#     (CloseTarget::ShellTab, Lang::En) => "this shell tab"
#     let d = move |en, jp| if ja { jp } else { en };
#
# どれも両方の言葉を持っている。**`tr(` の外か**で数えると 586 件出て、
# その大半がこれらだった。だから「**近くに英語の相方がいるか**」で見る ──
# 前後6行の中に、英字が2つ以上続くリテラルがあれば相方とみなす。
#
# **見えないもの（書いておく）。** 訳してある行の**隣**に未訳が1行だけ
# 混じっている場合、相方が6行以内にいるので通ってしまう。実際、直したあとに
# `▤ 詳細` の1行だけ元に戻して確かめたら鳴らなかった。
#
# 囲んでいる括弧の中だけを見る形も試したが、`format!("… 件")` のように相方が
# 構造上どこにも無いものと、`msg.contains("中止")`（表示ではなく判定）や
# `"copying" => "コピー中"` を見分けられず、誤検知が 23 件出た。**賢くすると
# 別のものを見落とす** ── `audit.py` が一度 `'http://'` を壊した形。
#
# 数えているのは「**まるごと日本語のまま残っている面**」で、そこは今日 8 か所
# 見つかって 0 になった。1行だけの取りこぼしは、`tr()` を書く手が拾う。
TUI_DIR = ROOT / "crates" / "cian-tui" / "src"
# 日本語しか言えないまま残っているもの。**増えたら落とす**（減らすのは別の日）。
TUI_CEILING = 0
RS_LIT = re.compile(r'"((?:[^"\\\n]|\\.)*)"')


def _english_ish(s):
    return bool(re.search(r"[A-Za-z]{2}", s)) and not JA.search(s)


def tui_japanese_only():
    """端末版のうち、英語の相方が見当たらない日本語。

    テストは見ない ── そこの日本語は画面に出ない。
    """
    out = []
    for path in sorted(TUI_DIR.glob("*.rs")):
        if path.name == "tests.rs":
            continue
        text = path.read_text(encoding="utf-8")
        # `#[cfg(test)] mod tests { … }` から先も落とす（同じ理由）。
        cut = text.find("#[cfg(test)]")
        if cut > 0:
            text = text[:cut]
        text = source_without_comments(text)
        lines = text.split("\n")
        for m in RS_LIT.finditer(text):
            v = m.group(1) or ""
            if not JA.search(v):
                continue
            n = text[: m.start()].count("\n")
            # **`Lang::Ja =>` の腕は相方を持っている。** 同じ `match` の中に
            # `Lang::En =>` の腕があり、註が長いと3行では届かない。腕そのもの
            # が「これは日本語側です」と言っているので、それを信じる。
            if "Lang::Ja" in lines[n] or "Lang::En" in lines[n]:
                continue
            # ±6 行。`if lang == Lang::Ja { … } else { … }` の腕は註を挟むと
            # 4〜5 行離れる（`ConfirmElevate` と空ファイルの案内がそれ）。
            # 広げすぎると本物を隠すので、実際に離れていた距離に合わせる。
            near = "\n".join(lines[max(0, n - 6) : n + 7])
            if any(_english_ish(x.group(1) or "") for x in RS_LIT.finditer(near)):
                continue
            out.append((path.name, n + 1, lines[n].strip()))
    return out


def main():
    left = untranslated("gui/renderer.js")
    bad = nested("gui/renderer.js")
    stuck = frozen("gui/renderer.js")
    done = len(spans_inside_tr(source_without_comments(
        (ROOT / "gui/renderer.js").read_text(encoding="utf-8"))))
    total = done + len(left)
    if "--list" in sys.argv:
        after = sys.argv[sys.argv.index("--list") + 1:]
        cap = int(after[0]) if after and after[0].isdigit() else 60
        for n, v in left[:cap]:
            print(f"  renderer.js:{n}  {v[:90]}")
        if len(left) > cap:
            print(f"  …ほか {len(left) - cap} 件")
        print()
    tui = tui_japanese_only()
    pct = 100 * done // total if total else 100
    print("=" * 72)
    for n, t in bad:
        print(f"  ■ tr が入れ子になっています  renderer.js:{n}  {t}")
    for n, t in stuck:
        print(f"  ■ tr が読み込み時に凍っています  renderer.js:{n}  {t}"
              "  ── 関数にして、描くたびに訊いてください")
    if bad or stuck:
        print()
    print(f"  窓版: 両方の言葉で言えるもの {done} / {total}（{pct}%）"
          f" ── まだ日本語だけ {len(left)} 件"
          + (f"（訳さないと決めたもの {len(KEEP)} 件は除く）" if not left else ""))
    over = len(tui) > TUI_CEILING
    print(f"  端末版: 日本語しか言えないもの {len(tui)} 件"
          + ("（上限 %d を超えています）" % TUI_CEILING if over else f"（上限 {TUI_CEILING}）"))
    if over or "--list" in sys.argv:
        for name, n, line in tui:
            print(f"    {name}:{n}  {line[:88]}")
    print("=" * 72)
    return 1 if (bad or stuck or over) else 0


if __name__ == "__main__":
    sys.exit(main())
