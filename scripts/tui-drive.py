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
import unicodedata
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
    # カーソル位置（DSR-CPR）。**本物の端末は必ず答える。**
    # 答えないままにしていたら、`:redraw` が `Terminal::clear()` →
    # `crossterm::cursor::position()` で 2 秒待って諦め、その `Err` が
    # `run_loop` の `?` を通って **cian が落ちた**（2026-09-11）。
    # ここで答えるようにしたのは「落ちないこと」を測るためではなく、
    # **本物の端末と同じ条件にする**ため ── 落ちないほうは cian 側で直した。
    (ESC + b"[6n", ESC + b"[1;1R"),
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
    "S-Left": "\x1b[1;2D", "S-Right": "\x1b[1;2C",
    "C-c": "\x03",
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

    def resize(self, cols, rows, wait=0.6):
        """窓の大きさを変える。**動いている最中に。**

        起動時の幅を変えて見るのは②がやっている。こちらは**使っている途中**の
        変形で、ポップアップも一覧も開いたまま桁が組み替わる ── 全角の片割れが
        いちばん出やすいのがここ。`pyte` 側の画面も一緒に作り直さないと、
        以降ずっと古い桁で読むことになる。
        """
        self.cols, self.rows = cols, rows
        self.screen.resize(rows, cols)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        os.kill(self.pid, __import__("signal").SIGWINCH)
        self.pump(wait)

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

    def reversed_cells(self) -> int:
        """反転で塗られているセルの数 ── シェルの選択はそこにしか出ない。

        文字は変わらないので `text()` では見えない。`pyte` は反転を
        `char.reverse` で持つ。
        """
        n = 0
        for y in range(self.rows):
            row = self.screen.buffer[y]
            for x in range(self.cols):
                if row[x].reverse:
                    n += 1
        return n

    def alive_or_dead(self) -> bool:
        """まだ動いているか。**死んだことを、次の手で知る前に知る。**"""
        self.pump(0.1)
        return not self.dead

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


def check_shell_selection(d, bad):
    """⑤ シェルの範囲選択 ── **反転したセルの数でしか見えない。**

    Shift+←/→ は画面の文字を一字も変えないので、`text()` を比べる物差し
    （④ の「動かなかったキー」）では永久に「動かなかった」と出る。選択は色で
    しか言われていないので、色を読む。

    そして **Esc を2回** 見る。1回目は選択を落として**シェルに留まり**、2回目
    でファイルへ戻る ── 1回で両方やると、選択をやめるのに必ずどこかへ行く
    ことになる。
    """
    t = start(d)
    t.pump(3.0)
    t.send("Esc", 0.5)
    t.send("J", 1.2)          # シェルへ
    t.send("Enter", 0.8)      # プロンプトを1行進めて、上に文字を置く
    base = t.reversed_cells()

    t.send("S-Left", 0.6)
    one = t.reversed_cells()
    t.send("S-Left", 0.6)
    t.send("S-Left", 0.6)
    three = t.reversed_cells()

    print(f"⑤ シェルの選択  : Shift+← ×1 → {one - base} セル / ×3 → {three - base} セル"
          + ("  ✓" if one > base and three > one else "  ✗"))
    if not (one > base and three > one):
        bad.append("シェルで Shift+← が範囲選択にならない（反転が増えない）")

    t.send("Esc", 0.6)
    after_esc = t.reversed_cells()
    still_shell = "シェル" in t.status() or "ファイルへ" in t.status() or "解除" in t.status()
    ok_esc = after_esc <= base
    print(f"⑤ Esc で解除    : 反転 {three - base} → {after_esc - base} セル"
          + ("  ✓" if ok_esc else "  ✗"))
    if not ok_esc:
        bad.append("シェルの選択が Esc で消えない")
    t.close()


def cells_wide(line: str) -> int:
    """その行が占める**桁**。全角は2つぶん。"""
    return sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in line)


def half_cells(t) -> list:
    """**全角の片割れが残っているところ。** `(y, x, なに)` の並び。

    最初は「行の桁数が端末幅ちょうどか」で見ていた。枠の中では効くが、
    **枠の無い行は端末幅まで埋まらない** ── 下のキー案内も状態行もシェルの
    中身も短くて当たり前で、それを 14 件の「桁くずれ」として並べた。
    当てにならない指摘が並ぶ検査は、読まれなくなる検査。

    見るのは行の長さではなく、**セルの対**そのもの。`pyte` は全角文字を
    「1つ目に字、2つ目に空の `data`」で持つので、崩れは二通りしかない:

      ① 相方が潰れた ── 全角の次のセルに、別の字が書かれている
      ② 片割れが残った ── 空の `data` の前が、全角ではない

    どちらも「字は全部読めるのに行がずれる」形で、目でも `cargo test` でも
    通る。**桁のある場所でしか出ない。**
    """
    out = []
    for y in range(t.rows):
        row = t.screen.buffer[y]
        cells = [row[x].data for x in range(t.cols)]
        for x, data in enumerate(cells):
            if data and unicodedata.east_asian_width(data[0]) in "WF":
                if x + 1 < t.cols and cells[x + 1] != "":
                    out.append((y, x, f"{data!r} の相方が {cells[x + 1]!r} に潰れている"))
            elif data == "" and (x == 0 or not (
                cells[x - 1] and unicodedata.east_asian_width(cells[x - 1][0]) in "WF"
            )):
                out.append((y, x, "片割れだけが残っている"))
    return out


def check_columns(d, bad):
    """⑥ どの行も端末の桁数ちょうどか ── **全角の片割れが残っていないか。**

    2026-09-10 に、`T`（トグル）を開くと画面のうち1行だけ 119 桁になっていた。
    ポップアップの右端が、後ろのシェル案内文の全角文字のまん中に刺さり、
    **相方だけが端末に残っていた**。以降その行は右へ1桁ずつずれて、右端の枠が
    1つ手前で終わる ── この家で「右上の角が消える」として何度か出た形の正体。

    見つけにくいのは、**欠けているのが文字ではなく桁**だから。字は全部読める
    ので、目でも `cargo test` でも通る。数えるのは桁だけでいい。

    直したのは `render.rs` の `clear_popup`。ratatui は全角の2桁目を
    `Cell::EMPTY`（記号は `" "`）で持つので、前の画面の片割れと**中身が
    等しく**、差分に載らない ── 隣の桁を `CellDiffOption::AlwaysUpdate` に
    して、空白を送り直させる。
    """
    t = start(d)
    t.pump(3.0)
    t.send("Esc", 0.5)
    worst = []
    for keys, what in [([], "起動直後"), (["T"], "トグル"), (["Esc", "?"], "ヘルプ"),
                       (["Esc", ","], "ソート"), (["Esc", "F3"], "ビューア")]:
        for k in keys:
            t.send(k, 0.7)
        rows = half_cells(t)
        mark = "✓" if not rows else "✗"
        print(f"⑥ 桁がそろう    : {what:<8} {mark}"
              + (f"  崩れ {rows[:4]}" if rows else ""))
        if rows:
            worst.append(f"{what} で全角の片割れが {len(rows)} 個: {rows[:4]}")
    for w in worst:
        bad.append(w)
    t.close()


def notice_body(t):
    """お知らせ（`Popup::Notice`）の**本文の行**。出ていなければ None。

    枠の上端（題に「お知らせ」/「Notice」）から、ボタンの行までのあいだ。
    枠の縦線を落として中身だけ返す ── 画面ぜんぶを対象にすると、後ろの
    ペインの見出しが答えを混ぜる。
    """
    lines = t.text().splitlines()
    top = next((i for i, l in enumerate(lines) if "お知らせ" in l or " Notice " in l), None)
    if top is None:
        return None
    out = []
    for l in lines[top + 1:]:
        if "[ コピー ]" in l or "[ Copy ]" in l:
            break
        inner = l.strip().strip("│").strip()
        if inner:
            out.append(inner)
    return out


def check_resize(d, bad):
    """⑧ **使っている途中で窓の大きさを変える。**

    ②は起動時の幅を3通り見る。こちらは**開いたまま**組み替える ── 一覧も
    ポップアップも桁を割り直すので、全角の片割れがいちばん出やすいのがここ。
    cian をサーバ相手に使うということは、**窓を並べ替えながら使う**という
    ことでもある。

    大きさは端の値を混ぜる ── 41 桁（ペイン2枚には足りない）から 200 桁まで。
    """
    sizes = [(120, 36), (72, 20), (160, 48), (41, 12), (100, 30), (200, 60), (120, 36)]
    for keys, what in [([], "素の一覧"), (["T"], "トグル"), (["Esc", "?"], "ヘルプ")]:
        t = start(d)
        t.pump(3.0)
        t.send("Esc", 0.4)
        for k in keys:
            t.send(k, 0.7)
        trouble = []
        for c, r in sizes:
            t.resize(c, r, 0.6)
            if t.dead:
                trouble.append(f"{c}×{r} で落ちた")
                break
            cells = half_cells(t)
            if cells:
                trouble.append(f"{c}×{r} で片割れ {len(cells)} 個 {cells[:2]}")
        print(f"⑧ 大きさ変更    : {what:<8} "
              + ("  ".join(trouble) if trouble else f"✓（{len(sizes)} 通り）"))
        for x in trouble:
            bad.append(f"{what} を開いたまま {x}")
        t.close()


def check_denied(d, bad):
    """⑦ 断られたとき、**理由が画面に出るか。**

    2026-09-10 の依頼:「管理者権限でないと操作できない挙動をした際にエラーに
    気づきにくい」。書き込めないディレクトリで新規作成とリネームを試して、
    画面に出るものを読む。

    見るのは2つ:

      ① **何かが出るか** ── 黙って何も起きないのが一番わるい
      ② **理由が出るか** ── ここで一度落ちた。`anyhow` の `Display` は
         いちばん外の文脈しか刷らないので、`rename A -> B` とだけ出て
         `Permission denied` は鎖の中に残っていた。**二つのパスと、失敗した
         という語が一つも無い知らせ**は、成功の報告に読める

    `chmod 0o555` は Unix の話なので、Windows では別の形（ACL）になる。
    ここで見ているのは**知らせ方**で、権限の仕組みではない。
    """
    # **砂場の名前に `denied` と書かない。** 最初そう名づけて、その名が
    # ペインの見出しに出るので、`t.text()` に "denied" が常にあった ──
    # 知らせから理由を落として走らせても ✓ のままだった。**検査が黙る**の
    # いつもの形で、変異テストがそれを捕まえた。
    tmp = tempfile.mkdtemp(prefix="cian-tui-readonly-")
    os.makedirs(os.path.join(tmp, "from"))
    os.makedirs(os.path.join(tmp, "to"))
    os.makedirs(os.path.join(tmp, "config"))
    open(os.path.join(tmp, "from", "a.txt"), "w").write("hello\n")
    os.chmod(os.path.join(tmp, "from"), 0o555)
    t = Tui([f"{tmp}/from", f"{tmp}/to"], env={"CIAN_CONFIG_DIR": f"{tmp}/config"})
    try:
        t.pump(3.0)
        t.send("Esc", 0.5)
        for keys, what in [(["a", "n", "e", "w", "Enter"], "新規ファイル"),
                           (["Esc", "r", "x", "Enter"], "リネーム")]:
            for k in keys:
                t.send(k, 0.45)
            # **知らせの中だけを読む。** 画面ぜんぶを見ると、後ろのペインに
            # 出ているパスや見出しが答えを混ぜる（上の註）。
            box = notice_body(t)
            said = box is not None
            why = bool(box) and any(
                w in "\n".join(box) for w in ("denied", "Permission", "権限", "許可")
            )
            print(f"⑦ 断られた時    : {what:<12} 知らせが出る {'✓' if said else '✗'}"
                  f"   理由が読める {'✓' if why else '✗'}")
            if not said:
                bad.append(f"{what}が断られても、画面に何も出ない")
            elif not why:
                bad.append(f"{what}が断られた知らせに、理由（Permission denied）が無い")
            t.send("Esc", 0.5)
    finally:
        os.chmod(os.path.join(tmp, "from"), 0o755)
        t.close()
        shutil.rmtree(tmp, ignore_errors=True)


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
        check_shell_selection(d, bad)
        check_columns(d, bad)
        check_resize(d, bad)
        check_denied(d, bad)
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
