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

# 下限：表が縮んだら、通さずに失敗する
LEAST_RULES = 12
LEAST_GATES = 8

# 検査の列に書いてあっても、実在を確かめる相手がいないもの
PROSE = ("（手順", "(手順")


HEADING = "## \u26d4 \u958b\u767a\u306e\u30eb\u30fc\u30eb"


def table_rows(text: str) -> list[tuple[str, str]]:
    """規則の章にある「ルール | 検査」の表だけを読む。

    CLAUDE.md には2列の表が他にもある（「手元で確かめられないこと」）。
    最初に全部の表を読んでいて、そちらの行を検査の無い規則として並べた。
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
    """検査の列から、実在を確かめられるものを拾って確かめる。

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
        return True, "手順（検査なし）"
    return False, "検査に、実在を確かめられるものが書かれていない"


# Windows のジョブが呼ぶ検査は、最低この数だけ見つかること。
# 取り落として「所見なし」と言わないための下限です。
LEAST_WIN_SCRIPTS = 2


def cp1252_deaths() -> list[str]:
    """Windows のジョブが呼ぶ Python が、cp1252 で落ちないか。

    **Windows の runner の標準出力は cp1252 です。** 日本語を1行出した瞬間に
    `UnicodeEncodeError` で落ち、「資材が悪い」ではなく**検査が壊れた**で
    ジョブが赤くなります。2026-10-31 に `buildstamp.py` で踏み、直しをその
    ファイルに書きました。**2026-11-28、新しい検査を足した日に同じ穴へ落ちま
    した。** 直しを書き写す必要を、どの検査も監視していなかったためです。

    見るのは「Windows で走るジョブが名指しした `scripts/*.py`」だけです。
    手元でしか走らない検査に同じ作法を求めても、守る理由がありません。
    """
    import re

    found, bad = 0, []
    for wf in sorted((ROOT / ".github" / "workflows").glob("*.yml")):
        text = wf.read_text(encoding="utf-8")
        # ジョブごとに切って、`windows-latest` で走るものだけを見ます。
        for chunk in re.split(r"\n  (?=[A-Za-z][\w-]*:\n)", text):
            if "windows-latest" not in chunk:
                continue
            for name in sorted(set(re.findall(r"scripts/([\w-]+)\.py", chunk))):
                at = ROOT / "scripts" / f"{name}.py"
                if not at.is_file():
                    bad.append(f"{wf.name} が呼ぶ scripts/{name}.py がありません")
                    continue
                found += 1
                body = at.read_text(encoding="utf-8")
                if "reconfigure(encoding=" not in body:
                    bad.append(
                        f"scripts/{name}.py は Windows のジョブが呼ぶのに、"
                        "標準出力を utf-8 に直していません（cp1252 で落ちます）"
                    )
    if found < LEAST_WIN_SCRIPTS:
        bad.append(
            f"Windows のジョブが呼ぶ検査を {found} 本しか見つけられていません"
            f"（下限 {LEAST_WIN_SCRIPTS}）。この検査が何も報告していません"
        )
    return bad


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
    print("  CLAUDE.md の規則に、落ちる検査が付いているか")
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
            " ── 表の形が変わって、この検査が何も報告しない"
        )
    if gated < LEAST_GATES:
        bad.append(
            f"検査の付いた規則が {gated} しかない（下限 {LEAST_GATES}）"
        )

    for line in tmp_venvs():
        bad.append(f"venv を `/tmp` に向けている。掃除で消える: {line}")

    bad.extend(cp1252_deaths())

    if bad:
        print(f"\n  検査の無い規則・届かない検査が {len(bad)} 件あります。\n")
        for b in bad:
            print(f"  ✗ {b}")
        print("\n  規則は、落ちる検査が付くまで効いていません。")
        print("=" * 72)
        return 1
    print(f"  規則 {len(rows)} 行、うち {gated} 行に実在する検査が付いています")
    print("  venv を `/tmp` に向けているファイルはありません")
    print("  Windows のジョブが呼ぶ検査は、cp1252 で落ちません")
    print("=" * 72)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
