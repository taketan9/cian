#!/usr/bin/env python3
"""資材は、自分がどのビルドかを正しく名乗るか。

    python3 scripts/buildstamp.py target/release/cian-tui
    python3 scripts/buildstamp.py cian-tui-win-x64.exe cian-server-win-x64.exe

渡した実行ファイルの中から `cian <版> (<SHA>)` を**バイト列として**拾い
（走らせられるものには `--version` も訊いて）、それが

  * `Cargo.toml` の `[workspace.package]` の版と一致するか
  * いまの `git rev-parse --short HEAD` と一致するか
  * **`-dirty` でも `unknown` でもないか**

を確かめます。CI の、資材を staging した直後に置く関門です。

## なぜ要るか

**v3.4.14 と v3.4.15 の `cian-tui-win-x64.exe` は、どちらも `-dirty` と名乗って
いた。** 原因は Windows ジョブが `cargo build` の前に書く `font.zip` と `font/`
で、`.gitignore` に無いので `crates/cian-tui/build.rs` の
`git status --porcelain` が空でなくなる。ビルド自体はタグの commit そのもの
なのに、資材は「手元で編集が乗っている」と言い続けていた。

`-dirty` は**彼が「直っていない」と言ったときに最初に訊くもの**で、
「それは編集中のビルドです」と「それは配った資材です」を分ける唯一の印。
公式の資材が常に `-dirty` を出していると、その問いが答えにならない。

**8分かけて作った資材が、自分の名前を間違えていた。** 気づいたのは
Mac から `grep -a` で exe の中の文字列を読んだからで、Windows が無くても
できる検査だった ── だから機械にした。**Windows の exe を Mac から見られる**
のがこの形の値段で、CI の中だけでなく、公開された資材を落としてそのまま
当てられます:

    gh release download v3.4.16 -D /tmp/r -p "cian-tui-win-x64.exe.zip"
    unzip -j /tmp/r/cian-tui-win-x64.exe.zip -d /tmp/r
    python3 scripts/buildstamp.py /tmp/r/cian-tui-win-x64.exe

`cian-server` は `--version` を持たない（版は JSON の `version` で答える）ので、
そちらはバイト列だけで見ます。

## 手元で走らせると落ちることがある（それが正しい）

編集中の作業ツリーで建てたバイナリは `-dirty` と名乗り、この検査は落ちます。
**それは誤検出ではありません。** これは「配るもの」に当てる物差しで、
手元で試すなら commit 済みの状態で建ててから。
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

# **Windows の runner の標準出力は cp1252。** 日本語を1行出した瞬間に
# `UnicodeEncodeError` で落ち、関門が「資材が悪い」のではなく「関門が壊れた」で
# ジョブを赤くする（2026-10-31、実際にそうなった）。環境変数で直すと、次に
# 誰かが別のジョブから呼んだときにまた踏むので、ここで直す。
for stream in (sys.stdout, sys.stderr):
    try:
        stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, OSError):
        pass

ROOT = Path(__file__).resolve().parent.parent

STAMP = re.compile(r"^cian (\S+) \(([^)]+)\)$")


def want_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    block = text.split("[workspace.package]", 1)[1]
    m = re.search(r'^version\s*=\s*"([^"]+)"', block, re.M)
    if not m:
        raise SystemExit("Cargo.toml の [workspace.package] に version が無い")
    return m.group(1)


def want_sha() -> str:
    out = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        raise SystemExit("git rev-parse が通りません ── git の中で走らせること")
    return out.stdout.strip()


BAKED = re.compile(rb"cian \d+\.\d+\.\d+ \([0-9a-f]{7,}(?:-dirty)?\)")


def baked(path: str) -> list[str]:
    """実行ファイルの中に焼かれている `cian <版> (<SHA>)` を、重複なく。

    **走らせずに読む。** Windows の exe でも、他の機械で作った資材でも同じ
    ように当たるのがこの検査の値打ちで、実際にこの形でしか見つからなかった。
    """
    blob = Path(path).read_bytes()
    out: list[str] = []
    for m in BAKED.finditer(blob):
        s = m.group(0).decode("ascii")
        if s not in out:
            out.append(s)
    return out


def said(exe: str) -> str | None:
    """`--version` の1行目。持っていなければ None（`cian-server` がそれ）。"""
    # **止まることがある。** `--version` は端末を要らないはずだが、runner の上で
    # 何かを待ったら関門が8分のジョブを道連れにする ── 待たない。中の文字列は
    # どちらにしても読めるので、ここで読めないことは落ちる理由にはしない。
    # **裸のファイル名を渡すと PATH を探しにいく。** CI は `cian-tui-macos.bin`
    # のように cwd のものを渡すので、絶対パスにしないと `--version` の側は
    # 一度も走らないまま「中の文字列」だけで通る（それで通ってしまっていた）。
    try:
        at = str(Path(exe).resolve())
        out = subprocess.run([at, "--version"], capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.SubprocessError):
        return None
    if out.returncode != 0:
        return None
    line = (out.stdout or out.stderr).strip().splitlines()
    return line[0].strip() if line else None


def judge(line: str, ver: str, sha: str) -> list[str]:
    bad: list[str] = []
    m = STAMP.match(line)
    if not m:
        return [f"形が `cian <版> (<SHA>)` ではありません: {line!r}"]
    got_ver, got_sha = m.group(1), m.group(2)
    if got_ver != ver:
        bad.append(f"版が {got_ver}、`Cargo.toml` は {ver}")
    if got_sha.endswith("-dirty"):
        bad.append(
            f"`-dirty` と名乗っています（{got_sha}）── ビルドの前に"
            "作業ツリーへ書いたものがあります。CI なら `$RUNNER_TEMP` へ逃がす"
        )
    elif got_sha == "unknown":
        bad.append("SHA が `unknown` ── git の無いところで建てた資材です")
    elif got_sha != sha:
        bad.append(f"SHA が {got_sha}、いまの HEAD は {sha}")
    return bad


def main() -> int:
    exes = [a for a in sys.argv[1:] if not a.startswith("-")]
    print("=" * 72)
    print("  資材は、自分がどのビルドかを正しく名乗るか")
    print("=" * 72)
    if not exes:
        print("  実行ファイルを1つ以上渡してください")
        print("=" * 72)
        return 1

    ver, sha = want_version(), want_sha()
    print(f"  名乗ってほしいもの: cian {ver} ({sha})\n")
    bad: list[str] = []
    for exe in exes:
        if not Path(exe).exists():
            bad.append(f"{exe}: ありません")
            print(f"  ✗ {exe}  ありません")
            continue
        stamps = baked(exe)
        line = said(exe)
        if line and line not in stamps:
            # 焼かれた文字列と、走らせて聞いた答えが違うことはない ── 違ったら
            # どちらかの読み方を間違えているので、黙って通さない。
            stamps.append(line)
        if not stamps:
            bad.append(f"{exe}: `cian <版> (<SHA>)` がどこにもありません")
            print(f"  ✗ {exe}  版を名乗っていません")
            continue
        for stamp in stamps:
            here = judge(stamp, ver, sha)
            how = "--version" if stamp == line else "中の文字列"
            print(f"  {'✓' if not here else '✗'} {exe}  {stamp}  ({how})")
            bad += [f"{exe}: {b}" for b in here]

    if bad:
        print(f"\n  {len(bad)} 件あります。\n")
        for b in bad:
            print(f"  ✗ {b}")
        print("\n  どのビルドか答えられない資材は、「直っていない」に答えられません。")
        print("=" * 72)
        return 1
    print(f"\n  {len(exes)} 本すべて、素の HEAD を名乗っています")
    print("=" * 72)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
