#!/usr/bin/env python3
"""上限の無い待ちを一覧にします（総ざらいの第2段）。

**エンジンは要求を1本ずつ処理します。** その中に返らない呼び出しが1つでも
あると、窓全体が止まります。第3段（`inject.py`）は実際に詰まらせて確かめ
ますが、**注入できるのは思いついた形だけ**です。このツールは逆から見ます ──
待つところを全部並べ、上限の無いものを数えます。

実際、S12・S13・S14（名前付きパイプの `head`、丸ごと読む `bytes`）は注入では
なく**読んで**見つけました。このツールは、その読み方を機械にしたものです。

    python3 scripts/deadline.py           数える
    python3 scripts/deadline.py --list    場所を全部出す

## 見るのは2つです

1. **子プロセスを待つところに上限があるか。** 上限があるのは
   `cian_core::proc::output_within` を通るものだけです。素の `.output()`
   `.status()` `.wait()` `.wait_with_output()` は、相手が返らなければ
   永久に待ちます
2. **利用者が名前を言ったものを、読む前に種類で断っているか。** 名前付き
   パイプは `File::open` が書く側を待ち、デバイスには読み終わりがありません。
   断りは `cian_core::attrs::special` を通ります

## 数の決まり

- **下限**（`LEAST_*`）── 対象を取りこぼして「所見なし」と報告する故障を
  防ぎます。`keycover.py` が 72 種から2種に落ちたまま百分率を出し続けた型です
- **天井**（`*_CEILING`）── 上限の無い待ちが増えないようにします。下げる
  ときは、その場所に上限を入れてから天井も一緒に下げます

**試験の中は見ません。** `#[cfg(test)]` の中と `tests.rs` は、相手も時間も
こちらが決めているので、止まっても止まるのはその試験だけです。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATES = ROOT / "crates"

# 子プロセスの終わりを待つ呼び出し。`output_within` はこの形を取らないので、
# ここに出てくるものは**すべて上限がありません**。
WAITS = re.compile(r"\.(output|status|wait|wait_with_output)\(\)")

# 読む前に種類を見てほしいところ ── 利用者が名前を言ったものを開く道。
OPENS = re.compile(r"(fs::read_to_string\(|fs::read\(|File::open\()")

# 上限が無くてよいもの。**理由を書きます。** 書けないものは天井の側に残します。
#
# 鍵は「ファイル:関数名」です。行番号にしないのは、1行足すたびに一覧が
# 誤った記述になるためです。
WAIT_OK = {
    # 利用者がその場にいて、終わるまで待つのが仕事のもの。
    "crates/cian-tui/src/lib.rs:suspend_and_edit":
        "外部エディタ。利用者がそれを閉じるまで待つのが正しい動き",
    "crates/cian-core/src/elevate.rs:run_elevated":
        "昇格した実行。UAC の同意を待つので、上限を置くと同意の途中で切れる",
    # ビルドの中。走るのは cargo の中で、窓はまだ無い。
    "crates/cian-tui/build.rs:main": "ビルド時。窓はまだ存在しない",
    "crates/cian-server/build.rs:main": "ビルド時。窓はまだ存在しない",
    # 自分で上限を持っているもの。
    "crates/cian-core/src/proc.rs:output_within": "これが上限そのもの",
    "crates/cian-core/src/proc.rs:shell_filter":
        "`:%!cmd` の実装。利用者が自分で打った1本で、終わるまでが仕事",
}

# 読む前の種類判定が要らないもの。**設定や自分の資材**で、利用者が名前を
# 言ったものではありません。
OPEN_OK_FILES = {
    "crates/cian-lua/src/lib.rs",
    "crates/cian-lua/src/settings_edit.rs",
    "crates/cian-lua/src/settings_ssh.rs",
    "crates/cian-lua/src/macros.rs",
    "crates/cian-lua/src/macro_script.rs",
    "crates/cian-tui/build.rs",
    "crates/cian-server/build.rs",
}

# いまの数。**増やさないための天井**です。
WAIT_CEILING = 23
OPEN_CEILING = 49

# 取りこぼし防止の下限。
LEAST_FILES = 80
LEAST_WAITS = 25
LEAST_OPENS = 40


def alive(text: str) -> list[tuple[int, str]]:
    """試験の中を落として、(行番号, 行) を返します。

    `#[cfg(test)]` の次に来る `mod ... {` から、対応する `}` までを飛ばします。
    波括弧は素朴に数えます。文字列やコメントの中の括弧で狂いますが、狂う
    方向は「多めに飛ばす」ではなく「少なめに飛ばす」ので、**見落としでは
    なく誤検出**の側に倒れます。
    """
    out: list[tuple[int, str]] = []
    skip_until = -1
    depth = 0
    in_test = False
    for n, line in enumerate(text.splitlines(), 1):
        if in_test:
            depth += line.count("{") - line.count("}")
            if depth <= 0:
                in_test = False
            continue
        if "#[cfg(test)]" in line:
            skip_until = n + 3  # 次の数行のうちに mod か fn が来ます
            continue
        if 0 < skip_until and n <= skip_until and ("mod " in line or "fn " in line):
            if "{" in line:
                depth = line.count("{") - line.count("}")
                in_test = depth > 0
            else:
                in_test = True
                depth = 0
            skip_until = -1
            continue
        out.append((n, line))
    return out


def enclosing(lines: list[tuple[int, str]], at: int) -> str:
    """その行を抱えている関数の名前。見つからなければ `?`。"""
    for n, line in reversed(lines):
        if n > at:
            continue
        m = re.search(r"\bfn\s+([a-z_0-9]+)", line)
        if m:
            return m.group(1)
    return "?"


def files() -> list[Path]:
    got = [p for p in CRATES.rglob("*.rs") if p.name != "tests.rs"]
    return sorted(got)


def main() -> int:
    show = "--list" in sys.argv
    waits: list[tuple[str, str, str]] = []   # (鍵, 場所, 行)
    opens: list[tuple[str, str, str]] = []
    allowed_waits = 0
    scanned = 0

    for path in files():
        rel = str(path.relative_to(ROOT))
        text = path.read_text(encoding="utf-8", errors="replace")
        lines = alive(text)
        scanned += 1
        for n, line in lines:
            if WAITS.search(line):
                key = f"{rel}:{enclosing(lines, n)}"
                if key in WAIT_OK:
                    allowed_waits += 1
                    continue
                waits.append((key, f"{rel}:{n}", line.strip()[:70]))
            if OPENS.search(line) and rel not in OPEN_OK_FILES:
                opens.append((rel, f"{rel}:{n}", line.strip()[:70]))

    print("=" * 72)
    print("  上限の無い待ちを数えます。総ざらいの第2段")
    print("=" * 72)
    print(f"  読んだファイル {scanned} 枚（試験の中は見ていません）")
    print()
    print(f"  ① 子プロセスを待つところ     上限なし {len(waits)} 件"
          f"（天井 {WAIT_CEILING}）／理由つきで許したもの {allowed_waits} 件")
    print(f"  ② 読む前に種類を見ないところ 候補 {len(opens)} 件（天井 {OPEN_CEILING}）")
    print()

    if show:
        print("  ① 上限の無い待ち")
        for key, where, line in waits:
            print(f"    {where}")
            print(f"      {line}")
        print()
        print("  ② 読む道")
        for _, where, line in opens:
            print(f"    {where}")
            print(f"      {line}")
        print()

    bad = []
    if scanned < LEAST_FILES:
        bad.append(f"読んだファイルが {scanned} 枚しかありません（{LEAST_FILES} 枚以上のはず）")
    if len(waits) + allowed_waits < LEAST_WAITS:
        bad.append(f"待つところが {len(waits) + allowed_waits} 件しか見つかりません"
                   f"（{LEAST_WAITS} 件以上のはず）。探し方が壊れています")
    if len(opens) < LEAST_OPENS:
        bad.append(f"読む道が {len(opens)} 件しか見つかりません（{LEAST_OPENS} 件以上のはず）")
    if len(waits) > WAIT_CEILING:
        bad.append(f"上限の無い待ちが {len(waits)} 件に増えました（天井 {WAIT_CEILING}）")
    if len(opens) > OPEN_CEILING:
        bad.append(f"読む道が {len(opens)} 件に増えました（天井 {OPEN_CEILING}）")

    print("=" * 72)
    if bad:
        for b in bad:
            print(f"  ✗ {b}")
        print()
        print("  上限を入れてから天井を下げてください。`--list` で場所が出ます")
        return 1
    print("  天井の中です。触った道から少しずつ上限を入れて、天井も一緒に下げること")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
