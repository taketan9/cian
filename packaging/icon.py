#!/usr/bin/env python3
"""案E「かぎ括弧」を .ico の7サイズに焼く。

外部ライブラリを使わずに描いている。理由は cian のビルドと同じで、
**この機械に無いものを要求しない**こと ── rsvg も ImageMagick も PIL も
入っていないし、アイコン1枚のために入れるものでもない。図形は角丸長方形
だけなので、4x4 のスーパーサンプリングで十分に滑らかになる。

日本語の「 」が左右の枠になり、その間に一行。**日本語の環境で日本語を書く人の
ために作られたツールである**ことを、説明せずに言う。並のファイラーアイコンの棚では
確実に浮く。それが選ばれた理由。

サイズごとに描き分ける。16px では**括弧だけ**にする（真ん中の行は消えるので、
残そうとすると滲むだけ）。アイコンは一枚の絵を縮めたものではなく、同じ考えを
各サイズで描き直したものの束。
"""

import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent
SIZES = [16, 24, 32, 48, 64, 128, 256]
SS = 4  # スーパーサンプル倍率

# いまのアイコンから読んだ二色。彼が「色味は好き」と言った当のもの。
G0 = (0x1E, 0x8F, 0x9B)
G1 = (0x16, 0xCB, 0xE1)
INK = (0xEA, 0xFC, 0xFF)      # 行の白
BAR = (0x17, 0x83, 0x8F)      # 光った行の中の「ファイル名」


def rounded(x, y, w, h, r, px, py):
    """(px, py) が角丸長方形の中か。すべて 128 単位系。"""
    if px < x or py < y or px > x + w or py > y + h:
        return False
    r = min(r, w / 2, h / 2)
    cx = min(max(px, x + r), x + w - r)
    cy = min(max(py, y + r), y + h - r)
    dx, dy = px - cx, py - cy
    return dx * dx + dy * dy <= r * r


def shapes_for(size):
    """このサイズで描くもの。前から順に重ねる。

    上下対称に置く。「 の角を (26, 26)、」 の角を (102, 102) に取ると 128 の
    中心 64 に対して鏡になり、どちらの括弧も同じ重さに見える。角丸の隅からは
    離す: 括弧が縁に触ると、括弧ではなく「枠が欠けている」に見える。

    **真ん中の一行は 64px 以上だけ。** 案を出したときに「16px で消える」と
    書いたが、実際に焼いてみると 32 と 48 でも消えるのではなく**」に寄って
    汚す**。部品を減らすほうが小さいアイコンは強い。
    """
    big = size >= 64
    small = size <= 24

    # 太さと腕の長さ。小さいほど太く、短く。
    t, arm = (12, 40) if big else ((15, 38) if not small else (18, 40))
    r = 2 if big else 0
    lo = 26 if not small else 22
    hi = 128 - lo

    out = [
        # 「：上の腕と、その左端から下りる縦
        (lo, lo, arm, t, r, INK, 1.0),
        (lo, lo, t, arm, r, INK, 1.0),
        # 」：右の縦と、その下端から左へ伸びる腕
        (hi - t, hi - arm, t, arm, r, INK, 1.0),
        (hi - arm, hi - t, arm, t, r, INK, 1.0),
    ]
    if big:
        # その間の一行：二つの括弧の中で cian が何をしているか。
        # 括弧から左右に等距離（13）で、括弧より一段静か。最初は 32 幅で、
        # 」の縦に触りそうに見えた。主語は括弧のほうなので、譲る。
        out.append((51, 59, 26, 10, 5, INK, 0.75))
    return out


def render(size):
    """RGBA の bytes を返す。"""
    n = size * SS
    step = 128.0 / n
    layers = shapes_for(size)
    px = bytearray(size * size * 4)

    for iy in range(size):
        for ix in range(size):
            r = g = b = a = 0.0
            for sy in range(SS):
                for sx in range(SS):
                    # サブピクセルの中心を 128 単位系へ
                    u = (ix * SS + sx + 0.5) * step
                    v = (iy * SS + sy + 0.5) * step
                    if not rounded(4, 4, 120, 120, 26, u, v):
                        continue
                    # 地の斜めグラデーション
                    t = max(0.0, min(1.0, (u + v) / 256.0))
                    cr = G0[0] + (G1[0] - G0[0]) * t
                    cg = G0[1] + (G1[1] - G0[1]) * t
                    cb = G0[2] + (G1[2] - G0[2]) * t
                    ca = 1.0
                    for (lx, ly, lw, lh, lr, col, op) in layers:
                        if rounded(lx, ly, lw, lh, lr, u, v):
                            cr = cr + (col[0] - cr) * op
                            cg = cg + (col[1] - cg) * op
                            cb = cb + (col[2] - cb) * op
                    r += cr * ca
                    g += cg * ca
                    b += cb * ca
                    a += ca
            k = SS * SS
            o = (iy * size + ix) * 4
            if a > 0:
                # 非プリマルチプライド: 色はカバーした分だけの平均、alpha は被覆率
                px[o + 0] = int(round(r / a))
                px[o + 1] = int(round(g / a))
                px[o + 2] = int(round(b / a))
                px[o + 3] = int(round(255 * a / k))
            else:
                px[o + 3] = 0
    return bytes(px)


def png(size, rgba):
    raw = bytearray()
    for y in range(size):
        raw.append(0)  # フィルタ None。図形が単純で、圧縮より読みやすさを取る
        raw += rgba[y * size * 4:(y + 1) * size * 4]

    def chunk(tag, data):
        c = struct.pack('>I', len(data)) + tag + data
        return c + struct.pack('>I', zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (b'\x89PNG\r\n\x1a\n'
            + chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(bytes(raw), 9))
            + chunk(b'IEND', b''))


def ico(pngs):
    """PNG を並べた .ico。いまの cian.ico と同じ作り（7枚とも PNG、32bpp）。"""
    n = len(pngs)
    head = struct.pack('<HHH', 0, 1, n)
    off = 6 + 16 * n
    dirs, body = b'', b''
    for size, data in pngs:
        dirs += struct.pack('<BBBBHHII',
                            0 if size == 256 else size,
                            0 if size == 256 else size,
                            0, 0, 1, 32, len(data), off)
        off += len(data)
        body += data
    return head + dirs + body


if __name__ == '__main__':
    made = []
    for s in SIZES:
        data = png(s, render(s))
        (OUT / f'b-{s}.png').write_bytes(data)
        made.append((s, data))
        print(f'  {s:>3}x{s:<3} {len(data):>7} B')
    blob = ico(made)
    (OUT / 'cian.ico').write_bytes(blob)
    print(f'\ncian.ico  {len(blob)} B  ({len(made)} 枚)')
    # macOS の Dock は .ico を読まない。`app.dock.setIcon` に渡せる PNG を
    # 一枚、同じ絵から出しておく。二つの絵にすると必ずずれる。
    (OUT / 'cian.png').write_bytes(dict(made)[256])
    print(f"cian.png  {len(dict(made)[256])} B  (256px、Dock 用)")
