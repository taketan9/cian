#!/usr/bin/env python3
"""画面の幅を、字数で測っているところ。

**2026-09-06 に、この形のバグが6つ出た。** 全部「日本語でしか出ない」もので、
英語では字数と桁数が同じなので `cargo test` も `drive.js` も目視も通っていた:

* ポップアップのボタンが `[ コピ` で切れる（7字・10桁）
* シェルのタブを叩くと隣に飛ぶ（当たり判定が字数ぶんしか無い）
* 右ペインの右上の角が消える（ラベル末尾の空白が角に乗る）
* `:blame` の欄が、日本語のコミッタ名の行だけ右にずれる
* トグルのメニューの左端が、☁ の行だけずれる
* 同じパスを二つの前端が違う長さに詰める（`truncate_middle` と `truncateMiddle`）

数えているのは**道具を使っているか**だけ。どちらの前端にも桁を測る道具が
あって、それを通していない算術を挙げる:

    Rust : crate::util::width(s)         （unicode-width）
    JS   : cellWidth(s) / padCells(s, n) （East Asian Width の W と F）

    python3 scripts/widths.py          # 件数
    python3 scripts/widths.py --list   # 一行ずつ

## 何を疑っているか

* **Rust** ── `chars().count()` を `u16` にしているもの。ratatui の座標は
  `u16` なので、そのキャストは「これは画面の桁だ」と言っているのと同じ。
  高さは `Vec::len()` で数えるので、ここには出てこない
* **JS** ── `padEnd` / `padStart` で見た目を揃えているものと、`.length` を
  桁の予算として比べているもの。数字を0詰めする `String(n).padStart(2, '0')`
  は桁の話ではないので除く

**賢くしない。** 「本当に画面に出るか」まで見ようとすると、この検査自身が
信用できない大きさになる ── `audit.py` が一度 `'http://'` を壊した形。
疑わしいものを挙げて、**理由を書いて免除する**ほうを採る。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUST = sorted((ROOT / "crates").glob("*/src/*.rs"))
JS = ROOT / "gui" / "renderer.js"

# 桁の話ではないもの。**行そのもの**を書く ── 「この関数は除く」にすると、
# 中身が変わっても免除が残る。
WAIVED = {
    # 数字の0詰め。文字幅ではなく桁揃えの話で、どちらも半角。
    "const one = (x, y) => Math.round(x + (y - x) * t).toString(16).padStart(2, '0');",
    "const p = (n) => String(n).padStart(2, '0');",
    # 詰めている中身が**作りからして半角**のもの。日本語が入る道が無い。
    'format!("{:<10}: {}", "state", state),',            # 見出しは英字の即値
    'format!("{:<10}: {}", "remembered", remembered),',
    'format!("{:<10}: {}", "query", cfg.query_cmd().unwrap_or_else(|| dash.clone())),',
    'format!("{:<10}: {}", "set", cfg.set_cmd("<id>").unwrap_or_else(|| dash.clone())),',
    'format!("{:<10}: {}", "off", cfg.off.clone().unwrap_or(dash)),',
    'format!("{:<10}: {}", "restore", cfg.restore),',
    'format!("{:<10}: {}", "last", last),',
    'let badge = format!("{:<6} ", c.mode().badge());',  # badge() は "simple" だけ
    'format!("{:<7} {} ", shown_hash, pad_to(&shown_who, 11)),',  # git のハッシュ
    'format!("{:08x}  {:<49}|{}|", index * 16, hex, ascii)',      # 16進ダンプ
}

RUST_SUSPECT = re.compile(
    r"chars\(\)\.count\(\)\s+as\s+u16"
    # **`{:<9}` は9桁ではなく9字。** 2026-09-10 に14か所見つかった ── `:count` の
    # `行`（1字2桁）と `ファイル`（4字8桁）が同じ9字に詰められて、数字の列が
    # 5通りにばらけていた。ヘルプは `drag in from Finder` が上限を超えて説明が
    # 鍵にくっついていた。**英語では字数と桁数が同じ**なので、目でも
    # `cargo test` でも通る。右詰め `{:>N}` は数字に使う形なので見ない。
    r"|\{:[<^]\d+\}"
)
# **`slice` は外した。** 配列にも、送る前のバイト数の打ち切りにも使うので、
# 7 件出て**そのどれも桁の話ではなかった**。当てにならない指摘が7件並ぶ検査は、
# 読まれなくなる検査。
#
# 代わりに、桁の予算を `.length` で切っている形を1つだけ足した ──
# `truncateMiddle` が `text.length <= max` で切っていて、端末版の
# `truncate_middle`（「全角は2つぶん」）と**同じパスを違う長さに詰めていた**。
JS_SUSPECT = re.compile(
    r"\.padEnd\(|\.padStart\(|\.length\s*(?:<=|<|>=|>)\s*(?:max|width|cols)\b"
)


def hits():
    out = []
    for path in RUST:
        if path.name == "tests.rs":
            continue
        for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            t = line.strip()
            if t.startswith("//") or t in WAIVED:
                continue
            if RUST_SUSPECT.search(line):
                out.append((str(path.relative_to(ROOT)), n, t))
    for n, line in enumerate(JS.read_text(encoding="utf-8").splitlines(), 1):
        t = line.strip()
        if t.startswith("//") or t.startswith("///") or t in WAIVED:
            continue
        if JS_SUSPECT.search(line):
            # 数字を作っている行は桁の話ではない。
            if re.search(r"toString\(\d+\)|String\(\w+\)|\bNumber\b", line):
                continue
            out.append(("gui/renderer.js", n, t))
    return out


def main() -> int:
    found = hits()
    listing = "--list" in sys.argv
    print("=" * 72)
    if not found:
        print("  画面の幅は、どこも桁で測っています"
              f"（免除 {len(WAIVED)} 行 ── 数字の0詰めと、半角しか入らない欄）")
        print("=" * 72)
        return 0
    print(f"  字数で測っているかもしれないところ {len(found)} 件")
    print()
    for path, n, line in found if listing else found[:20]:
        print(f"    {path}:{n}")
        print(f"      {line[:88]}")
    print()
    print("  桁で測るなら: Rust は `crate::util::width(s)` / `pad_to(s, n)`、"
          "JS は `cellWidth(s)` / `padCells(s, n)`")
    print("  桁の話でないなら、その行を `scripts/widths.py` の WAIVED に"
          "理由つきで書く")
    print("=" * 72)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
