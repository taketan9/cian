#!/usr/bin/env python3
"""端末版を、本物の pty で動かす。

窓版には `gui/drive.js` があって「例外 0 件」を基準にできるのに、端末版には
何も無かった。理由は「端末版は手元で動かせない」と思い込んでいたからで、
**それは嘘だった。**

`script(1)` に流すと起動直後で止まる。cian が端末へ問い合わせるからだ:

    ESC[c      DA1              どんな端末か
    ESC[5n     DSR              生きているか
    ESC[16t                     1セルは何画素か
    ESC[14t                     窓は何画素か
    ESC_G…ESC\\  kitty graphics   画像を出せるか
    ESC[?u     kitty keyboard   拡張キーを送れるか

素の pty は誰も答えないので、cian は答えを待つ。**答えれば動く。**
`portable-pty` を `=0.8.1` に留めてある理由（ConPTY が `ESC[6n` の答えを
待って固まった）とまったく同じ形で、端末が答えない問い合わせは「止まった」
としか見えない。

    python3 scripts/tui-drive.py              一巡して報告する
    python3 scripts/tui-drive.py --screen     最後の画面も出す
    python3 scripts/tui-drive.py -- j j Space 好きな手を打つ

**この道具にしか答えられないこと**を見る ── 実際のバイト列、実際の桁、実際の
マウス。中身の判断は `cargo test` の 690 本が見ているので、ここで重ねない。

  ① 起動する（問い合わせに答えて、待ちに入らない）
  ② 枠が閉じている ── 四隅を、いくつかの端末幅で
  ③ 叩いた場所が当たる ── 日本語のタブ名で当たり判定がずれないか
  ④ 動かなかったキー ── `drive.js` と同じ物差し

## pyte が要る

画面を組み立てるのに VT のエミュレータが要る。**入っていなければ、その旨を
言って終わる** ── 黙って飛ばすと、走っていないものを「通った」と読む。

    python3 -m venv /tmp/cian-tui-env && /tmp/cian-tui-env/bin/pip install pyte
    /tmp/cian-tui-env/bin/python scripts/tui-drive.py

（Homebrew の python は PEP 668 で `pip install` を拒むので venv が要る。）
"""

from __future__ import annotations

import fcntl
import os
import pty
import select
import shutil
import struct
import sys
import tempfile
import termios
import time

try:
    import pyte
except ImportError:
    sys.stderr.write(
        "pyte が要ります（画面を組み立てる VT エミュレータ）:\n"
        "  python3 -m venv /tmp/cian-tui-env\n"
        "  /tmp/cian-tui-env/bin/pip install pyte\n"
        "  /tmp/cian-tui-env/bin/python scripts/tui-drive.py\n"
    )
    raise SystemExit(2)

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ESC = b"\x1b"

# 端末が答えるはずのもの。**答えないと起動直後で止まる。**
ANSWERS = [
    (ESC + b"[c", ESC + b"[?62;1;2;6;9;15;22c"),
    (ESC + b"[5n", ESC + b"[0n"),
    (ESC + b"[16t", ESC + b"[6;16;8t"),
    (ESC + b"[14t", ESC + b"[4;576;960t"),
    (ESC + b"[?u", ESC + b"[?0u"),
    (ESC + b"_G", ESC + b"_Gi=31;OK" + ESC + b"\\"),
]

# 生のバイト列。crossterm がこれを読む。
KEYS = {
    "Esc": "\x1b", "Enter": "\r", "Tab": "\t", "Space": " ", "Backspace": "\x7f",
    "Up": "\x1b[A", "Down": "\x1b[B", "Right": "\x1b[C", "Left": "\x1b[D",
    "F1": "\x1b[11~", "F2": "\x1b[12~", "F3": "\x1b[13~", "F9": "\x1b[20~",
    "F10": "\x1b[21~",
    # 修飾つきの矢印。1 + Shift(1) + Alt(2) + Ctrl(4)。
    "C-S-Left": "\x1b[1;6D", "C-S-Right": "\x1b[1;6C",
    "C-S-Up": "\x1b[1;6A", "C-S-Down": "\x1b[1;6B",
}

CORNERS = {"╭", "╮", "╰", "╯", "┌", "┐", "└", "┘", "╔", "╗", "╚", "╝"}


def binary() -> str:
    """新しいほうの cian-tui。release を優先しない。

    `gui/engine.js` が同じ判断をしている理由もそこ ── 朝の release ビルドが
    残っているあいだに午後の `cargo build` は debug へ入り、古いほうと話し
    続けることになる。
    """
    found = []
    for profile in ("debug", "release"):
        at = os.path.join(ROOT, "target", profile, "cian-tui")
        if os.path.exists(at):
            found.append(at)
    if not found:
        raise SystemExit("cian-tui がありません: cargo build --bin cian-tui")
    return max(found, key=lambda p: os.stat(p).st_mtime)


class Tui:
    def __init__(self, args, cols=120, rows=36, env=None):
        self.cols, self.rows = cols, rows
        self.screen = pyte.Screen(cols, rows)
        self.stream = pyte.ByteStream(self.screen)
        e = dict(os.environ)
        e.update({"TERM": "xterm-256color", "COLORTERM": "truecolor", "LANG": "en_US.UTF-8"})
        e.update(env or {})
        exe = binary()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ.clear()
            os.environ.update(e)
            os.execv(exe, [exe] + args)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.dead = False

    def pump(self, secs=0.6):
        end = time.time() + secs
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if not r:
                continue
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                self.dead = True
                break
            if not data:
                self.dead = True
                break
            self.stream.feed(data)
            out = b"".join(reply for probe, reply in ANSWERS if probe in data)
            if out:
                os.write(self.fd, out)
        return self

    def send(self, key, wait=0.4):
        os.write(self.fd, KEYS.get(key, key).encode())
        return self.pump(wait)

    def click(self, col, row, wait=0.6):
        """SGR のマウス（`ESC[?1006h` を cian 自身が有効にしている）。1 始まり。"""
        os.write(self.fd, f"\x1b[<0;{col};{row}M".encode())
        self.pump(0.15)
        os.write(self.fd, f"\x1b[<0;{col};{row}m".encode())
        return self.pump(wait)

    def line(self, y) -> str:
        """1行を桁のとおりに。

        **`or ' '` を書かないこと。** 全角の2セル目は `data == ''` で、そこに
        空白を足すと「マ ー ク」になる。触っていないセルは既に `' '` なので、
        素直に繋ぐのが桁と一致する。（`pyte` の `Screen.display` はこの
        セルで例外を投げるので使えない。）
        """
        row = self.screen.buffer[y]
        return "".join(row[x].data for x in range(self.cols)).rstrip()

    def text(self) -> str:
        return "\n".join(self.line(y) for y in range(self.rows))

    def status(self) -> str:
        for y in range(self.rows - 1, -1, -1):
            s = self.line(y).strip()
            if s:
                return s
        return ""

    def highlighted(self, y) -> str:
        """その行で背景が塗られている桁の中身 ── 選ばれているタブはそこ。

        字だけ読んでも分からない。選択は色でしか言われていない。
        """
        row = self.screen.buffer[y]
        cols = [x for x in range(self.cols) if row[x].bg != "default"]
        if not cols:
            return ""
        return "".join(row[x].data for x in cols).strip()

    def close(self):
        for fn in (lambda: os.close(self.fd),
                   lambda: os.kill(self.pid, 9),
                   lambda: os.waitpid(self.pid, 0)):
            try:
                fn()
            except Exception:
                pass


def sandbox():
    d = tempfile.mkdtemp(prefix="cian-tui-drive-")
    for sub in ("from", "to", "config", "from/深い階層"):
        os.makedirs(os.path.join(d, sub), exist_ok=True)
    open(os.path.join(d, "from/a.txt"), "w").write("one two three\nsecond line\n")
    open(os.path.join(d, "from/b.md"), "w").write("# 見出し\n\n- [ ] 牛乳\n- [x] 珈琲\n")
    open(os.path.join(d, "from/長い名前のファイル.txt"), "w").write("日本語の中身\n")
    open(os.path.join(d, "from/深い階層/inner.txt"), "w").write("inner\n")
    return d


def start(d, cols=120, rows=36):
    return Tui([f"{d}/from", f"{d}/to"], cols=cols, rows=rows,
               env={"CIAN_CONFIG_DIR": f"{d}/config"})


def check_starts(d, bad):
    """① 起動する。

    **問い合わせに答えないと、ここで止まる。** 止まったことは「画面に何も
    書かれない」として出るので、行が1本も無ければそれが答え。
    """
    t = start(d)
    t.pump(3.0)
    t.send("Esc", 0.6)
    drew = sum(1 for y in range(t.rows) if t.line(y).strip())
    print(f"① 起動          : {drew} 行を描いた（問い合わせに答えて止まらない）")
    if drew < 10:
        bad.append("起動して画面が組み上がらない（問い合わせの答えが足りない？）")
    if t.dead:
        bad.append("起動した端末版がすぐ死んだ")
    t.close()


def check_frame(d, bad):
    """② 枠が閉じている ── 四隅を、いくつかの幅で。

    表示切替が上端の枠に右端揃えで描かれ、ラベル末尾の空白が右上の `╮` を
    消していたことがある（2026-09-06）。Rust 側の
    `a_wide_character_never_eats_the_border` は枠の**行**を飛ばして中身だけ
    見ていたので、そこが盲点だった。**幅を変えて見る**のはここでしかできない。
    """
    for cols in (100, 120, 160):
        t = start(d, cols=cols)
        t.pump(3.0)
        t.send("Esc", 0.6)
        top = t.line(0)
        corners = [c for c in top if c in CORNERS]
        right = t.screen.buffer[0][cols - 1].data
        ok = right in CORNERS
        print(f"② 枠 {cols:>3}桁      : 上端の角 {len(corners)} 個 / 右端 {right!r} {'✓' if ok else '✗'}")
        if not ok:
            bad.append(f"{cols} 桁で右ペインの右上の角が {right!r} になっている")
        t.close()


def check_click(d, bad):
    """③ 叩いた場所が当たる ── 日本語のタブ名で。

    当たり判定を字数で作ると、全角1文字につき1桁ぶん左へずれる。実測では
    「日本語」が 3..10 桁に描かれているのに 8・9・10 桁を叩くと隣のタブへ
    飛んでいた。**描かれている桁を画面から読んで、そこを叩く**ので、
    ずれれば必ず出る。
    """
    t = start(d)
    t.pump(3.0)
    t.send("Esc", 0.6)
    t.send("J", 1.5)                       # シェルへ
    t.send("Esc", 0.5)                     # `:` を打つためファイルへ戻る
    for k in (":", "shellname"):
        t.send(k, 0.4)
    t.send("Enter", 0.9)
    t.send("日本語", 0.5)
    t.send("Enter", 1.2)
    t.send("J", 0.8)
    t.send("F9", 1.6)                      # 2枚目

    y = next((y for y in range(t.rows) if "shell 2" in t.line(y)), None)
    if y is None:
        bad.append("シェルのタブ帯が読めない（2枚目が出ていない）")
        print("③ タブの当たり  : 帯が読めない")
        t.close()
        return
    row = t.line(y)
    left, right = row.find("日本語") + 1, row.find("shell 2") + 1
    hits = []
    for col in (left, left + 2, left + 4):      # 「日本語」の左・中・右端
        t.send("F2", 0.7)                       # いったん2枚目へ
        t.click(col, y + 1)
        hits.append((col, t.highlighted(y)))
    ok = all("日本語" in h for _, h in hits)
    print(f"③ タブの当たり  : 「日本語」は {left}..{left + 5} 桁 / "
          + "  ".join(f"{c}桁→{h or '(無)'}" for c, h in hits)
          + ("  ✓" if ok else "  ✗"))
    if not ok:
        bad.append("日本語のタブの上を叩くと隣のタブへ飛ぶ（当たり判定が字数）")
    t.close()


ROUND = [
    ("j", "下へ"), ("j", "下へ"), ("Space", "マーク"), ("Tab", "右ペイン"),
    ("Tab", "左へ"), ("/", "絞込"), ("c", "打つ"), ("Esc", "解除"),
    (",", "並替"), ("Esc", "閉じる"), ("?", "ヘルプ"), ("Esc", "閉じる"),
    ("T", "トグル"), ("Esc", "閉じる"), ("C-S-Right", "境界を右へ"),
    ("C-S-Left", "戻す"), ("C-S-Down", "境界を下へ"), ("C-S-Up", "戻す"),
    ("J", "シェルへ"), ("C-S-Right", "シェルから境界を右へ"),
    ("C-S-Left", "戻す"), ("Esc", "ファイルへ"), ("Esc", ""),
]


def check_round(d, bad, show_screen):
    """④ 動かなかったキー ── `drive.js` と同じ物差し。

    「押したのに画面が何も変わらない」を数える。**下端の1行ではなく画面全部を
    見る** ── ポップアップを開く手（`,` `?` `T`）も境界を動かす手も、状態行は
    一言も変えないので、それだけ見ていると効いている手が全部「動かなかった」に
    化ける（最初にそう出た）。

    0 にはならない（閉じるものが無い `Esc` は本当に何もしない）ので、
    **数そのものではなく、増えたことを見る。**
    """
    t = start(d)
    t.pump(3.0)
    t.send("Esc", 0.6)
    quiet = []
    for key, what in ROUND:
        before = t.text()
        t.send(key, 0.55)
        if t.text() == before:
            quiet.append(key)
    print(f"④ 一巡          : {len(ROUND)} 手のうち 動かなかったキー {len(quiet)} 件"
          + (f"  {quiet}" if quiet else ""))
    if t.dead:
        bad.append("一巡の途中で端末版が死んだ")
    if show_screen:
        print("\n----- 最後の画面 -----")
        print(t.text())
    t.close()


def main() -> int:
    args = sys.argv[1:]
    show_screen = "--screen" in args
    if "--" in args:
        keys = args[args.index("--") + 1:]
        d = sandbox()
        t = start(d)
        t.pump(3.0)
        for k in keys:
            before = t.status()
            t.send(k, 0.6)
            mark = " " if t.status() != before else "×"
            print(f" {mark} {k:<12} {t.status()[:90]}")
        print("\n" + t.text())
        t.close()
        shutil.rmtree(d, ignore_errors=True)
        return 0

    print("=" * 72)
    print(f"  端末版を pty で動かす ── {os.path.relpath(binary(), ROOT)}")
    print("=" * 72)
    bad: list[str] = []
    d = sandbox()
    try:
        check_starts(d, bad)
        check_frame(d, bad)
        check_click(d, bad)
        check_round(d, bad, show_screen)
    finally:
        shutil.rmtree(d, ignore_errors=True)
    print("=" * 72)
    if bad:
        for b in bad:
            print(f"  ✗ {b}")
        print("=" * 72)
        return 1
    print("  端末版は本物の端末で立って、枠は閉じていて、叩いた場所が当たります")
    print("=" * 72)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
