#!/usr/bin/env python3
"""cian.icns ── Dock と Finder が読む形に、各サイズを描き直して束ねる。

`icon.py` を呼ぶだけで、絵は持たない。**アイコンは一枚を縮めたものではなく、
同じ考えを各サイズで描き直したものの束**という向こうの判断が、ここでも効く ──
16px は括弧だけ、という描き分けは `icon.py` が寸法を見て決めている。

Dock の升目に合わせて 80% で描き、残りを透明の余白にする。.ico は角まで
埋める（Windows はそれで正しい）が、Mac の Dock で他と並ぶと一回り大きい。

    packaging/macos/icns.py <出力先.icns>
"""

import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import icon  # noqa: E402

INSET = 80          # 升目に対する絵の割合（%）
SIZES = [16, 32, 128, 256, 512]     # それぞれ 1x と 2x を作る

_drawn = {}         # 辺の長さ → PNG。16@2x と 32 は同じ絵なので一度だけ描く。


def tile(side):
    """一辺 `side` の升目に、透明の余白を付けて描く。"""
    if side in _drawn:
        return _drawn[side]
    art = side * INSET // 100
    rgba = icon.render(art)
    pad = (side - art) // 2
    canvas = bytearray(side * side * 4)
    for y in range(art):
        src = y * art * 4
        dst = ((y + pad) * side + pad) * 4
        canvas[dst:dst + art * 4] = rgba[src:src + art * 4]
    _drawn[side] = icon.png(side, bytes(canvas))
    return _drawn[side]


def main(out):
    work = Path(tempfile.mkdtemp(suffix='.iconset'))   # iconutil は名前で判断する
    for size in SIZES:
        for scale, side in ((1, size), (2, size * 2)):
            name = f'icon_{size}x{size}{"@2x" if scale == 2 else ""}.png'
            (work / name).write_bytes(tile(side))
            print(f'  {name}', flush=True)
    subprocess.run(['iconutil', '-c', 'icns', str(work), '-o', out], check=True)
    for f in work.iterdir():
        f.unlink()
    work.rmdir()
    print(f'{out}  {Path(out).stat().st_size} B')


if __name__ == '__main__':
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
