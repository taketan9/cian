#!/usr/bin/env python3
"""Does CLAUDE.md's rules table name gates that actually exist?

The failure this exists for, from 2026-10-30: the rule "put the pty venv in
`~/.cache`, never `/tmp` — the cleaner eats it" had been written down after
being hit twice, and was still true. But the four places that actually *hand
out the instruction* — both skills, and the error messages of `tui-drive.py`
and `sweep.py` — all still said `/tmp`. The rule was in the memory; the path
to stepping on it a third time was in the tooling. **A rule is not in force
until something falls over when it is broken.**

So CLAUDE.md states each rule next to the gate that watches it, and this is
the gate on that: every gate the table names must exist, and the one rule
whose gate is this file (the venv path) is checked here directly.

    python3 scripts/claudemd.py          # pass/fail
    python3 scripts/claudemd.py --list   # every rule and what was found

What it does **not** do: judge whether a gate is any good. `requests.py`
asserting nothing and reporting fine is exactly the failure this project
keeps hitting, and a name-exists check cannot see it. That is what
mutation-testing each gate by hand is for.

The floors below are deliberate. A check that counts has to say how few is
too few, or it goes quiet without going red — `keycover.py` lost all but two
of its keys and kept printing a percentage.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 下限 ── 表が縮んだら、黙って通さずに落ちる
LEAST_RULES = 12
LEAST_GATES = 8

# 関門の列に書いてあっても、実在を確かめる相手がいないもの
PROSE = ("（手順", "(手順")


HEADING = "## \u26d4 \u958b\u767a\u306e\u30eb\u30fc\u30eb"


def table_rows(text: str) -> list[tuple[str, str]]:
    """規則の節にある「ルール | 関門」の表だけを読む。

    CLAUDE.md には2列の表が他にもある（「手元で確かめられないこと」）。
    最初に全部の表を読んでいて、そちらの行を関門の無い規則として並べた ──
    **読む範囲を言わない検査は、別のものを測る。**
    """
    lines = text.splitlines()
    start = next((i for i, l in enumerate(lines) if l.startswith(HEADING)), None)
    if start is None:
        return []
    rows = []
    seen_table = False
    for line in lines[start + 1:]:
        if line.startswith("## "):
            break
        if not line.startswith("|"):
            if seen_table and line.strip() == "":
                continue
            if seen_table and not line.startswith("|"):
                break
            continue
        seen_table = True
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) != 2:
            continue
        rule, gate = cells
        if rule == "ルール" or set(rule) <= set("- :"):
            continue
        rows.append((rule, gate))
    return rows


def rust_test_names() -> set[str]:
    names = set()
    for f in ROOT.joinpath("crates").rglob("*.rs"):
        for m in re.finditer(r"\bfn ([a-z_0-9]+)\(", f.read_text(encoding="utf-8")):
            names.add(m.group(1))
    return names


def gate_exists(gate: str, tests: set[str]) -> tuple[bool, str]:
    """関門の列から、実在を確かめられるものを拾って確かめる。

    `scripts/x.py` はファイル、`` `some_test_name` `` は Rust の関数、
    `cargo …` は走らせずに素通りさせる（CI が同じものを走らせている）。
    """
    for tok in re.findall(r"`([^`]+)`", gate):
        tok = tok.strip()
        if tok.startswith("cargo "):
            return True, f"cargo: {tok.split()[1]}"
        if "/" in tok or tok.endswith(".py"):
            p = ROOT / tok
            if not p.exists():
                return False, f"そんなファイルは無い: {tok}"
            return True, tok
        if re.fullmatch(r"[a-z_0-9]+", tok):
            if tok not in tests:
                return False, f"そんなテストは無い: {tok}"
            return True, f"{tok}()"
    if any(gate.startswith(p) for p in PROSE):
        return True, "手順（関門なし）"
    return False, "関門に、実在を確かめられるものが書かれていない"


def tmp_venvs() -> list[str]:
    """追跡ファイルのどれかが venv を `/tmp` に向けていないか。

    `tempfile.mkdtemp` のような、その場で作って捨てるものは対象外 ──
    見るのは**人に渡す手順**として `/tmp` の venv を名乗るものだけ。
    """
    out = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.split()
    bad = []
    for rel in out:
        p = ROOT / rel
        if not p.is_file() or p.suffix in (".png", ".ico", ".zip", ".ttf", ".woff2"):
            continue
        try:
            text = p.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        for m in re.finditer(r"/tmp/[A-Za-z0-9._-]*env[A-Za-z0-9._/-]*", text):
            bad.append(f"{rel}: {m.group(0)}")
    return bad


def main() -> int:
    listing = "--list" in sys.argv
    md = ROOT / "CLAUDE.md"
    print("=" * 72)
    print("  CLAUDE.md の規則に、落ちる関門が付いているか")
    print("=" * 72)
    if not md.exists():
        print("  ✗ CLAUDE.md が無い")
        print("=" * 72)
        return 1

    rows = table_rows(md.read_text(encoding="utf-8"))
    tests = rust_test_names()
    bad: list[str] = []
    gated = 0

    for rule, gate in rows:
        ok, why = gate_exists(gate, tests)
        if ok and not why.startswith("手順"):
            gated += 1
        if not ok:
            bad.append(f"{rule[:52]} ── {why}")
        if listing:
            mark = "✓" if ok else "✗"
            print(f"  {mark} {rule[:58]}")
            print(f"      {why}")

    if len(rows) < LEAST_RULES:
        bad.append(
            f"規則が {len(rows)} 行しか読めていない（下限 {LEAST_RULES}）"
            " ── 表の形が変わって、この検査が黙っている"
        )
    if gated < LEAST_GATES:
        bad.append(
            f"関門の付いた規則が {gated} しかない（下限 {LEAST_GATES}）"
        )

    for line in tmp_venvs():
        bad.append(f"venv を `/tmp` に向けている ── 掃除で消える: {line}")

    if bad:
        print(f"\n  関門の無い規則・届かない関門が {len(bad)} 件あります。\n")
        for b in bad:
            print(f"  ✗ {b}")
        print("\n  規則は、落ちる関門が付くまで効いていません。")
        print("=" * 72)
        return 1
    print(f"  規則 {len(rows)} 行、うち {gated} 行に実在する関門が付いています")
    print("  venv を `/tmp` に向けているファイルはありません")
    print("=" * 72)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
