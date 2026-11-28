#!/usr/bin/env python3
"""Which keys does the window promise, and which has anyone ever pressed?

The recurring failure in this front end is not a key that is missing. It is a
key that is written, listed in `?`, and never arrives — `Ctrl+E` sat in the
help for days while Monaco quietly took the chord for "go to end of line", and
`Ctrl+C` / `Ctrl+X` / `Ctrl+V` did nothing at all in vim style. Both were found
by a person on another continent pressing them.

`scripts/audit.py` cannot see this: it checks that the *name* on the other side
of a binding exists, which is true in both of those cases. The thing that was
false is that the keystroke reaches the code. Only pressing it can tell you.

So this counts one gap: keys the help names, against keys `gui/drive.js` sends.
It does not prove a key works — a press whose effect nobody checks is still
worth little — but a key that has never once been sent is a key nobody has any
evidence about, and that set should be small and deliberate rather than most of
the keyboard.

    python3 scripts/keycover.py          # the gap
    python3 scripts/keycover.py --list   # every unpressed key
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RENDERER = ROOT / "gui" / "renderer.js"
DRIVER = ROOT / "gui" / "drive.js"

# Keys that are the platform's or the terminal's, not this window's, and are
# not going to be sent by a driver. Listed rather than pattern-matched, so
# adding one is a decision somebody made on purpose.
NOT_DRIVEN = {
    # Chords the OS or the browser owns; pressing them through CDP tests
    # nothing about cian.
    "Ctrl+Shift+矢印",  # macOS Mission Control eats it; verified on Windows only
    "ドラッグで選択",
    "ドラッグ選択",
    "右クリック",
    "Ctrl+クリック",
    # Ranges and prose that name a family rather than one keystroke.
    "F1〜F8",
    "F1-F8",
    "j/k",
    "gg/G",
    # 一つの打鍵ではなく回数や向きを言っているもの。押すことはできるが、
    # 「押した」で満たされる主張ではない。
    "Esc×3",
    "Backspace×3",
    "←→",
    "Shift+←→",
    "+/−",
}


# ヘルプの行が何行くらいあるはずか、の下限。
#
# **この検査は一度何も報告しませんでした。** 下の正規表現は説明が素の文字列であることを
# 前提にしていて（`['Ctrl+S', '保存']`）、窓版を `tr(en, ja)` に通した回に
# ほぼ全部が `['Ctrl+S', tr("save", '保存')]` になり、拾える行が 72 → 2 に
# 落ちました。それでも百分率は出るので、緑に見えたまま数か月動いていました。
# 数が崩れたら**落ちる**ようにしておけば、次に形が変わったとき取りこぼしを報告します。
FLOOR = 40


def documented() -> set[str]:
    """Every key the help's first column names."""
    src = RENDERER.read_text(encoding="utf-8")
    keys: set[str] = set()
    # 説明は素の文字列でも `tr(...)` でもよい。
    pattern = r"\['([^']*(?:Ctrl|Shift|Alt|F\d|Enter|Esc|Space)[^']*)',\s*(?:'|\"|`|tr\()"
    for m in re.finditer(pattern, src):
        # `/` と `、` の両方で割る。ヘルプは「C 、Ctrl+Shift+P 、Ctrl+,」の
        # ように読点で三つ並べる書き方をしていて、そこを割らないと
        # **その三つはどう押しても一致しない。** 押していないのではなく、
        # 数え方のせいで永久に未達だった。天井が誤っていると、割合も誤りになる。
        for part in re.split(r"\s*/\s*|\s*、\s*", m.group(1)):
            part = part.strip()
            # `:theme` and friends are commands, tested by name elsewhere.
            if part and not part.startswith(":"):
                keys.add(part)
    if len(keys) < FLOOR:
        raise SystemExit(
            f"ヘルプから拾えたキーが {len(keys)} 種しかありません（下限 {FLOOR}）。\n"
            "検査が壊れています。ヘルプ行の書き方が変わったなら、"
            "この正規表現を直してから数えてください。"
        )
    return keys


def driven() -> set[str]:
    """Every key gui/drive.js sends, in the spelling the help uses."""
    src = DRIVER.read_text(encoding="utf-8")
    sent: set[str] = set()
    # 記号も拾う。`Ctrl+]` や `Ctrl+,` は押しているのに、この正規表現が
    # 英字とF数字しか見ていなかったので「押していない」と数えられていた。
    token = (
        r"'((?:Ctrl\+|Shift\+|Alt\+|Mod\+)*"
        r"(?:F\d{1,2}|Enter|Escape|Esc|Tab|Space|Arrow\w+|Left|Right|Up|Down"
        r"|Page\w+|PgUp|PgDn|Home|End|Bksp|Del"
        r"|[A-Za-z0-9]|[\[\],.;=+\-/?]))'"
    )
    for k in re.findall(token, src):
        sent.add(k)
        # The help writes Esc, the driver writes Escape; and the driver's
        # `Mod` is Ctrl on Windows, which is the spelling the help uses.
        sent.add(k.replace("Escape", "Esc").replace("Mod+", "Ctrl+"))
    return sent


# 同じキーの別の書き方。ヘルパーが押しているのに数えられない、を無くす ──
# **天井も床も、綴りのせいで動いてはいけない。**
ALIAS = {
    "←": "ArrowLeft", "→": "ArrowRight", "↑": "ArrowUp", "↓": "ArrowDown",
    "+Left": "+ArrowLeft", "+Right": "+ArrowRight", "+Up": "+ArrowUp", "+Down": "+ArrowDown",
    "Backspace": "Bksp", "PageUp": "PgUp", "PageDown": "PgDn",
    "Escape": "Esc", "Delete": "Del",
}


def normalise(k: str) -> str:
    k = k.replace("Escape", "Esc").replace(" ", "")
    for a, b in ALIAS.items():
        k = k.replace(a, b)
    return k


def main() -> int:
    # 除外も綴りを揃えてから当てる。`Esc ×3` と `Esc×3` が別物として
    # 扱われて、除外したはずのものが未達に残っていた。
    skip = {normalise(k) for k in NOT_DRIVEN}
    want = {k for k in documented() if normalise(k) not in skip}
    have = {normalise(k) for k in driven()}
    # A chord counts as driven if the driver sends it in any spelling; the
    # help writes `Ctrl+E`, the driver `Ctrl+e`.
    missing = sorted(k for k in want if normalise(k).lower() not in {h.lower() for h in have})

    covered = len(want) - len(missing)
    pct = 100 * covered / len(want) if want else 100
    print("=" * 72)
    print(f"ヘルプが名前を挙げるキー : {len(want)} 種")
    print(f"drive.js が押すキー      : {covered} 種  ({pct:.0f}%)")
    print(f"一度も押されていないキー : {len(missing)} 種")
    print("=" * 72)

    if "--list" in sys.argv and missing:
        print("\n一度も押されていないもの:")
        for k in missing:
            print(f"  {k}")
        print(
            "\n押していないキーは「動く証拠が無いキー」です。Ctrl+E と vim 流の\n"
            "Ctrl+C/X/V はどちらもこの集合にいて、実機で見つかりました。"
        )
    elif missing:
        print("\n--list で一覧が出ます。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
