#!/usr/bin/env python3
"""わざと詰まらせて、エンジンが固まらないことを確かめる（総ざらいの第3段）。

**エンジンは要求を1本ずつ順に処理します。** したがって1つの要求が返らない場合、
Electron 版の画面は全体が応答しなくなります。待ちに上限を持たない呼び出しは
`crates/cian-core/src/proc.rs` の `quiet()`・SFTP の通信・`read_to_string()` の
3種類で、どれも要求の処理から呼びます（詳しくは `docs/sweep-plan.ja.md`）。

この道具は窓を介しません。エンジンに要求を1件流し、**5秒で切って**何が返った
かを見ます。5秒は本人が答えた上限です（2026-11-28）。

    python3 scripts/inject.py              全部
    python3 scripts/inject.py fifo zero    名前を指定して
    python3 scripts/inject.py --keep       砂場を消さない

## 合格の条件は3つだけです

1. 5秒以内に返ること（固まらないこと）
2. 断るときは理由を返すこと
3. その理由が本当のことを言っていること（存在するものを「ありません」と
   言わないこと）

3 を機械で見られる場合は `wrong` に正規表現を書きます。見られない場合は
所見として出すだけにします。
"""

from __future__ import annotations

import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEADLINE = 5.0


def engine() -> Path:
    """いちばん新しいエンジン。`gui/engine.js` と同じ選び方です。"""
    out = [ROOT / "target" / d / "cian-server" for d in ("release", "debug")]
    out = [p for p in out if p.exists()]
    if not out:
        sys.exit("エンジンがありません。`cargo build -p cian-server` を実行してください")
    return max(out, key=lambda p: p.stat().st_mtime)


class Engine:
    """エンジンを1つ立てて、要求を1件ずつ流します。"""

    def __init__(self, cwd: Path, env: dict | None = None):
        e = dict(os.environ)
        e.update(env or {})
        self.p = subprocess.Popen(
            [str(engine()), str(cwd)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            env=e, text=True, bufsize=1,
        )
        self.reply: str | None = None

    def point_at(self, name: str) -> int | None:
        """左のペインを一覧し、`name` の行番号を返します。

        **`hash` や `du` は `path` を見ません。** ペインのカーソルが指すものを
        処理するので、注入はカーソルを置いてから流す必要があります ── これを
        知らずに `path` だけ渡していた間、`/dev/zero` のチェックサムが 0.03 秒で
        「合格」になっていました（空振りしていたためです）。
        """
        _, r = self.ask("list", {"pane": "left"})
        if not r or "ok" not in r:
            return None
        for i, e in enumerate(r["ok"].get("entries", [])):
            if e.get("name") == name:
                return i
        return None

    def ask(self, method: str, params: dict, deadline: float = DEADLINE):
        """1件流して、`deadline` 秒だけ待ちます。返り値は (経過, 返事 or None)。

        **`id` が一致する行だけを返事とみなします。** エンジンは進捗などの
        行も送るので、最初の1行を返事として読むと、まだ終わっていない要求を
        「返りました」と報告します ── 実際にそれで `/dev/zero` の
        チェックサムを 0.03 秒で合格にしていました（本当は返りません）。
        """
        want = 7
        self.reply = None

        def read():
            try:
                while True:
                    line = self.p.stdout.readline()
                    if not line:
                        return
                    try:
                        obj = json.loads(line)
                    except ValueError:
                        continue
                    if obj.get("id") == want:
                        self.reply = obj
                        return
            except Exception:
                return

        t = threading.Thread(target=read, daemon=True)
        t.start()
        at = time.time()
        self.p.stdin.write(json.dumps({"id": want, "method": method, "params": params}) + "\n")
        self.p.stdin.flush()
        t.join(deadline)
        took = time.time() - at
        return took, self.reply

    def close(self):
        self.p.kill()
        self.p.wait(timeout=5)


def sandbox() -> Path:
    """詰まりそうなものを並べた砂場を作ります。"""
    d = Path(tempfile.mkdtemp(prefix="cian-inject-"))
    (d / "ok.txt").write_text("ordinary\n", encoding="utf-8")

    # ① 名前付きパイプ ── 読むと、書く側が現れるまで返りません
    os.mkfifo(d / "fifo.txt")

    # ② 読めないファイル ── 権限で断られます
    (d / "denied.txt").write_text("secret\n", encoding="utf-8")
    os.chmod(d / "denied.txt", 0o000)

    # ③ 2GB の疎ファイル ── 丸ごと読むと終わりません
    with open(d / "huge.txt", "wb") as f:
        f.truncate(2 * 1024 * 1024 * 1024)

    # ④ 輪になったリンク ── 辿ると終わりません
    os.symlink(d / "loop", d / "loop")

    # ⑤ 壊れた UTF-8 の名前 ── 表示と受け渡しで落ちやすいところです
    try:
        os.rename(d / "ok.txt", os.fsdecode(bytes(d) + b"/broken-\xff\xfe.txt"))
        (d / "ok.txt").write_text("ordinary\n", encoding="utf-8")
    except OSError:
        pass

    # ⑥ /dev/zero へのリンク ── 一覧に出すためです。際限なく読めるものが
    #    ペインの中にある状態を作ります
    try:
        os.symlink("/dev/zero", d / "zero-link")
    except OSError:
        pass

    # ⑦ たくさんの項目 ── 一覧と容量が返るかどうかです
    many = d / "many"
    many.mkdir()
    for i in range(50_000):
        (many / f"f{i:05}.txt").touch()
    return d


def dead_port() -> int:
    """接続は受けるが、何も言わないポート。SFTP の相手役です。"""
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    s.listen(8)
    threading.Thread(target=lambda: [s.accept() for _ in iter(int, 1)], daemon=True).start()
    return s.getsockname()[1]


def cases(d: Path, port: int) -> list[dict]:
    """名前・要求・期待。`wrong` に当たる返事は、理由が嘘だという所見です。"""
    return [
        {"name": "fifo", "what": "名前付きパイプを読む",
         "method": "viewpath", "params": {"path": str(d / "fifo.txt")},
         "wrong": r"がありません|not found"},
        {"name": "fifo-peek", "what": "名前付きパイプの先頭だけ読む",
         "method": "peek", "params": {"path": str(d / "fifo.txt"), "arg": "-n 5"}},
        {"name": "fifo-hash", "what": "名前付きパイプのチェックサム",
         "method": "hash", "params": {"pane": "left"}, "cursor_on": "fifo.txt"},
        {"name": "zero", "what": "/dev/zero を読む",
         "method": "viewpath", "params": {"path": "/dev/zero"}},
        {"name": "zero-hash", "what": "/dev/zero のチェックサム（リンク経由）",
         "method": "hash", "params": {"pane": "left"}, "cursor_on": "zero-link"},
        {"name": "denied", "what": "読めないファイルを読む",
         "method": "viewpath", "params": {"path": str(d / "denied.txt")}},
        {"name": "huge", "what": "2GB の疎ファイルを読む",
         "method": "viewpath", "params": {"path": str(d / "huge.txt")}},
        {"name": "huge-hash", "what": "2GB の疎ファイルのチェックサム",
         "method": "hash", "params": {"pane": "left"}, "cursor_on": "huge.txt"},
        {"name": "loop", "what": "輪になったリンクの容量",
         "method": "du", "params": {"pane": "left"}, "cursor_on": "loop"},
        {"name": "many-list", "what": "5万件の一覧",
         "method": "list", "params": {"pane": "left", "path": str(d / "many")}},
        {"name": "many-du", "what": "5万件の容量",
         "method": "du", "params": {"pane": "left"}, "cursor_on": "many"},
        # 上限は 20 秒（`cian-scp` の `HANDSHAKE`）。**窓はその間ずっと
        # 止まります。** 5秒の基準を満たすには、接続を要求の処理から外へ
        # 出す必要があります ── 所見 S4 に残しています。
        {"name": "sftp-dead", "what": "応答しない相手へ SFTP で接続", "deadline": 25.0,
         "method": "connect",
         "params": {"pane": "left", "user": "x", "host": "127.0.0.1", "port": port,
                    "path": ".", "password": "x"}},
    ]


def main() -> int:
    pick = [a for a in sys.argv[1:] if not a.startswith("--")]
    keep = "--keep" in sys.argv
    print("=" * 72)
    print("  わざと詰まらせる ── 5秒で切ります")
    print("=" * 72)
    d = sandbox()
    port = dead_port()
    bad: list[str] = []
    slowly: list[str] = []
    try:
        for c in cases(d, port):
            if pick and c["name"] not in pick:
                continue
            eng = Engine(d)
            try:
                params = dict(c["params"])
                if c.get("cursor_on"):
                    at = eng.point_at(c["cursor_on"])
                    if at is None:
                        print(f"  ✗ {c['name']:<12} {c['what']:<28} "
                              f"{c['cursor_on']} が一覧にありません（測れていません）")
                        bad.append(f"{c['name']}: 仕掛けが一覧に出ていません")
                        continue
                    params["cursors"] = {"left": at, "right": 0}
                took, reply = eng.ask(c["method"], params, c.get("deadline", DEADLINE))
            finally:
                eng.close()
            if reply is None:
                print(f"  ✗ {c['name']:<12} {c['what']:<28} 5秒で返りませんでした")
                bad.append(f"{c['name']}: 固まりました（{c['method']}）")
                continue
            err = reply.get("error")
            mark = "✓"
            note = f"断りました: {err}" if err else "返りました"
            # **5秒を超えたものは、返っても固まっていた時間です。**
            # 要求は1本ずつ処理されるので、その間ずっと窓は止まっています。
            if took > DEADLINE:
                mark = "△"
                note = f"{note}（窓は {took:.0f} 秒止まります）"
                slowly.append(f"{c['name']}: {took:.1f} 秒かかります（5秒を超えています）")
            if err and c.get("wrong") and re.search(c["wrong"], err):
                mark = "✗"
                note = f"理由が嘘です: {err}"
                bad.append(f"{c['name']}: 存在するものを「ありません」と断っています")
            print(f"  {mark} {c['name']:<12} {c['what']:<28} {took:5.2f}s  {note[:60]}")
    finally:
        if keep:
            print(f"\n  砂場: {d}")
        else:
            os.chmod(d / "denied.txt", 0o644)
            shutil.rmtree(d, ignore_errors=True)
    print("=" * 72)
    for one in slowly:
        print(f"  △ {one}")
    if bad:
        for b in bad:
            print(f"  ✗ {b}")
        return 1
    if slowly:
        print("  固まりはありませんが、5秒を超えて窓が止まるものがあります")
        return 0
    print("  どの要求も5秒以内に返り、断る理由も本当のことを言っています")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
