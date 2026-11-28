#!/usr/bin/env python3
"""端末版（cian-tui）と窓版（cian-gui）で、画面に出る言葉が揃っているか。

audit.py は片側ずつしか見ません。requests.py は頼まれたことしか見ません。
**同じものを二つの前端が別の言葉で呼んでいる**のは、そのどちらにも映らない。
実際、2026-08-31 の時点で

  端末版「コピー」          窓版「コピー（保持）」
  端末版「リネーム」        窓版「名前を変える」
  端末版「ゴミファイル検出」 窓版「不要さがし」
  端末版「並び: 日時 ▲」    窓版「並び: date ↑」

のように 30 箇所ちかく食い違っていて、どれも一件ずつ発生して気づく形でした。

見ている次元:

  ① メニューの項目名   `MenuItem::label` の日本語（lib.rs）
  ② スイッチの行名     `toggle_rows` の日本語（toggles.rs）
  ③ 並び替えの語       `sort_label`（render.rs）
  ④ コマンド名         commands.rs の verb が窓版の辞書にもあるか

見ているのは「その語が窓版のメニューを組んでいる場所に出てくるか」までで、
**行ごとの対応までは見ていません**（`チャット` が別のメニューに残っていれば、
一つの表から消えても気づきません）。行の並びは drive.js の `list` で読みます。

窓版に**まだ無い機能**の項目は KNOWN に理由つきで書きます。表が減るのが前進で、
気づかれずに減らせてしまう検査は検査ではないので、ここに書いた分だけが免除です。

    python3 scripts/parity.py           # 揃っているか
    python3 scripts/parity.py --list    # 何を見ているか
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GUI = (ROOT / "gui/renderer.js").read_text(encoding="utf-8")


def region(start, end):
    """renderer.js の一区画。ファイル全体を探すと、ヘルプ画面に同じ語が
    載っているせいでメニューから消えても通ってしまう（実際、最初の版は
    「リネーム」を「名前を変える」に戻しても何も報告しませんでした）。"""
    i = GUI.index(start)
    return GUI[i: GUI.index(end, i)]


def labels_in(text):
    """その区画がメニュー行として出す文字列。丸ごと一致で見ます。

    `label:` の右は三項式のこともある（Mac と Windows で名前が違う行、
    開始と停止で入れ替わる行）ので、その行に出てくる文字列を全部拾います。"""
    out = set()
    for line in text.splitlines():
        if "label:" in line:
            out |= set(re.findall(r"'((?:[^'\\]|\\.)*)'", line.split("label:", 1)[1]))
    # `group(` takes a label that may be `tr(en, ja)`, so take every string on
    # the line rather than only the first argument — the same reason `label:`
    # is read that way just above.
    for line in text.splitlines():
        if "group(" in line:
            out |= set(re.findall(r"'((?:[^'\\]|\\.)*)'", line.split("group(", 1)[1]))
    # `['key', 'label']` — and the label may now be `tr(en, ja)`, so the
    # Japanese is the second string rather than the first.
    out |= {p for p in re.findall(
        r"\['[^']*',\s*(?:tr\([^,]*,\s*)?'((?:[^'\\]|\\.)*)'", text)}
    return {re.sub(r"\s+", " ", x).strip() for x in out}


# ① メニュー ② スイッチ ③ 並び替え — 窓版の側も、その画面を組んでいる場所だけ。
MENU_WORDS = labels_in(region("function viewerRows()", "const cfg = {"))
MENU_WORDS |= labels_in(region("function contextRows()", "/// A row that opens a submenu"))
MENU_WORDS |= labels_in(region("function aiRows()", "const CONTEXT = {"))
MENU_WORDS |= labels_in(region("function menuRows()", "function drawMenu()"))
TOGGLE_WORDS = labels_in(region("const TOGGLES = {", "const SORT_MENU = {"))
# `function sorts()` — 定数から関数になりました（`tr()` を読み込み時に評価すると
# 言語を切り替えても変わらないため。scripts/i18n.py がその検査を持っています）。
SORT_WORDS = labels_in(region("function sorts()", "async function applySort"))

# 退いた言い方。**どちらの前端にも、画面に出す文字列としては残らない。**
#
# 語を揃えるとき、メニューだけ直してヘルプとコマンド辞書を置き去りにすると、
# 同じ窓の中で二通りの呼び方が並びます（実際、メニューを「ゴミファイル検出」に
# した回、ヘルプは「不要さがし」のままでした）。値は代わりに使う語。
RETIRED = {
    "不要さがし": "ゴミファイル検出",
    "不要ファイル検出": "ゴミファイル検出",
    "畳み方の案": "ディレクトリ構成を提案",
    "自由に訊く": "チャット",
    "改名案": "指示でリネーム / AIリネーム",
    "意味検索": "セマンティック検索",
    "メモ帳流": "メモ帳",
    "名前を変える": "リネーム",
    "行き先を指定してコピー": "指定先へコピー",
    "まとめてリネーム": "エディタでリネーム",
    "差分をとる": "左右を比較",
    "重複を探す": "重複ファイルを検出",
}

# 語を数える対象。コメントは読まない ── 経緯としてわざと書いてあるので。
TEXTS = ["gui/renderer.js", "crates/cian-tui/src/lib.rs",
         "crates/cian-tui/src/palette.rs", "crates/cian-tui/src/toggles.rs",
         "crates/cian-tui/src/render.rs", "crates/cian-tui/src/commands.rs"]


def speech(path):
    """その版が画面に出す行だけ。行頭が `//` `///` `*` のものは落とす。"""
    out = []
    for n, line in enumerate((ROOT / path).read_text(encoding="utf-8").splitlines(), 1):
        t = line.lstrip()
        if t.startswith("//") or t.startswith("*") or t.startswith("#"):
            continue
        out.append((n, line))
    return out


# 窓版にまだ無いもの。「無い」ことを覚えておく表で、免除の理由が要ります。
KNOWN = {
}

# 窓版が**エンジンの答えから組み立てる**ラベル。字面はソースに無いので、
# 組み立てている場所があることを見ます。端末版はプラットフォームごとに3通り
# 書いていて、窓版は `cian_core::os::file_manager_name()` の1語を差し込みます。
BUILT = {
    "Finder で表示": r"osCan\.file_manager\} で表示",
    "エクスプローラーで表示": r"osCan\.file_manager\} で表示",
    "ファイルマネージャで表示": r"osCan\.file_manager\} で表示",
}

# ①②③ の語を集める。
def menu_labels():
    lib = (ROOT / "crates/cian-tui/src/lib.rs").read_text(encoding="utf-8")
    # `impl MenuItem` appears twice (is_group, then label); the labels are in
    # the second. Taking the first one found nothing at all and the check sat
    # there saying "揃っています" about twelve words instead of a hundred and
    # nineteen — the failure mode this whole file exists to catch.
    blk = lib[lib.index("fn label(self, lang: Lang)"):]
    blk = blk[: blk.index("\n    }\n")]
    out = []
    for m in re.finditer(r"MenuItem::(\w+) =>", blk):
        seg = blk[m.end(): m.end() + 500]
        ja = re.findall(r'tr\(\s*lang,\s*"(?:[^"\\]|\\.)*",\s*"((?:[^"\\]|\\.)*)"', seg)
        if ja:
            out.append((f"MenuItem::{m.group(1)}", ja[0]))
    return out


def toggle_labels():
    tg = (ROOT / "crates/cian-tui/src/toggles.rs").read_text(encoding="utf-8")
    blk = tg[tg.index("fn toggle_rows"): tg.index("fn toggles_move")]
    return [(f"ToggleId::{i}", ja) for i, ja in
            re.findall(r'ToggleId::(\w+),?\s*\n?\s*tr\(self\.lang,\s*"[^"]*",\s*"([^"]*)"', blk)]


def sort_words():
    rd = (ROOT / "crates/cian-tui/src/render.rs").read_text(encoding="utf-8")
    blk = rd[rd.index("pub(crate) fn sort_label"):]
    blk = blk[: blk.index("\n}\n")]
    return [("sort_label", ja) for ja in re.findall(r'tr\(lang,\s*"[^"]*",\s*"([^"]*)"', blk)]


def tui_verbs():
    cm = (ROOT / "crates/cian-tui/src/commands.rs").read_text(encoding="utf-8")
    verbs = set()
    for m in re.finditer(r'^\s{12}((?:"[a-zA-Z0-9_!-]+"\s*\|\s*)*"[a-zA-Z0-9_!-]+")\s*(?:if [^=]*)?=>',
                         cm, re.M):
        verbs |= set(re.findall(r'"([^"]+)"', m.group(1)))
    return verbs


def gui_verbs():
    names = set()
    for m in re.finditer(r"\{\s*name:\s*'([^']+)'(.*?)\},?\n", GUI):
        names.add(m.group(1))
        al = re.search(r"alias:\s*\[([^\]]*)\]", m.group(2))
        if al:
            names |= set(re.findall(r"'([^']+)'", al.group(1)))
    return names


# ── AI に言う言葉 ────────────────────────────────────────────────────────

AI_CRATES = {
    "cian-tui": ROOT / "crates/cian-tui/src",
    "cian-server": ROOT / "crates/cian-server/src",
}


def ai_prompts():
    """クレートごとの、行内に直書きされた AI システムプロンプト。

    **これも「同じものを同じ言葉で呼んでいるか」の話です。** 人が読むラベルは
    この表が監視していたのに、モデルが読む文章は4本とも両前端に別々のコピーが
    あって、誰も見ていませんでした。コピーは必ずずれます ── ずれても、どちらが
    正しいかを言える人がいません。

    見つけ方は「`"You ` で始まる Rust の文字列リテラル」。cian のプロンプトは
    全部そう始まります（`cian_core::aiprompt` に移したものは定数参照なので、
    ここには写りません）。
    """
    out = {}
    for crate, src in AI_CRATES.items():
        found = set()
        for path in sorted(src.rglob("*.rs")):
            text = path.read_text(encoding="utf-8")
            for m in re.finditer(r'"You [^"]{40,}"', text, re.S):
                # 行継続（`\` + 改行 + 前置空白）を畳んでから比べる。
                body = re.sub(r"\\\s*\n\s*", " ", m.group(0))
                found.add(re.sub(r"\s+", " ", body).strip())
        out[crate] = found
    return out


def shared_prompts():
    """2つのクレートに同じ文章が出ていたら、その文章。"""
    p = ai_prompts()
    return sorted(p["cian-tui"] & p["cian-server"])


def head(label):
    """`コピー  (Ctrl+C)` の左半分。窓版は鍵を別の列に出すので、名前だけを見る。"""
    label = re.sub(r"\s\s+\([^)]*\)$", "", label)
    return re.sub(r"\s+", " ", label).strip()


def main():
    words = menu_labels() + toggle_labels() + sort_words()
    if "--list" in sys.argv:
        for where, ja in words:
            print(f"  {where:28} {head(ja)}")
        print(f"\n  コマンド名 {len(tui_verbs())} 個（commands.rs の verb）")
        print(f"  免除 {len(KNOWN)} 件")
        return 0

    missing = []
    for where, ja in words:
        h = head(ja)
        if not h or h in KNOWN:
            continue
        pool = (TOGGLE_WORDS if where.startswith("ToggleId")
                else SORT_WORDS if where == "sort_label"
                else MENU_WORDS)
        if h in BUILT:
            if not re.search(BUILT[h], GUI):
                missing.append((where, f"{h}（組み立てている場所が見当たりません）"))
            continue
        if h not in pool:
            missing.append((where, h))

    lost = sorted(v for v in tui_verbs() - gui_verbs() if v not in {"-"})

    doubled = shared_prompts()

    retired = []
    for path in TEXTS:
        for n, line in speech(path):
            for bad, good in RETIRED.items():
                if bad in line:
                    retired.append((path, n, bad, good))

    print("=" * 72)
    if not missing and not lost and not retired and not doubled:
        shared = len(ai_prompts()["cian-tui"] | ai_prompts()["cian-server"])
        print(f"  語 {len(words)} 件・コマンド {len(tui_verbs())} 件・"
              f"退いた言い方 {len(RETIRED)} 件、"
              f"両方の前端で揃っています（免除 {len(KNOWN)} 件）")
        print(f"  AI に言う言葉のうち、片方にしか無い直書きは {shared} 本"
              f"（両方にコピーがあるものは 0 本）")
        print("=" * 72)
        return 0
    for where, h in missing:
        print(f"  ■ 端末版の「{h}」が窓版に見当たりません  ({where})")
    for v in lost:
        print(f"  ■ 端末版の :{v} が窓版の辞書にありません")
    for path, n, bad, good in retired:
        print(f"  ■ 退いた言い方「{bad}」が {path}:{n} に出ています → 「{good}」")
    for text in doubled:
        print(f"  ■ 同じ AI プロンプトが両方のクレートに直書きされています")
        print(f"    「{text[:70]}…」")
        print(f"    → cian_core::aiprompt に移して、両方から参照してください")
    print()
    print("  端末版が正です（feedback-gui-keys-from-tui-table）。窓版の言葉を")
    print("  合わせるか、まだ無い機能なら scripts/parity.py の KNOWN に理由を書く。")
    print("=" * 72)
    return 1


if __name__ == "__main__":
    sys.exit(main())
