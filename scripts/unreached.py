#!/usr/bin/env python3
"""Does this version say what Claude could not reach from the Mac?

`UNREACHED.ja.md` holds, per version, the things that were shipped without
being exercised on real hardware — Windows, Taketan's JIS keyboard and IME,
the company's AI endpoint and shares, and his eyes.

This is not a homework list for him. The point of the gate is the other
direction: **it stops "I verified it" from quietly widening.** Everything the
Mac can reach gets reached here — the terminal through a real pty
(`tui-drive.py`, `sweep.py`), the window through real keys and clicks
(`drive.js`). What is left over is small, and has to be written down rather
than left in a chat message that scrolls away.

Most versions should say 無し, and that is the healthy answer. The failure
this guards against is the version with **nothing written at all**, which
reads identically to "nothing was unreachable" while meaning "nobody asked".

    python3 scripts/unreached.py          # pass/fail
    python3 scripts/unreached.py --list   # every version section

Writing a section costs one line. `## v3.4.15` followed by `**無し**` is a
complete, honest answer when it is true.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOC = ROOT / "UNREACHED.ja.md"

# 下限 ── 節がまるごと読めなくなったら、黙って通さずに落ちる
LEAST_SECTIONS = 2

NONE_MARKS = ("無し", "なし")


def version() -> str:
    """`Cargo.toml` の workspace の版。ここが唯一の出どころ。"""
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    block = text.split("[workspace.package]", 1)[1]
    m = re.search(r'^version\s*=\s*"([^"]+)"', block, re.M)
    if not m:
        raise SystemExit("Cargo.toml の [workspace.package] に version が無い")
    return m.group(1)


def sections(text: str) -> dict[str, str]:
    """`## v3.4.14` から次の `## ` までを、版ごとに。"""
    out: dict[str, str] = {}
    cur: str | None = None
    body: list[str] = []
    for line in text.splitlines():
        m = re.match(r"^## v(\d+\.\d+\.\d+)\s*$", line)
        if m:
            if cur:
                out[cur] = "\n".join(body)
            cur, body = m.group(1), []
            continue
        if cur:
            body.append(line)
    if cur:
        out[cur] = "\n".join(body)
    return out


def judge(body: str) -> tuple[bool, str]:
    """節が答えになっているか。

    空の節は「届かなかったものは無い」と読めてしまうのに、実際は誰も
    考えていない ── **読めてしまう空白**がこの検査の相手。
    """
    rows = [
        l
        for l in body.splitlines()
        if l.startswith("|") and not set(l) <= set("| -:") and not l.startswith("| 何が")
    ]
    said_none = any(any(w in l for w in NONE_MARKS) for l in body.splitlines() if l.strip())
    if rows and said_none:
        return False, "行が並んでいるのに「無し」とも書いてある（どちらか）"
    if rows:
        return True, f"{len(rows)} 件"
    if said_none:
        return True, "無し"
    return False, "空 ── 「無し」と書くか、届かなかったものを書くこと"


def main() -> int:
    listing = "--list" in sys.argv
    print("=" * 72)
    print("  この版が、手元から届かなかったところを言っているか")
    print("=" * 72)
    if not DOC.exists():
        print(f"  ✗ {DOC.name} が無い")
        print("=" * 72)
        return 1

    ver = version()
    secs = sections(DOC.read_text(encoding="utf-8"))
    bad: list[str] = []

    for v, body in secs.items():
        ok, why = judge(body)
        if not ok:
            bad.append(f"v{v}: {why}")
        if listing:
            print(f"  {'✓' if ok else '✗'} v{v}  {why}")

    if ver not in secs:
        bad.append(
            f"いまの版 v{ver} の節が無い ── 届かなかったものを書くか、"
            "`## v{v}` に `**無し**` の1行を書くこと".replace("{v}", ver)
        )
    if len(secs) < LEAST_SECTIONS:
        bad.append(
            f"節が {len(secs)} しか読めていない（下限 {LEAST_SECTIONS}）"
            " ── 見出しの形が変わって、この検査が黙っている"
        )

    if bad:
        print(f"\n  {len(bad)} 件あります。\n")
        for b in bad:
            print(f"  ✗ {b}")
        print("\n  「確かめた」と言える範囲は、物差しが届く範囲だけです。")
        print("=" * 72)
        return 1
    print(f"  版 {len(secs)} 節、いまの v{ver} も答えています")
    print("=" * 72)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
