#!/usr/bin/env python3
"""Is every promise in gui/REQUESTS.ja.md still kept?

The failure this exists for: a thing Taketan asked for was built, and then
removed by a later design pass — mine — with a comment arguing against them.
Nothing in the code said where that line had come from, so on the second look
it was indistinguishable from a line I had chosen myself.

This does not check that the code is good. It checks that a request Taketan
made is still satisfied, and names the request when it is not. A failure here
is not "fix the regex"; it is "you removed something that was asked for — go
and ask before you do".

    python3 scripts/requests.py          # pass/fail
    python3 scripts/requests.py --list   # what each row looks for

Each row's check column is `path ~ regex` (must match) or `path !~ regex`
(must not appear anywhere in the file).

The negative form is a separate operator on purpose. It was first written as
a negative lookahead — `^(?!.*やめる).*$` — which passes on any file with one
line that does not contain the word, i.e. always. Three rows were asserting
nothing and saying they were fine, which is the failure mode this whole file
exists to make impossible. Mutation-test every check: break it, watch it
complain, put it back.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TABLE = ROOT / "gui" / "REQUESTS.ja.md"


def rows() -> list[dict]:
    """Every data row of the one table in the register."""
    out: list[dict] = []
    for line in TABLE.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        # A row whose first cell is a number is a data row. Anything else is
        # the header or the separator, and those are walked past.
        if not cells or not cells[0].isdigit():
            continue
        # **A data row with the wrong number of cells is broken, not absent.**
        # Row 22 was written with `\|\|` in its check — a perfectly good
        # regex, and two more pipes than a markdown table has room for. It
        # split into seven cells, fell through the `continue` below, and the
        # register said "24 件、すべて守られています" while holding 25 rows.
        # A checker that answers a question it did not read is worse than one
        # that refuses.
        if len(cells) != 5:
            out.append({
                "n": cells[0],
                "asked": cells[2] if len(cells) > 2 else "",
                "error": f"5列のはずが {len(cells)} 列です — 検査に `|` は書けません（表が割れます）",
            })
            continue
        num, date, asked, what, check = cells
        m = re.match(r"`([^\s~!]+)\s*(!?~)\s*(.+)`$", check)
        if not m:
            out.append({"n": num, "asked": asked, "error": f"検査の書き方が読めません: {check}"})
            continue
        out.append({
            "n": num, "date": date, "asked": asked, "what": what,
            "path": m.group(1), "negate": m.group(2) == "!~", "pattern": m.group(3),
        })
    return out


# `(?s)(?=.*A)(?=.*B)…` ── この台帳でいちばん多い書き方。「A も B も、どこかに
# 有ること」を1つの正規表現で言うためのものだが、**外れたときに壊滅的に遅い**:
# 600KB の `renderer.js` に対して、`.*` の伸縮を先読みの数だけ掛け合わせる。
#
# 2026-09-20 に実際に固まった。**守られていない依頼が、赤ではなく無反応として
# 出る** ── この台帳がいちばんしてはいけないことだ。意味は「それぞれがどこかに
# 有ること」でしかないので、先読みを切り離して1つずつ探す。線形で、同じ答えが出る。
LOOKAHEADS = re.compile(r"^\(\?s\)((?:\(\?=\.\*.*?\))+)$", re.DOTALL)


def matches(pattern: str, text: str) -> int | None:
    """`pattern` が当たった位置。当たらなければ `None`。"""
    m = LOOKAHEADS.match(pattern)
    if m:
        # `(?=.*X)` を1つずつに割る。入れ子の括弧があるので、深さで数える。
        parts, depth, start = [], 0, None
        body = m.group(1)
        for i, c in enumerate(body):
            if c == "(":
                if depth == 0:
                    start = i
                depth += 1
            elif c == ")":
                depth -= 1
                if depth == 0:
                    parts.append(body[start + len("(?=.*") : i])
        first = None
        for part in parts:
            hit = re.search(part, text, re.DOTALL | re.MULTILINE)
            if not hit:
                return None
            if first is None:
                first = hit.start()
        return first if first is not None else 0
    hit = re.search(pattern, text, re.MULTILINE)
    return hit.start() if hit else None


def main() -> int:
    listing = "--list" in sys.argv
    entries = rows()
    if not entries:
        print(f"！ {TABLE} に読める行がありません")
        return 1

    bad: list[tuple[dict, str]] = []
    for e in entries:
        if "error" in e:
            bad.append((e, e["error"]))
            continue
        f = ROOT / e["path"]
        if not f.exists():
            bad.append((e, f"{e['path']} がありません"))
            continue
        text = f.read_text(encoding="utf-8")
        try:
            hit = matches(e["pattern"], text)
        except re.error as err:
            bad.append((e, f"正規表現が壊れています: {err}"))
            continue
        if e["negate"] and hit is not None:
            line = text[: hit].count("\n") + 1
            bad.append((e, f"{e['path']}:{line} に、あってはならないものがあります"))
        elif not e["negate"] and hit is None:
            bad.append((e, f"{e['path']} が条件を満たしていません"))
        elif listing:
            op = "!~" if e["negate"] else "~"
            print(f"  ok  {e['n']:>2}. {e['asked'][:44]}")
            print(f"          {e['path']}  {op}  {e['pattern'][:60]}")

    print("=" * 72)
    if bad:
        print(f"守られていない依頼が {len(bad)} 件あります。\n")
        for e, why in bad:
            print(f"  ✗ {e['n']}. {e.get('date', '')}  {e['asked']}")
            print(f"      満たすもの: {e.get('what', '(未記入)')}")
            print(f"      {why}\n")
        print("直す前に、まず訊いてください。これは頼まれて入ったものです。")
        print("=" * 72)
        return 1
    print(f"依頼 {len(entries)} 件、すべて守られています")
    print("=" * 72)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
