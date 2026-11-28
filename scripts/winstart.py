#!/usr/bin/env python3
"""配る Windows の exe は、素の Windows で起動するか。

    python3 scripts/winstart.py <exe> [<exe>...]
    python3 scripts/winstart.py --list <exe>     読み取った取り込み先を全部出す

**2026-11-28、仮想環境の素の Windows 11 で `cian-tui-win-x64.exe` が起動しません
でした。** 画面には何も出ず、終了コードだけが `-1073741515`（`0xC0000135` ＝
DLL が見つかりません）でした。原因は `VCRUNTIME140.dll` で、これは Visual C++ の
再頒布可能パッケージに入っているものです。**Windows の素の状態にはありません。**
会社の PC では別のアプリが置いていったものが在るため、気づきませんでした。

この検査は exe を**走らせずに**読みます。Windows でしか走らない物をこちらで
確かめられる、数少ない道です（`scripts/buildstamp.py` と同じ作法です）。

## 合格の条件

1. 取り込み先（import）の DLL を1つ以上読み取れること ── 読み落として
   「所見なし」と言わないためです
2. 再頒布可能パッケージの DLL を取り込んでいないこと
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

# **Windows の runner の標準出力は cp1252。** 日本語を1行出した瞬間に
# `UnicodeEncodeError` で落ち、「資材が悪い」ではなく「検査が壊れた」で
# ジョブが赤くなる。`scripts/buildstamp.py` が 2026-10-31 に発生した同じ原因に、
# **この検査を足した日にもう一度落ちた**（2026-11-28）：同じ直しを別の
# ファイルに書き写す必要があることを、どの検査も監視していなかった。
# いまは `scripts/claudemd.py` が監視する。
for stream in (sys.stdout, sys.stderr):
    try:
        stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, OSError):
        pass

# 素の Windows に無いもの：入れるには再頒布可能パッケージが要ります。
REDIST = ("vcruntime", "msvcp", "concrt", "vcomp", "vcamp", "mfc")

# 読み取れた取り込み先の下限。0 件を「きれい」と読まないためです。
LEAST = 3


def imports(path: Path) -> list[str]:
    """PE の取り込み表にある DLL の名前。読めなければ空。"""
    b = path.read_bytes()
    if b[:2] != b"MZ":
        return []
    pe = struct.unpack_from("<I", b, 0x3C)[0]
    if b[pe:pe + 4] != b"PE\0\0":
        return []
    sections, = struct.unpack_from("<H", b, pe + 6)
    opt_size, = struct.unpack_from("<H", b, pe + 20)
    opt = pe + 24
    magic, = struct.unpack_from("<H", b, opt)
    # 取り込み表は data directory の2番目。PE32+ と PE32 で頭の長さが違います。
    dirs = opt + (112 if magic == 0x20B else 96)
    imp_rva, = struct.unpack_from("<I", b, dirs + 8)
    if not imp_rva:
        return []

    # section をたどって RVA をファイルの位置に直します。
    table = []
    at = opt + opt_size
    for i in range(sections):
        s = at + i * 40
        va, = struct.unpack_from("<I", b, s + 12)
        size, = struct.unpack_from("<I", b, s + 16)
        off, = struct.unpack_from("<I", b, s + 20)
        table.append((va, size, off))

    def here(rva: int) -> int | None:
        for va, size, off in table:
            if va <= rva < va + max(size, 1):
                return off + (rva - va)
        return None

    out, at = [], here(imp_rva)
    if at is None:
        return []
    while True:
        chunk = b[at:at + 20]
        if len(chunk) < 20 or chunk == b"\0" * 20:
            break
        name_rva, = struct.unpack_from("<I", chunk, 12)
        where = here(name_rva)
        if where is not None:
            end = b.index(b"\0", where)
            out.append(b[where:end].decode("ascii", "replace"))
        at += 20
    return out


def main() -> int:
    show = "--list" in sys.argv
    files = [Path(a) for a in sys.argv[1:] if not a.startswith("--")]
    if not files:
        print(__doc__)
        return 1

    print("=" * 72)
    print("  配る exe は、素の Windows で起動するか")
    print("=" * 72)
    bad: list[str] = []
    for f in files:
        if not f.exists():
            print(f"  ✗ {f} がありません")
            bad.append(f"{f.name}: ありません")
            continue
        got = imports(f)
        if show:
            for one in got:
                print(f"  取り込み {f.name}: {one}")
        if len(got) < LEAST:
            print(f"  ✗ {f.name}  取り込み先を {len(got)} 件しか読めませんでした（読み落としです）")
            bad.append(f"{f.name}: 取り込み表を読めていません")
            continue
        need = [d for d in got if d.lower().startswith(REDIST)]
        if need:
            print(f"  ✗ {f.name}  {len(got)} 件中、再頒布が要るもの: {', '.join(need)}")
            bad.append(f"{f.name}: {', '.join(need)} が要ります")
        else:
            print(f"  ✓ {f.name}  取り込み先 {len(got)} 件、どれも Windows に在るものです")
    print("=" * 72)
    if bad:
        for one in bad:
            print(f"  ✗ {one}")
        print("\n  素の Windows では起動しません（終了コード 0xC0000135）。")
        print("  `.cargo/config.toml` の `crt-static` が効いているか確かめてください。")
        return 1
    print("  素の Windows で起動します")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
