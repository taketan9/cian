#!/usr/bin/env python3
"""リモート（SFTP）を、**本物のサーバ相手に**通す。

## なぜこれがあるか

リモートペインは 2026-08 に入ったのに、2026-09-02 まで**一度も動かして確かめて
いなかった**。理由はずっと同じ ──「手元に SFTP サーバが無い」。その間に入った
コードは全部「書いたが通していない」で、ロードマップにもそう書いてあった。

無かったのはサーバではなく、**立て方の発想**だった。macOS にも Linux にも
`sshd` と `sftp-server` は最初から入っていて、**管理者権限なしで**自分専用の
ものを高いポートに立てられる。ここでやっているのはそれだけ:

  * 使い捨てのホスト鍵と client 鍵を作る
  * `127.0.0.1:2222` だけを listen する `sshd_config` を書く
  * `/usr/sbin/sshd` を自分の権限で起動する
  * `cian-server` を繋いで、実際にファイルが動いたかを**サーバ側の実体で**見る

これを立てた日に、**素通りしていた不具合が1つ出た** ── ローカルペインから
リモートペインへの `c` が、ローカルにコピーして「1件コピーしました」と言って
いた。リモートペインの `cwd` は接続前のディレクトリのままなので、行き先が
手元の適当な場所になっていた。画面には何も出ない。

## 使い方

    python3 scripts/remote.py           # 立てて、通して、片付ける
    python3 scripts/remote.py --keep    # 終わってもサーバを残す（自分で叩く用）

Windows では動かない（`sftp-server` が無い）。**CI の3-OS には入れていない** ──
Windows で必ず落ちる検査は、落ちても誰も見なくなる。
"""
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def free_port():
    """A port nobody is on.

    **Fixed at 2222 first, and a leftover sshd from the previous run held it.**
    The check saw the port answer, decided the server was up, and then failed
    to authenticate — against *the old server*, with the new run's key. The
    error said "the key was refused", which was true and pointed nowhere near
    the cause.
    """
    import socket
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


PORT = free_port()
SFTP_SERVER = "/usr/libexec/sftp-server"      # macOS
SFTP_SERVER_LINUX = "/usr/lib/openssh/sftp-server"


def sftp_server_path():
    for p in (SFTP_SERVER, SFTP_SERVER_LINUX, "/usr/libexec/openssh/sftp-server"):
        if os.path.exists(p):
            return p
    return None


class Server:
    """自分の権限で立てた sshd。127.0.0.1 だけを聴く。"""

    def __init__(self, home):
        self.home = home
        self.proc = None
        self.key = os.path.join(home, "id")

    def start(self):
        run = lambda *a: subprocess.run(a, check=True, capture_output=True)
        run("ssh-keygen", "-q", "-t", "ed25519", "-f", os.path.join(self.home, "hostkey"), "-N", "", "-C", "host")
        run("ssh-keygen", "-q", "-t", "ed25519", "-f", self.key, "-N", "", "-C", "client")
        shutil.copy(self.key + ".pub", os.path.join(self.home, "authorized_keys"))
        for f in ("hostkey", "id", "authorized_keys"):
            os.chmod(os.path.join(self.home, f), 0o600)
        cfg = os.path.join(self.home, "sshd_config")
        with open(cfg, "w") as f:
            f.write(f"""Port {PORT}
ListenAddress 127.0.0.1
HostKey {self.home}/hostkey
PidFile {self.home}/sshd.pid
AuthorizedKeysFile {self.home}/authorized_keys
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
Subsystem sftp {sftp_server_path()}
""")
        log = open(os.path.join(self.home, "sshd.log"), "w")
        self.proc = subprocess.Popen(["/usr/sbin/sshd", "-f", cfg, "-D", "-e"], stdout=log, stderr=log)
        # 立ち上がるまで待つ。固定の sleep ではなく、実際に繋がるまで。
        for _ in range(50):
            time.sleep(0.1)
            with socket_try() as ok:
                if ok:
                    return
        raise SystemExit(f"sshd が立ちませんでした: {self.home}/sshd.log")

    def stop(self):
        if self.proc:
            self.proc.terminate()
            self.proc.wait(timeout=5)


class socket_try:
    def __enter__(self):
        import socket
        s = socket.socket()
        s.settimeout(0.2)
        try:
            s.connect(("127.0.0.1", PORT))
            return True
        except OSError:
            return False
        finally:
            s.close()

    def __exit__(self, *a):
        return False


class Engine:
    """`cian-server` を1本、stdin/stdout で叩く。

    **読み取りは専用スレッドで、`select` は使わない。** 最初は select で待って
    いて、転送は成功しているのに `done` が来ない ── と3回報告した。届いていた。
    Python の `readline` はバッファで読むので、`call` が返事を読んだときに次の
    行まで一緒に取り込んでいて、その行はもうカーネルには無い。`select` は
    「読むものは無い」と答え、こちらは「エンジンが黙っている」と読んだ。

    **道具の報告が観測を隠した。** エンジンを直しにいくところだった。
    """

    def __init__(self):
        import queue
        import threading
        exe = os.path.join(HERE, "target", "debug", "cian-server")
        if not os.path.exists(exe):
            raise SystemExit("先に `cargo build -p cian-server` を")
        self.p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  text=True, bufsize=1, cwd=HERE)
        self.n = 0
        self.events = []
        self.replies = {}
        self.q = queue.Queue()
        self.dead = False

        def pump():
            for line in self.p.stdout:
                try:
                    self.q.put(json.loads(line))
                except json.JSONDecodeError:
                    pass
            self.dead = True

        threading.Thread(target=pump, daemon=True).start()

    def _take(self, secs):
        import queue
        try:
            return self.q.get(timeout=secs)
        except queue.Empty:
            return None

    def call(self, method, **params):
        self.n += 1
        want = self.n
        self.p.stdin.write(json.dumps({"id": want, "method": method, "params": params}) + "\n")
        self.p.stdin.flush()
        if want in self.replies:
            return self.replies.pop(want)
        while True:
            d = self._take(20)
            if d is None:
                raise RuntimeError(f"{method}: 返事がありません")
            if "event" in d:
                self.events.append(d)
            elif d.get("id") == want:
                if "error" in d:
                    raise RuntimeError(f"{method}: {d['error']}")
                return d.get("ok")
            else:
                self.replies[d["id"]] = d.get("ok")

    def wait_done(self, secs=15.0):
        """転送は worker で走る。`done` が来るまで待つ。

        先に貯まっている分を見てから待つ ── 速い転送は、次の要求の返事を待って
        いる間に終わっていて、`call` がもう拾っている。
        """
        for d in self.events:
            if d.get("event") == "done":
                self.events.remove(d)
                return d
        end = time.time() + secs
        while time.time() < end:
            d = self._take(0.3)
            if d is None:
                continue
            if "event" in d:
                self.events.append(d)
                if d["event"] == "done":
                    self.events.remove(d)
                    return d
            elif "id" in d:
                self.replies[d["id"]] = d.get("ok")
        return None

    def close(self):
        try:
            self.p.stdin.close()
        except Exception:
            pass


def main():
    if sftp_server_path() is None or not os.path.exists("/usr/sbin/sshd"):
        print("sshd / sftp-server がありません（Windows ではこの検査は動きません）")
        return 0
    keep = "--keep" in sys.argv
    # `realpath`: on macOS a temp dir is `/var/folders/…` and the same place
    # answers as `/private/var/folders/…`. The server reports paths as it
    # resolved them, so a mark set under the other spelling matches no row —
    # and `targets()` then quietly falls back to the cursor, which is a
    # different file. The check would have been testing the wrong one.
    home = os.path.realpath(tempfile.mkdtemp(prefix="cian-remote-"))
    root = os.path.join(home, "root")
    local = os.path.join(home, "local")
    os.makedirs(os.path.join(root, "sub"))
    os.makedirs(local)
    open(os.path.join(root, "one.txt"), "w").write("one\n")
    open(os.path.join(root, "sub", "deep.txt"), "w").write("deep\n")
    open(os.path.join(local, "up.txt"), "w").write("x" * 5000)

    server = Server(home)
    server.start()
    e = Engine()
    bad = []

    def check(name, got, want):
        ok = got == want
        print(f"  {'ok' if ok else '✗ '} {name}")
        if not ok:
            print(f"       期待 {want!r}")
            print(f"       実際 {got!r}")
            bad.append(name)

    def here(*parts):
        return sorted(os.listdir(os.path.join(*parts)))

    def connect(pane, path):
        return e.call("connect", pane=pane, host="127.0.0.1", port=PORT,
                      user=os.environ.get("USER") or os.environ.get("LOGNAME"),
                      key=server.key, path=path)

    try:
        print("鍵で入る")
        e.call("list", pane="left", path=local)
        r = connect("right", root)
        check("接続してディレクトリが読める",
              sorted(x["name"] for x in r["pane"]["entries"] if not x.get("parent")),
              ["one.txt", "sub"])

        # `..` is navigation, not a row: first whatever the order, shown
        # with dotfiles off, absent only at `/`. It used to sort among the
        # directories — at the bottom in reverse — so a home with thirty
        # dot-directories showed no way up, and a shallow one did.
        print("`..` は常に先頭")
        check("繋いだ直後に `..` が先頭", r["pane"]["entries"][0].get("parent"), True)
        check("カーソルは `..` の次", r["pane"]["cursor"], 1)
        e.call("sort", pane="right", key="name", reverse=True)
        v = e.call("remotelist", pane="right", path=root)
        check("逆順でも先頭", v["pane"]["entries"][0].get("parent"), True)
        e.call("sort", pane="right", key="name", reverse=False)
        e.call("hidden", pane="right")
        v = e.call("remotelist", pane="right", path=root)
        check("隠しファイルを消しても残る", v["pane"]["entries"][0].get("parent"), True)
        e.call("hidden", pane="right")
        v = e.call("remotelist", pane="right", path="/")
        check("`/` には無い", any(x.get("parent") for x in v["pane"]["entries"]), False)
        e.call("remotelist", pane="right", path=root)

        print("ローカル → サーバ")
        e.call("copy", pane="left", paths=[os.path.join(local, "up.txt")])
        done = e.wait_done()
        check("転送が done を出す", (done or {}).get("ok"), 1)
        check("サーバ側に届いている", "up.txt" in here(root), True)
        check("中身も届いている", os.path.getsize(os.path.join(root, "up.txt")), 5000)

        print("サーバ → ローカル")
        e.call("copy", pane="right", paths=[os.path.join(root, "one.txt")])
        e.wait_done()
        check("手元に降りている", "one.txt" in here(local), True)

        print("chmod")
        e.call("setmarks", pane="right", paths=[os.path.join(root, "one.txt")])
        e.call("chmod", pane="right", spec="600")
        check("モードが変わっている",
              oct(stat.S_IMODE(os.stat(os.path.join(root, "one.txt")).st_mode)), "0o600")

        print("サーバ内の移動")
        e.call("setmarks", pane="right", paths=[os.path.join(root, "one.txt")])
        e.call("remoteop", pane="right", what="move", to=os.path.join(root, "sub"))
        check("移った", "one.txt" in here(root, "sub"), True)
        check("元から消えた", "one.txt" not in here(root), True)

        print("サーバ → サーバ（同一ホスト = rename）")
        connect("left", os.path.join(root, "sub"))
        connect("right", root)
        r = e.call("move", pane="left", paths=[os.path.join(root, "sub", "one.txt")])
        check("rename で済ませた", r.get("renamed"), 1)
        check("戻ってきた", "one.txt" in here(root), True)

        print("ディレクトリごと送る")
        tree = os.path.join(local, "proj")
        os.makedirs(os.path.join(tree, "src", "deep"))
        open(os.path.join(tree, "README.md"), "w").write("r")
        open(os.path.join(tree, "src", "main.rs"), "w").write("m")
        open(os.path.join(tree, "src", "deep", "x.rs"), "w").write("x")
        e.call("list", pane="left", path=local)
        e.call("setmarks", pane="left", paths=[tree])
        e.call("copy", pane="left")
        done = e.wait_done()
        check("3ファイルとして数えた", (done or {}).get("ok"), 3)
        check("木ごと届いている",
              sorted(os.listdir(os.path.join(root, "proj", "src"))), ["deep", "main.rs"])
        check("一番深いところも", os.path.exists(os.path.join(root, "proj", "src", "deep", "x.rs")), True)

        print("転送前に中身を数える")
        plan = e.call("transferplan", paths=[tree])
        check("フォルダのファイル数", plan["files"], 3)
        check("バイト数も出る", plan["bytes"] > 0, True)
        check("行ごとにも出る", plan["rows"][0]["is_dir"], True)

        print("ディレクトリごと降ろす")
        shutil.rmtree(tree)
        e.call("list", pane="left", path=local)
        # 右ペインは proj ができる前の一覧を持っている。読み直さないと
        # マークが当たる行が無い。
        e.call("remotelist", pane="right", path=root)
        e.call("setmarks", pane="right", paths=[os.path.join(root, "proj")])
        e.call("copy", pane="right")
        e.wait_done()
        check("木ごと降りている",
              os.path.exists(os.path.join(local, "proj", "src", "deep", "x.rs")), True)

        print("サーバ → サーバ（コピーは中継）")
        # 両ペインとも繋ぎ直す ── 上のディレクトリの検査で左は手元に戻して
        # いる。**足場の前提は毎回書き直すこと**：直前の検査が置いていった
        # 状態に乗ると、検査は通ったり落ちたりするだけで何も言わなくなる。
        connect("left", os.path.join(root, "sub"))
        connect("right", root)
        r = e.call("copy", pane="right", paths=[os.path.join(root, "one.txt")])
        e.wait_done()
        check("中継で届いた", "one.txt" in here(root, "sub"), True)
        check("一時ファイルを残していない",
              [f for f in os.listdir(tempfile.gettempdir()) if f.startswith("cian-relay")], [])
        print("転送レートの上限")
        big = os.path.join(local, "big.bin")
        open(big, "wb").write(b"x" * 400_000)
        e.call("list", pane="left", path=local)
        connect("right", root)
        e.call("limit", spec="200k")
        e.call("setmarks", pane="left", paths=[big])
        t0 = time.time()
        e.call("copy", pane="left")
        e.wait_done(30)
        slow = time.time() - t0
        e.call("limit", spec="off")
        os.remove(os.path.join(root, "big.bin"))
        e.call("setmarks", pane="left", paths=[big])
        t0 = time.time()
        e.call("copy", pane="left")
        e.wait_done(30)
        fast = time.time() - t0
        # 400KB を 200KB/s で送れば2秒前後。上限なしは loopback なので一瞬。
        check("上限が実際に効いている", slow > 1.4 and slow > fast * 2,
              True)
        print(f"       上限あり {slow:.2f}s / なし {fast:.2f}s")

        print("リモートのファイルを開いて書き戻す")
        # **カーソルは窓と同じ形で送る。** `remoteview` はマークではなく
        # カーソルの行を開く（`selected()`）。窓版は `ask()` が毎回
        # `cursors: {left, right}` を載せているので、足場も同じにしないと
        # 「エンジンが違う行を開く」と読めてしまう ── 一度そう読んだ。
        v = e.call("remotelist", pane="right", path=root)
        rows = [x["name"] for x in v["pane"]["entries"]]
        at = rows.index("one.txt")
        v = e.call("remoteview", pane="right", cursors={"left": 0, "right": at})
        local_copy = v.get("path")
        check("落として開けた", bool(local_copy) and os.path.exists(local_copy), True)
        with open(local_copy, "w") as f:
            f.write("changed\n")
        e.call("remotesave", pane="right", path=local_copy)
        check("サーバに書き戻った", open(os.path.join(root, "one.txt")).read(), "changed\n")

        print("リモートの作成・改名・削除")
        e.call("remoteop", pane="right", what="mkdir", name="made")
        os.makedirs(os.path.join(root, "made", "inner"), exist_ok=True)
        open(os.path.join(root, "made", "inner", "f.txt"), "w").write("f")
        e.call("remotelist", pane="right", path=root)
        e.call("setmarks", pane="right", paths=[os.path.join(root, "made")])
        e.call("remoteop", pane="right", what="delete")
        check("中身ごと消えた", os.path.exists(os.path.join(root, "made")), False)
        # `:aijunk` was cut with the rest of the file-operating AI (request
        # 180); the check that it refused a remote pane outlived it here and
        # kept this run red for a command that no longer exists.
    finally:
        e.close()
        if keep:
            print(f"\nサーバは残してあります: {home}（鍵 {server.key}、ポート {PORT}）")
        else:
            server.stop()
            shutil.rmtree(home, ignore_errors=True)

    print("=" * 72)
    if bad:
        print(f"  リモートで {len(bad)} 件落ちています: {', '.join(bad)}")
        print("=" * 72)
        return 1
    print("  リモートは本物のサーバ相手に通っています")
    print("=" * 72)
    return 0


if __name__ == "__main__":
    sys.exit(main())
