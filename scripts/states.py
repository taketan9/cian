#!/usr/bin/env python3
"""状態を掛け合わせて、通知なしに消えるものを探します（総ざらいの第4段）。

**`self.popup` は1枠しかありませんでした。** 何かを開くと、ドッキング中の
パネルが未保存ごと消えていました（`open_popup` / `stash_viewer` に集約済み）。
この型のバグは、**一つずつ触っている限り出ません。** どちらも単体では正しく
動くからです。出るのは掛け合わせたときだけです。

    ~/.cache/cian-tui-env/bin/python scripts/states.py
    ~/.cache/cian-tui-env/bin/python scripts/states.py --list   消えた組を全部出す

## やること

軸を2つ選び、**A を立ててから B を立てて、A がまだ立っているか**を見ます。
8軸の順序つき 56 通りです。

- **立っていることは、画面の印で見ます。** 内部の状態を訊くと、「開いている
  と言っているのに画面には出ていない」を見逃します（`report.on === true` で
  3ラウンド「直った」と言い続けた型）
- **B が立たなかった組は、損失として数えません。** シェルに焦点があるときの
  `/` はシェルへ行きます。これは A が消えたのではなく B が立たなかった
  だけで、別の話です
- **消えてよい組は理由つきで書きます**（`FINE`）。書いていない組が出たら
  落ちます

## 数の決まり

- `LEAST_AXES` ── 立たない軸があれば落ちます。**立たない軸は、測っていない
  軸です**
- `FINE` に無い損失は赤。消えてよいと判断したら、理由を書いて足します
"""

from __future__ import annotations

import importlib.util
import os
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("td", ROOT / "scripts" / "tui-drive.py")
td = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(td)


def sandbox() -> str:
    d = tempfile.mkdtemp(prefix="cian-states-")
    for sub in ("from", "to", "config"):
        os.makedirs(os.path.join(d, sub), exist_ok=True)
    open(os.path.join(d, "from/a.txt"), "w").write("one two three\nsecond line\nthird\n")
    open(os.path.join(d, "from/b.md"), "w").write("# 見出し\n\n- [ ] 牛乳\n")
    # 書庫の中に入るための一つ。`G` で末尾へ行くとこれに当たります。
    with zipfile.ZipFile(os.path.join(d, "from/box.zip"), "w") as z:
        z.writestr("inside.txt", "in the zip\n")
        z.writestr("dir/deep.txt", "deeper\n")
    return d


def right_head(t) -> str:
    """右ペインの見出し（上3行の、58 桁から先）。

    閲覧が開いているかは**ここに `—` が出ているか**で見ます。素の見出しは
    `◀ ▶  1 /path…` で、長音符ではなく三点リーダです。`— UTF-8` で測って
    いたときは、`b.md` に F3 が開く **Markdown プレビュー**を「閲覧が立たない」
    と読みました（2026-11-28）── 印は、中身の種類で変わらないところに
    置きます。
    """
    return "\n".join(t.line(y)[58:] for y in range(3))


def left_side(t) -> str:
    """左ペインの字だけ。

    **状態行は「焦点のあるペイン」を映します。** 最初はマークもフィルタも
    状態行で数えていて、閲覧やシェルへ焦点が移った瞬間に「マーク 0」に
    なるのを「マークが消えた」と読みました。10 件の偽の所見を出してから
    気づきました（2026-11-28）。左ペインに残っているものは、左ペインの桁で
    見ます。
    """
    return "\n".join(t.line(y)[:58] for y in range(t.rows))


class Axis:
    """一つの状態。立て方と、立っていることの印。

    `hurt` は**消えたときに何が失われるか**です。順位をつけるために書きます ──
    「ヘルプが閉じた」と「編集が消えた」は同じ損失ではありません。
    """

    def __init__(self, name, keys, mark, hurt):
        self.name = name
        self.keys = keys
        self.mark = mark
        self.hurt = hurt


def axes() -> list[Axis]:
    return [
        # 印はマークの丸そのもの。**左ペインの桁だけを見ます。** 閲覧の
        # 見出しに出る「変更あり」の丸（`a.txt ●`）は右ペインにあります。
        Axis("マーク", [("Space", 0.5)],
             lambda t: "●" in left_side(t),
             "選んだものが消えます。次の操作が別のものに当たります"),
        # 絞り込みが効いていれば、`/a` に当たらない `box.zip` は左に出ません。
        # 状態行の「フィルタ /a」は焦点が移ると消えるので使えません。
        Axis("フィルタ", [("/", 0.6), ("a", 0.4), ("Enter", 0.8)],
             lambda t: "box.zip" not in left_side(t),
             "絞り込みが消えます。画面の件数が変わります"),
        Axis("Visual", [("v", 0.6)],
             lambda t: "VISUAL" in t.status(),
             "選択の途中が消えます"),
        # 閲覧の見出しは、中で何をしていても出ています。`R READ` で測ると
        # 中で Visual や編集に入った瞬間に「閲覧が消えた」と読みます。
        # `gg` `j` で `..` の次（最初のファイル）へ。**どの軸の上でも同じ
        # ものを開くため**です。マークは Space でカーソルが1つ下がるので、
        # 置かずに F3 を押すと軸ごとに違うファイルを見ることになります。
        Axis("閲覧", [("gg", 0.5), ("j", 0.4), ("F3", 1.8)],
             lambda t: "—" in right_head(t),
             "読んでいた場所を失います"),
        # 印は**打った字そのもの**。状態行でも見出しの丸でもなく、
        # 「X が画面に在るか」が、編集が残っているかの唯一の証拠です。
        Axis("未保存の編集", [("gg", 0.5), ("j", 0.4), ("F3", 1.8), ("i", 0.7), ("X", 0.7)],
             lambda t: "Xone two three" in t.text(),
             "**打った字が消えます。** この表でいちばん重い損失です"),
        Axis("シェルに焦点", [("J", 2.0)],
             lambda t: t.status().startswith("S "),
             "打ちかけの行が残ったまま焦点だけ外れます"),
        Axis("ヘルプ", [("?", 1.0)],
             lambda t: "キー一覧" in t.text(),
             "開いたパネルが閉じるだけです。軽い損失です"),
        Axis("書庫の中", [("G", 0.6), ("Enter", 1.8)],
             lambda t: "⊞ box.zip/" in t.text(),
             "見ていた場所を失い、外のフォルダへ戻されます"),
    ]


# 消えてよい組。**理由を書きます。**
#
# 鍵は「A を立てたあとで B を立てたら A が消えた」の (A, B)。
FINE = {
    ("Visual", "マーク"): "Space は選択を確定して Visual を抜けます（vim と同じ）",
    ("マーク", "書庫の中"):
        "書庫へ入ると一覧が入れ替わり、外のファイルを指していたマークは"
        "指すものを失います（フォルダを移るのと同じ扱いです）",
}

LEAST_AXES = 8


def raise_axis(t, axis: Axis) -> bool:
    for key, wait in axis.keys:
        t.send(key, wait)
    return axis.mark(t)


def main() -> int:
    show = "--list" in sys.argv
    d = sandbox()
    all_axes = axes()
    print("=" * 72)
    print("  状態を掛け合わせる。総ざらいの第4段")
    print("=" * 72)

    # ① 軸が立つか。立たない軸は、測っていない軸です。
    standing = []
    for a in all_axes:
        t = td.start(d)
        t.pump(3.0)
        t.send("Esc", 0.3)
        ok = raise_axis(t, a)
        t.close()
        print(f"  {'✓' if ok else '✗'} 軸 {a.name}")
        if ok:
            standing.append(a)
    print()
    if len(standing) < LEAST_AXES:
        print(f"  ✗ 立った軸が {len(standing)} 本しかありません（{LEAST_AXES} 本のはず）")
        print("    立たない軸は測っていない軸です。立て方か印が古くなっています")
        return 1

    # ② 掛け合わせ。
    lost: list[tuple[str, str, str]] = []
    blocked: list[tuple[str, str]] = []
    inside: list[tuple[str, str]] = []
    hidden: list[tuple[str, str]] = []
    pairs = 0
    for a in standing:
        for b in standing:
            if a.name == b.name:
                continue
            pairs += 1
            t = td.start(d)
            t.pump(3.0)
            t.send("Esc", 0.3)
            if not raise_axis(t, a):
                t.close()
                print(f"  ✗ {a.name} が立ちません（素の画面から）")
                return 1
            # **B の印が最初から立っているなら、その組は測れません。**
            # 理由は2つあります。B が A の中に在る場合（編集はビューアの中で
            # 起きるので、`未保存の編集` の上では `閲覧` が既に立っています）
            # と、A が B の印を**覆って**成り立たせている場合（ヘルプのパネルが
            # 左ペインを隠すので、「`box.zip` が見えない」で測るフィルタは
            # 立って見えます）。どちらも「この組は測れない」です。測れない
            # ものを測ると、挿入中の本文に `ggj` と打ち込んで「編集が消えた」
            # と読みます（2026-11-28、実際にやりました）。
            if b.mark(t):
                inside.append((a.name, b.name))
                t.close()
                continue
            b_on = raise_axis(t, b)
            a_on = a.mark(t)
            status = t.status()[:70]
            # **消えたのか、覆われただけなのか。** ヘルプのパネルは左ペインを
            # 覆うので、「`box.zip` が見えない」で測っているフィルタはパネルの
            # 下に隠れます。B を閉じて A が戻るなら、消えてはいません ──
            # 人が確かめるときと同じ手です（2026-11-28）。
            covered = False
            if b_on and not a_on:
                for _ in range(2):
                    t.send("Esc", 0.6)
                covered = a.mark(t)
            t.close()
            if not b_on:
                blocked.append((a.name, b.name))
                continue
            if a_on:
                continue
            if covered:
                hidden.append((a.name, b.name))
                continue
            lost.append((a.name, b.name, status))

    print(f"  組 {pairs} 通りを見ました（立たなかった組 {len(blocked)}"
          f"／B の印が先に立っていた組 {len(inside)}"
          f"／B が A を覆っただけの組 {len(hidden)}）")
    print()
    news = [x for x in lost if (x[0], x[1]) not in FINE]
    known = [x for x in lost if (x[0], x[1]) in FINE]
    print(f"  消えた組 {len(lost)}（理由を書いてあるもの {len(known)}）")
    if show:
        for a, b, status in lost:
            why = FINE.get((a, b), "**理由が書かれていません**")
            print(f"    {a} → {b} を立てると {a} が消えます")
            print(f"      {why}")
            print(f"      そのときの状態行: {status}")
        if blocked:
            print()
            print("  立たなかった組（A の上では B を立てられません）")
            for a, b in blocked:
                print(f"    {a} の上で {b}")
        if inside:
            print()
            print("  B の印が先に立っていた組（測れません）")
            for a, b in inside:
                print(f"    {a} の上では {b} の印が最初から立っています")
        if hidden:
            print()
            print("  覆われただけの組（B を閉じたら A が戻りました）")
            for a, b in hidden:
                print(f"    {b} が {a} を覆います")
    print()
    print("=" * 72)
    if news:
        for a, b, status in news:
            hurt = next(x.hurt for x in standing if x.name == a)
            print(f"  ✗ {a} → {b} で {a} が消えます")
            print(f"     失われるもの: {hurt}")
            print(f"     そのときの状態行: {status}")
        print()
        print("  消えてよいなら理由を `FINE` に書いてください。通知なしに消えるのは駄目です")
        return 1
    print("  掛け合わせても、理由の無い消え方はありません")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
