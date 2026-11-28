#!/usr/bin/env python3
"""**全部触る。** 端末版のコマンドを一つずつ叩いて、四つの測定方法で見る。

`tui-drive.py` は「一巡」で 23 手を打つ。それは**壊れていないか**の監視で、
「全機能の動作検証」ではない。`commands.rs` が答える verb は **180 個近く**あり
（このツールが毎回自分で数えるので、固定の一覧は持たない）、一巡で触れるのは
ひと握り。触ったことのない道は、間違っていても誰も気づかない。

    ~/.cache/cian-tui-env/bin/python scripts/sweep.py            # 全部
    ~/.cache/cian-tui-env/bin/python scripts/sweep.py --list     # 一つずつ結果を出す
    ~/.cache/cian-tui-env/bin/python scripts/sweep.py theme du   # 名前を指定して

## 一つの verb について見るもの

| | 見るもの | 外すと何が起きるか |
| --- | --- | --- |
| ㋐ 反応 | 画面が変わるか | 「書いたのに届かない」 |
| ㋒ 桁 | 全行が端末の桁数ちょうどか | 全角の片割れ（日本語でしか出ない） |
| ㋓ 迷子 | `unknown command` と言われないか | 名乗っているのに答えない道 |
| ㋕ 生存 | 叩いたあと、まだ動いているか | そこで固まる道 |

㋑（言葉）はここでは見ない。文字列を読むのは `kotoba.py` の仕事で、
**同じことを二つのツールに見させると、片方を直したときもう片方が何も報告しない。**

## 触らないもの（理由つき）

**「危ないから飛ばす」ではなく「このツールでは見られないから飛ばす」。**
飛ばしたものは数に出して、別の見方があるならそれを書く。

## 反応が無くても、壊れているとは限らない

`:pwd` は状態行に道を出すだけ、`:mark` は既にマークが無ければ何も起きない。
だから **✗ は「調べる価値がある」であって「バグ」ではない**。`QUIET_OK` に
理由を書いて外す。消すのではなく、**なぜ指摘しないのか**を残す
（`audit.py` の `TERM_OK` と同じ作法）。
"""

from __future__ import annotations

import importlib.util
import os
import re
import shutil
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("td", ROOT / "scripts" / "tui-drive.py")
td = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(td)


def verbs() -> list[str]:
    """`commands.rs` が答える verb ── `parity.py` と同じ読み方。

    **一覧を手で書かない。** 手で書いた一覧は、コマンドが増えた日に古くなり、
    「全部触った」と言いながら新しい道を素通りする。
    """
    text = (ROOT / "crates/cian-tui/src/commands.rs").read_text(encoding="utf-8")
    out: set[str] = set()
    for m in re.finditer(
        r'^\s{12}((?:"[a-zA-Z0-9_!-]+"\s*\|\s*)*"[a-zA-Z0-9_!-]+")\s*(?:if [^=]*)?=>', text, re.M
    ):
        out |= set(re.findall(r'"([^"]+)"', m.group(1)))
    return sorted(out)


# ── このツールでは見られないもの ────────────────────────────────────────
SKIP = {
    # verb ではない。`:cd -`（ひとつ前のディレクトリ）の**引数**が、
    # `parity.py` の数え方（12桁インデントの match 腕）に verb として拾われて
    # いる。`parity.py` のコマンド件数はそのぶん多い。この全件検査が迷子として
    # 先に見つけた
    "-": "`:cd -` の引数であって、コマンドではない",
    "q": "終わる。叩いたら以降が測れない",
    "quit": "同上",
    # 端末を外の編集機に明け渡す。pty の中身が cian のものでなくなるので、
    # ここから先は何を読んでも cian の画面ではない。**別の見方**: 端末版を
    # 手で起動して `:vim` を叩き、戻ってきた画面を見る
    "vi": "外部エディタに端末を渡す",
    "vim": "同上",
    "nvim": "同上",
    "e": "外部エディタ（`edit_selected_file`）",
    "edit": "同上",
    "office": "OS の関連付けで別のアプリが開く",
    "officelink": "同上",
    # 相手が要る。手元に sshd も SFTP サーバも無いので、ここで見えるのは
    # 「繋がらなかった」だけ。**別の見方**: `scripts/remote.py` が 127.0.0.1 に
    # sshd を立てる
    "ssh": "相手が要る（接続待ちで止まる）",
    "sftp": "同上",
    "remote": "同上",
}

# 入ると出られなくなる（同じ verb をもう一度で出る）モード。
MODAL = {"key"}

# ── Esc で素の一覧に戻らないのが正しいもの ────────────────────────────
BACK_OK = {
    "shell": "シェル枠は開いたまま残る。Esc は焦点をファイルへ戻すだけ",
    "sync": "同上（シェル枠を開いてから同時入力を切り替える）",
    "broadcast": "同上",
    "preview": "トグル。もう一度 :preview で消える",
    "branch": "トグル（`toggle_branch_view`）。もう一度 :branch で戻る",
    "zoom": "トグル（`toggle_zoom`）。もう一度 :zoom で戻る",
    # **このツールの見立ての方が間違っていた。** `:touch` は選んだものの日時を
    # 更新するので、一覧の日時が変わる。**変わるのが仕事**で、起動時の画面に
    # 戻れないのは当たり前。単体では「選択されていません」で終わるため通り、
    # 長い走行で前のマークが残っているときだけ落ちていた（順番に依存する
    # 見かけの不具合）。
    "touch": "日時を更新する。一覧の日時が変わるので、起動時の画面には戻らない",
    "paste": "貼り付け。一覧が増えるのが仕事（`Ctrl+V` と同じ）",
    # **Esc では閉じない。** 一括リネームは編集画面で、既定の文法は vim ──
    # Esc は挿入をやめるだけで、閉じるのは `:q`。エディタとしては筋が通って
    # いるが、**画面にその案内が無い**（下端に出るのは桁と `text` だけ）。
    # 素人が開いたら出られない。**直すなら案内で、挙動ではない。**
    "renamelist": "編集画面。vim 文法なので閉じるのは :q（案内が無いのは別途）",
}

# ── `unknown command` と言われるのが正しいもの ────────────────────────
#
# **パネルを開いていないと答えない名前。** 閲覧パネルの中でだけ生きるので、
# 一覧の上で叩けば「そんなコマンドはありません」が正しい応答です。
# このツールは閲覧パネルを開かないので、**見られないものとして外します**
# （中で壊れたときは、この全件検査では捕まりません）。
UNKNOWN_OK = {
    "g": "閲覧パネルの中だけ（行の絞り込み）。`Popup::Viewer` のときだけ答える",
    "v": "同上（除外する側の絞り込み）",
    "s": "同上（置換の入力欄を開く）",
}

# ── 何も起きないのが正しいもの ────────────────────────────────────────
QUIET_OK = {
    "pwd": "目印そのもの。`prime` が毎回これを打つので、変わらなくて当たり前",
    "deselect": "マークが無ければ外すものが無い",
    "unmark": "同上",
    "select": "既に全部マークされていなければ動くが、一巡の順で無マーク",
    "back": "履歴が空なら戻り先が無い",
    "undo": "取り消すものが無い",
    "redo": "やり直すものが無い",
    "stage": "git の作業ツリーではない作業用ディレクトリなので、足すものが無い",
    "svnadd": "同上（svn でもない）",
    "local": "リモートペインでなければ、戻る先が無い（`leave_remote_pane`）",
    "tabclose": "タブが1つだけなら閉じない（`close_active` が `len() > 1` で守っている）",
    "refresh": "同じ中身を読み直すので、画面は同じ",
    "refresh!": "同上",
    "rescan": "同上",
    "reload": "設定を読み直す。作業用ディレクトリに init.lua が無いので変わらない",
    "source": "同上",
    "redraw": "同じ画面を描き直す。変わったら逆におかしい",
    "ls": "同上（読み直し）",
    "dir": "同上",
}


def sandbox() -> str:
    """一つの作業用ディレクトリを全部の verb で使い回す。

    verb ごとに立て直すと verb の数だけ起動して 10 分を超える。**代わりに、一つ叩く
    たびに Esc を3回**打ってパネルを閉じ、次の verb を素の一覧から始める。
    """
    d = tempfile.mkdtemp(prefix="cian-sweep-")
    for sub in ("from", "to", "config", "from/深い階層"):
        os.makedirs(os.path.join(d, sub), exist_ok=True)
    open(os.path.join(d, "from/a.txt"), "w").write("one two three\nsecond line\n")
    open(os.path.join(d, "from/b.md"), "w").write("# 見出し\n\n- [ ] 牛乳\n")
    open(os.path.join(d, "from/長い名前のファイル.txt"), "w").write("日本語の中身\n")
    open(os.path.join(d, "from/深い階層/inner.txt"), "w").write("inner\n")
    return d


class Bench:
    """一つの端末版を使い回し、**素の一覧に戻れなくなったら立て直す**。

    最初は一つの実体で 139 個を叩いていた。速いが、**どの verb のせいかが
    分からなくなる。** `:image` のパネルを開いたまま次を叩き、その次で死んで、
    死因が最後の verb に見えた。実際 `:ime` は単体では何ともない。

    だから毎回、Esc を3回打って**起動直後の画面と見比べる**。戻っていなければ
    立て直して、その verb を「Esc で戻らない」に数える。これは速さのための
    妥協ではなく、**それ自体が測定方法**（㋔）。開いたパネルが閉じないのは、
    次に何をしても付いて回るバグだから。
    """

    def __init__(self, d, fresh_each=True):
        self.d = d
        self.t = None
        self.baseline = ""
        # 戻らなかったときの違い（✗ の理由）。
        self.why = ""
        # **既定は「一つ触るたびに立て直す」。**
        # 使い回すと前のものの状態が残り、**次のものの結果がそれに化ける** ──
        # 78 個のキーを続けて押した回、10 個が「反応なし」に出て、立て直して
        # 押し直したら 8 個は動いた。`gg` は直前の `Shift+U` で既に先頭に
        # いただけ、`Space` は `..` の上でマークを断られていただけだった。
        # **依頼の言葉どおり「まっさらな気持ちで」** ── 速さは `--fast` で。
        self.fresh_each = fresh_each
        self.start()

    def start(self):
        """立て直して、**描き終わってから**起動直後の画面を覚えます。

        **決まった秒数だけ待つのでは足りませんでした。** 2026-11-28、全部を
        走らせたときだけ1本目の verb が「Esc で戻らない」と出ました。残した
        理由を読むと、比べる相手（起動直後の画面）の0行目が
        `^[Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA` ── 端末への問い合わせの生バイトで、
        1〜2行目は空でした。実行ファイルが冷えている1本目だけ 3 秒では描き
        終わらず、**壊れた画面を正しい画面として 165 個すべてと比べていました。**
        描けたかどうかは秒ではなく画面で決めます。
        """
        for attempt in range(3):
            if self.t is not None:
                self.t.close()
            self.t = td.Tui([f"{self.d}/from", f"{self.d}/to"], cols=120, rows=36,
                            env={"CIAN_CONFIG_DIR": f"{self.d}/config"})
            self.t.pump(3.0)
            self.t.send("Esc", 0.3)
            if self.drawn():
                self.baseline = self.body()
                return
        raise SystemExit(
            "起動直後の画面が描き終わりません。比べる相手が壊れたままでは、"
            "どの verb の結果も当てになりません")

    def drawn(self, secs: float = 10.0) -> bool:
        """一覧が描けたか。**枠の角が0行目に在り、生の escape が無いこと。**

        文字で探しません（`名前` は日本語の見出しで、言語の設定で変わります）。
        枠の角は端末版がどの言語でも描くもので、`^[` は描き終わっていない証拠
        です。
        """
        end = time.time() + secs
        while True:
            rows = self.body().splitlines()
            head = rows[0] if rows else ""
            if head[:1] in td.CORNERS and "^[" not in self.body() \
                    and sum(1 for r in rows if r.strip()) >= 3:
                return True
            if time.time() >= end:
                return False
            self.t.pump(0.3)

    def body(self) -> str:
        """画面のうち、**下の2行を除いた**ところ。

        下の2行はキーの案内と状態行で、状態行は叩いた verb の返事が出る場所。
        そこまで見比べると「知らせが出ている」を「戻っていない」と読む ──
        最初そうやって 98 個を挙げ、その大半が正しく動いた verb だった。
        **見たいのは「パネルが開いたままか」**で、返事が出たかどうかではない。
        """
        # **行末の空白は落とす。** 見えない差で「戻っていない」と報告していた。
        # 2026-11-28、`:about` が全件検査のときだけ✗になり、残した理由を読むと
        # 15〜17 行目の違いは行末の空白だけだった。パネルは閉じている。
        # 開いたままのパネルは見える文字を変えるので、空白を落としても
        # 見落とさない。
        return "\n".join(
            l.rstrip() for l in self.t.text().splitlines()[: self.t.rows - 2])

    def reset(self) -> bool:
        """素の一覧に戻す。戻せたら True、立て直したら False。

        **戻らなかったときは、何がどう違ったかを残します**（`self.why`）。
        2026-11-28、`:about` が全部走らせたときだけ「戻らない」と出て、単体
        では3回続けて通りました。そのとき手元に残っていたのは「戻らない」の
        4文字だけで、**パネルが開いたままなのか、画面の1桁が違うだけなのかを
        あとから確かめる術がありませんでした。** ✗ の理由を捨てるツールは、
        同じ✗をもう一度出すまで何も教えません。
        """
        self.why = ""
        if self.t.dead:
            self.start()
            return False
        for _ in range(3):
            self.t.send("Esc", 0.15)
        back = self.body() == self.baseline
        if not back:
            self.why = self.diff_from_baseline()
        if back and not self.fresh_each:
            return True
        self.start()
        return back

    def diff_from_baseline(self) -> str:
        """いまの画面と起動直後の画面の違い。行番号つきで、最初の3行だけ。"""
        now, was = self.body().splitlines(), self.baseline.splitlines()
        out = []
        for y in range(max(len(now), len(was))):
            a = was[y] if y < len(was) else ""
            b = now[y] if y < len(now) else ""
            if a != b:
                out.append(f"{y}行目 起動直後 {a.strip()[:40]!r} → いま {b.strip()[:40]!r}")
            if len(out) == 3:
                break
        return " / ".join(out) if out else "字は同じで、行数だけ違います"

    def close(self):
        if self.t is not None:
            self.t.close()


def typed(t, verb: str) -> bool:
    """`:` を開いてから verb を打つ。**開いたことを確かめてから打つ。**

    最初は `:` のあと 0.2 秒で打ち始めていた。プロンプトが間に合わないと
    `r`（リネーム）が先に効いて、残りが**新しい名前として打ち込まれる** ──
    作業用ディレクトリのディレクトリが `深い階層edraw` に改名されて、`:redraw` を測った
    つもりの画面がリネームの結果だった。**測るつもりのものを測っていない**
    のが、いちばん高くつく壊れ方。
    """
    for _ in range(6):
        t.send(":", 0.25)
        if any(l.startswith(":") for l in t.text().splitlines()):
            break
    else:
        return False
    for ch in verb:
        t.send(ch, 0.05)
    t.send("Enter", 0.8)
    return True


def prime(t):
    """状態行に**目印**を置いてから測る。

    状態行は次の verb を叩いても消えない。だから前の verb の返事が残ったまま
    次を測ると、**同じ返事を返す verb が並んだとき「反応なし」に見える** ──
    最初そうやって 17 個を挙げ、そのうち 12 個は AI 未設定・バージョン管理下
    ではない、と**きちんと答えていた**。答えが同じだっただけだった。

    `:pwd` は道を状態行に出す（ついでにクリップボードへも入れる）。どの verb
    とも重ならない返事なので、目印に使える。
    """
    typed(t, "pwd")


def run(bench, verb: str) -> dict:
    """`:verb` を叩いて、五つの測定方法を読む。"""
    t = bench.t
    prime(t)
    before = t.text()
    opened = typed(t, verb)
    after = t.text()
    alive = not t.dead
    short = td.half_cells(t) if alive else []
    unknown = "unknown command" in after or "そんなコマンドはありません" in after
    # **モードに入る verb は、その場で出す。** `:key` は以降の打鍵をぜんぶ
    # 状態行に映すので、そのままだと**後ろの verb がすべて「キーを見ている
    # だけ」になる。** Esc では出られない（窓版は出られる）。全件検査が
    # 「反応なし」を 8 個挙げた回、その大半がこれで隠れていた。
    if alive and verb in MODAL:
        typed(t, verb)
    came_back = bench.reset() if alive else False
    return {
        "verb": verb,
        "moved": after != before,
        "short": short,
        "unknown": unknown,
        "alive": alive,
        "back": came_back,
        "opened": opened,
        # **✗ の理由**。`came_back` が False のときだけ中身が入ります。
        "why": bench.why,
    }



# ══ キー ═══════════════════════════════════════════════════════════════
#
# コマンドと同じ四つの測定方法を、**マニュアルが名前を挙げているキー**に当てる。
# 一覧は `lib.rs` の `entry("…")` から取る。`keycover.py` が窓版について
# 見ているのと同じ出どころで、こちらは端末版を**実際に押す**。

MOUSE = {
    "drag in from Finder", "right-click", "wheel", "click a tab", "drag a border",
    "double-click", "drag an entry", "drag",
}

# 打鍵に直せない綴り。**報告せずに飛ばさない。** 走っていないものを「通った」と
# 読むのが、cian でいちばん高くつく間違い方。
UNTYPEABLE = {
    "F1-F8": "範囲の書き方であって、キー1つではない",
    "Ctrl+Shift+←→↑↓": "同上（四方向をまとめた書き方）",
    "Ctrl+": "`Ctrl+Shift+P` がマニュアルの改行で割れたもの",
    "Ctrl+ C": "同上",
    "Ctrl+Shift+": "同上",
}

# 押すと測れなくなるもの。
KEY_SKIP = {
    "q": "終わる",
    "Z": "`ZZ` の片割れ。単体では待ちに入る",
}

_NAMED = {
    "Esc": "\x1b", "Enter": "\r", "Tab": "\t", "Space": " ",
    "Backspace": "\x7f", "Bksp": "\x7f",
    "Up": "\x1b[A", "Down": "\x1b[B", "Right": "\x1b[C", "Left": "\x1b[D",
    "↑": "\x1b[A", "↓": "\x1b[B", "→": "\x1b[C", "←": "\x1b[D",
    "PgUp": "\x1b[5~", "PgDn": "\x1b[6~", "Home": "\x1b[H", "End": "\x1b[F",
    "F1": "\x1b[11~", "F2": "\x1b[12~", "F3": "\x1b[13~", "F4": "\x1b[14~",
    "F5": "\x1b[15~", "F6": "\x1b[17~", "F7": "\x1b[18~", "F8": "\x1b[19~",
    "F9": "\x1b[20~", "F10": "\x1b[21~", "F11": "\x1b[23~", "F12": "\x1b[24~",
}
_ARROW_CSI = {"Up": "A", "Down": "B", "Right": "C", "Left": "D",
              "↑": "A", "↓": "B", "→": "C", "←": "D"}
_FKEY_NUM = {"F1": 11, "F2": 12, "F3": 13, "F4": 14, "F5": 15, "F6": 17,
             "F7": 18, "F8": 19, "F9": 20, "F10": 21, "F11": 23, "F12": 24}


def encode(spec: str):
    """マニュアルの綴りを、端末が送るバイト列に。読めなければ None。

    修飾つきの数え方は 1 + Shift(1) + Alt(2) + Ctrl(4) ── `tui-drive.py` の
    `KEYS` と同じ。**Shift+文字は大文字そのもの**で、`Shift+Space` のように
    大文字にならないものは端末が区別できないので None を返す。
    """
    if spec in _NAMED:
        return _NAMED[spec]
    if len(spec) == 1 or (len(spec) == 2 and spec[0] == spec[1] and spec.isalpha()):
        return spec
    parts = spec.split("+")
    base = parts[-1]
    mods = {p.lower() for p in parts[:-1]}
    if not mods:
        return None
    code = 1 + (1 if "shift" in mods else 0) + (2 if "alt" in mods else 0) \
             + (4 if "ctrl" in mods else 0)
    if base in _ARROW_CSI:
        return f"\x1b[1;{code}{_ARROW_CSI[base]}"
    if base in _FKEY_NUM:
        return f"\x1b[{_FKEY_NUM[base]};{code}~"
    if base in ("PgUp", "PgDn"):
        return f"\x1b[{5 if base == 'PgUp' else 6};{code}~"
    if len(base) == 1 and base.isalpha():
        if mods == {"shift"}:
            return base.upper()
        if mods == {"ctrl"}:
            return chr(ord(base.upper()) - 64)
        if mods == {"alt"}:
            return "\x1b" + base
        # Ctrl+Shift+文字 と Ctrl+Enter のたぐいは、素の端末では送れない
        # （kitty の拡張が要る。この全件検査は `ESC[?u` に「使えない」と答えて
        # いるので、cian も素の綴りしか受け取らない）
        return None
    return None


def manual_keys() -> list[str]:
    text = (ROOT / "crates/cian-tui/src/lib.rs").read_text(encoding="utf-8")
    out: list[str] = []
    for k in re.findall(r'entry\(\s*"([^"]+)"', text):
        if k != k.lstrip() or k.startswith(":"):
            continue
        for alt in re.split(r"\s*(?:,|/)\s*", k):
            alt = alt.strip()
            if alt and not alt.startswith(":") and alt not in out:
                out.append(alt)
    return out


def sig(t) -> str:
    """画面の**見た目**の指紋 ── 文字と、塗られている桁の両方。

    `j` や `k` は文字を一字も変えない。選ばれている行は**色でしか言われて
    いない**ので、`text()` だけを比べると、カーソル移動が永久に「反応なし」に
    なる（`tui-drive.py` の⑤がシェルの範囲選択で発生したのと同じ原因）。
    """
    marks = []
    for y in range(t.rows):
        row = t.screen.buffer[y]
        for x in range(t.cols):
            c = row[x]
            if c.bg != "default" or c.reverse:
                marks.append((y, x))
    return t.text() + "\n#" + repr(marks)


def sweep_keys(listing: bool, fresh: bool = True) -> int:
    names = manual_keys()
    mouse = [k for k in names if k in MOUSE]
    untypeable = [k for k in names if k in UNTYPEABLE]
    skipped = [k for k in names if k in KEY_SKIP]
    todo = [k for k in names
            if k not in MOUSE and k not in UNTYPEABLE and k not in KEY_SKIP]
    unreadable = [k for k in todo if encode(k) is None]
    todo = [k for k in todo if encode(k) is not None]

    print("=" * 72)
    print(f"  マニュアルが名前を挙げるキーを全部押す。{len(todo)} 個"
          f"（マウス {len(mouse)} / 綴りが読めない {len(unreadable) + len(untypeable)}"
          f" / 押さない {len(skipped)}）")
    print("=" * 72)

    d = sandbox()
    bench = Bench(d, fresh_each=fresh)
    quiet, short, died, stuck = [], [], [], []
    try:
        for k in todo:
            t = bench.t
            # キーは `:pwd` で目印を置かない。その返事が状態行の
            # 「いまのファイル名」を隠すので、カーソル移動が見えなくなる。
            if k in KEY_CONTEXT:
                t.send(KEY_CONTEXT[k], 0.8)
            before = sig(t)
            t.send(encode(k), 0.55)
            moved = sig(t) != before
            alive = not t.dead
            cells = td.half_cells(t) if alive else []
            # 文脈を作ってから押したキーは、素の一覧に戻らなくて当たり前
            # （`J` でシェル枠を開いている）。㋔ は測らない。
            back = bench.reset() if alive else False
            if k in KEY_CONTEXT:
                back = True
            if listing:
                flags = []
                if not moved:
                    flags.append("反応なし")
                if cells:
                    flags.append(f"桁 {cells[:2]}")
                if not alive:
                    flags.append("死んだ")
                elif not back:
                    flags.append("Esc で戻らない")
                print(f"  {k:<16} {'  '.join(flags) if flags else 'ok'}")
            if not moved and k not in KEY_QUIET_OK:
                quiet.append(k)
            if cells:
                short.append((k, cells[:2]))
            if not alive:
                died.append(k)
                break
            if not back and k not in KEY_BACK_OK:
                stuck.append(k)
    finally:
        bench.close()
        shutil.rmtree(d, ignore_errors=True)

    print()
    print(f"  ㋐ 反応なし : {len(quiet)} 個" + (f"  {' '.join(quiet)}" if quiet else ""))
    print(f"  ㋒ 桁くずれ : {len(short)} 個")
    for k, c in short:
        print(f"      {k}  {c}")
    print(f"  ㋔ Esc で戻らない : {len(stuck)} 個"
          + (f"  {' '.join(stuck)}" if stuck else ""))
    print(f"  ㋕ 死んだ   : {len(died)} 個" + (f"  {' '.join(died)}" if died else ""))
    print()
    print("  このツールでは押せないもの（数に入れて、別の見方を書く）:")
    print(f"      マウス {len(mouse)} 個 ── {' / '.join(mouse)}")
    print("          → `tui-drive.py` の③（叩いた桁が当たるか）と `drive.js` が見る")
    for k in untypeable:
        print(f"      {k:<22} {UNTYPEABLE[k]}")
    for k in unreadable:
        print(f"      {k:<22} 素の端末では送れない綴り（kitty の拡張が要る）")
    for k in skipped:
        print(f"      {k:<22} {KEY_SKIP[k]}")
    print("=" * 72)
    if short or died or stuck or quiet:
        print("  ✗ 上を見ること")
        print("=" * 72)
        return 1
    print("  マニュアルのキーは、押せば何かが起きて、桁は揃っています")
    print("=" * 72)
    return 0


# **先に文脈を作ってから押すキー。** マニュアルの「シェル」の章にあるものは、
# ファイル一覧から押しても何も起きなくて当たり前 ── そこを「反応なし」に
# 並べると、本当に届いていないキーがその中に埋もれる。
KEY_CONTEXT = {
    "Shift+H": "J",       # ペイン間のフォーカス移動
    "K": "J",
    "w": "J",             # タブを閉じる
    "F1": "J",            # 前/次のタブ
    "F2": "J",
    "Shift+F1": "J",      # 分割ペインのフォーカス
    "Shift+F8": "J",      # 左右分割
    "Shift+F9": "J",
    "Shift+F10": "J",
    "Shift+F12": "J",     # 分割ペインのズーム
    "Shift+PgUp": "J",    # シェルの出力を遡る
    "Shift+←": "J",       # シェルの範囲選択
    "Shift+→": "J",
    "Shift+↑": "J",
}

# 押しても何も起きないのが正しいキー。**理由を書く。**
KEY_QUIET_OK = {
    "Esc": "閉じるものが無い。一巡の測定方法でも同じ（`tui-drive.py` の④）",
    "Left": "左ペインへ焦点を移す。起動時から左にいるので、動く先が無い",
    "F1": "前のタブへ。タブが1枚なら行き先が無い",
    "F2": "次のタブへ。同上",
    "Shift+F1": "分割ペインの間を移る。分割が1枚なら行き先が無い",
    # **これは候補として残す。** 分割が1枚のときの「ズーム」は、何も起きない
    # のが正しいのか、1枚をいっぱいに広げるべきなのかが決まっていない。
    # 決まっていないことを「正しい」と書かない。
    "Shift+F12": "分割ペインのズーム。分割が1枚のとき何もしない（是非は未決）",
    # マニュアル自身が「kittyキーボード対応が必要」と書いている。この全件検査は
    # `ESC[?u` に「使えない」と答えているので、届くのは素の 0x08 ── つまり
    # **端末の設定どおりに、何も届かないのが正しい**。WezTerm で
    # `enable_kitty_keyboard = true` にすると届く（keys.rs:2460）
    "Ctrl+H": "kitty キーボードが要る。素の端末には届かない（マニュアルにも書いてある）",
}

# 押したら素の一覧に戻らないのが正しいキー。**動くのが仕事**のキーたち。
KEY_BACK_OK = {
    "Ctrl+V": "貼り付け。一覧が増えるのが仕事",
    "Enter": "ディレクトリに入る。戻ったら逆におかしい",
    "b": "ブランチ表示のトグル。もう一度 b で戻る",
    "Backspace": "親ディレクトリへ上がる。戻ったら逆におかしい",
    "Bksp": "同上",
    "o": "反対ペインを同じ場所へ",
    "O": "同上（向きが逆）",
    "J": "シェル枠へ。枠は開いたまま残る",
    "F12": "焦点を画面いっぱいに。もう一度で戻る",
}


# ══ 猿 ═════════════════════════════════════════════════════════════════
#
# 全件検査は「マニュアルが名前を挙げているもの」を順に触る。**思いついた順でしか
# 触れない**ので、思いつかなかった組み合わせは永久に残る。依頼の言葉で
# 「僕が思いつかないようなこともたくさんあるだろうから」。
#
# そこで乱数で叩き続けて、**三つだけ**見る:
#
#   ㋕ 落ちないか。サーバ相手のツールが落ちるのは、いちばん高い故障
#   ㋒ 桁が崩れないか。日本語でしか出ない
#   ㋖ 戻れるか。Esc を何度押しても抜け出せないパネルに入らないか
#
# **種を出す。** 崩れたら `--seed` で同じ順を打ち直せる。再現できない
# バグ報告は、直したかどうかも確かめられない。

MONKEY_POOL = [
    "j", "k", "h", "l", "g", "G", "Enter", "Esc", "Tab", "Space", "v", "V",
    "y", "p", "d", "r", "a", "A", "o", "O", "m", "s", "t", "b", "n", "N",
    "f", "c", "z", "C", "T", "M", "J", "K", "L", "F3", "F5", "F9", "F10",
    "F12", "Up", "Down", "Left", "Right", "Backspace", "Ctrl+A", "Ctrl+C",
    "Ctrl+X", "Ctrl+V", "Ctrl+F", "Ctrl+G", "Ctrl+P", "Ctrl+H", "Shift+D",
    "Shift+U", "Shift+F", "Shift+S", "Shift+P", "Shift+H", "Shift+F8",
    "Shift+F9", "Shift+F10", "Alt+h", "Alt+l", "?", "=", "@", ":",
    # 打ち込む文字。プロンプトが開いていれば中身になり、開いていなければ
    # キーとして解釈される。**どちらも起きてよい**のが猿の役目。
    "x", "1", "-", ".", "/", "*", "あ", "漢", " ",
]


def monkey(steps: int, seed: int, listing: bool) -> int:
    import random

    rng = random.Random(seed)
    print("=" * 72)
    print(f"  乱数で {steps} 手（種 {seed}）── 落ちないか・桁が崩れないか・戻れるか")
    print("=" * 72)
    d = sandbox()
    bench = Bench(d, fresh_each=False)
    t = bench.t
    history: list[str] = []
    trouble: list[str] = []
    try:
        for i in range(steps):
            k = rng.choice(MONKEY_POOL)
            history.append(k)
            bytes_ = encode(k) or k
            t.send(bytes_, 0.08)
            if t.dead:
                trouble.append(f"{i + 1} 手目 {k!r} で落ちた")
                break
            # 桁は 20 手に一度だけ見る。毎手見ると走らせるのに時間がかかり、
            # **走らせない検査は無いのと同じ**。
            if (i + 1) % 20 == 0:
                cells = td.half_cells(t)
                if cells:
                    trouble.append(f"{i + 1} 手目までに全角の片割れ {len(cells)} 個: {cells[:3]}")
                if listing:
                    print(f"  {i + 1:4} 手 …{' '.join(history[-6:])}")
        # 最後に、Esc を何度押しても戻れるか。
        if not t.dead:
            for _ in range(8):
                t.send("Esc", 0.12)
            if bench.body() == bench.baseline:
                print("  Esc ×8 で素の一覧へ戻れました")
            else:
                # **これは「バグ」ではなく「候補」。** 猿はディレクトリを
                # 移り、タブを開き、配色を変える。戻らないのが当たり前の
                # 手をいくつも打っている。見たいのは「パネルから抜け出せない」ほう。
                print("  Esc ×8 で素の一覧には戻りませんでした（移動やタブは戻らなくて当然）")
                print(f"      最後の状態行: {t.status()[-70:]}")
    finally:
        bench.close()
        shutil.rmtree(d, ignore_errors=True)

    print("=" * 72)
    if trouble:
        for x in trouble:
            print(f"  ✗ {x}")
        print(f"  打った順（そのまま再現できます）:")
        print("      " + " ".join(history))
        print(f"  もう一度: ~/.cache/cian-tui-env/bin/python scripts/sweep.py --monkey {steps} --seed {seed}")
        print("=" * 72)
        return 1
    print(f"  {len(history)} 手、落ちず、桁も崩れませんでした")
    print("=" * 72)
    return 0


# ══ 意地の悪い名前 ═════════════════════════════════════════════════════
#
# 窓版には `node gui/drive.js --values` がある（設定画面の欄に意地の悪い値を
# 入れて往復させる）。こちらは**端末版の入力欄**で、行き先は Lua ではなく
# **ファイルシステム** ── 壊れ方が違う。
#
#   引用符・空白・日本語 … 名前として通るはず。通らないなら理由が出るはず
#   `/` と NUL          … OS が受け取らない。**断るはずで、通知なしに落ちてはいけない**
#   とても長い名前      … 255 バイトの壁。断り方が見えるか
#   先頭の `-`          … 引数と間違えないか
#   `..` と `.`         … 作らせてはいけない
#
# 見るのは「作れたか」ではなく、**作れなかったときに理由が出るか**。
# 何の表示もなく終わる欄が、cian でいちばん質の悪い壊れ方。

NASTY_NAMES = [
    ("引用符", 'a"b.txt'),
    ("空白", "a b.txt"),
    ("日本語", "日本語のファイル.txt"),
    ("先頭のダッシュ", "-rf.txt"),
    ("長い名前", "x" * 300),
    ("斜線", "a/b.txt"),
    ("点二つ", ".."),
    ("点一つ", "."),
]


def check_names(listing: bool) -> int:
    """端末版の「新規ファイル」に、意地の悪い名前を入れる。"""
    print("=" * 72)
    print(f"  端末版の入力欄に、意地の悪い名前を {len(NASTY_NAMES)} 通り")
    print("=" * 72)
    d = sandbox()
    bench = Bench(d, fresh_each=True)
    bad = []
    try:
        for what, name in NASTY_NAMES:
            t = bench.t
            t.send("a", 0.5)
            # **切らずに打つ。** 60 字で切っていたので、255 バイトの壁が
            # 一度も試されていなかった。試していないものを「通った」と
            # 読むのが、cian でいちばん高くつく間違い方。
            for ch in name:
                t.send(ch, 0.01)
            t.send("Enter", 0.9)
            st = t.status()
            # **一覧ではなく、状態行で判断する。** 長い名前は一覧で詰められる
            # ので、「一覧に出たか」で見ると作れたものを「何も報告しなかった」と読む。
            made = "created" in st or "作成" in st
            told = "お知らせ" in t.text() or any(
                w in st
                for w in ("できません", "ありません", "失敗", "denied", "使えません",
                          "長すぎ", "too long", "エラー", "不正")
            )
            mark = "作れた" if made else ("断った" if told else "**何も報告しなかった**")
            if listing:
                print(f"  {what:<14} {mark:<10} {st[-64:]}")
            if not made and not told:
                bad.append(f"{what}（{name[:20]}…）が、作られも断られもしない")
            if not t.alive_or_dead():
                bad.append(f"{what} で端末版が死んだ")
                break
            bench.reset()
    finally:
        bench.close()
        shutil.rmtree(d, ignore_errors=True)
    print("=" * 72)
    if bad:
        for b in bad:
            print(f"  ✗ {b}")
        print("=" * 72)
        return 1
    print("  どれも、作れるか、理由を言って断るかのどちらかです")
    print("=" * 72)
    return 0


def main() -> int:
    listing = "--list" in sys.argv
    if "--keys" in sys.argv:
        return sweep_keys(listing, "--fast" not in sys.argv)
    if "--names" in sys.argv:
        return check_names(listing)
    if "--monkey" in sys.argv:
        argv = sys.argv
        steps = int(argv[argv.index("--monkey") + 1]) if len(argv) > argv.index("--monkey") + 1 \
            and argv[argv.index("--monkey") + 1].isdigit() else 400
        seed = int(argv[argv.index("--seed") + 1]) if "--seed" in argv else 1
        return monkey(steps, seed, listing)
    want = [a for a in sys.argv[1:] if not a.startswith("-")]
    fresh = "--fast" not in sys.argv
    names = want or [v for v in verbs() if v not in SKIP]
    skipped = [v for v in verbs() if v in SKIP] if not want else []

    print("=" * 72)
    print(f"  端末版のコマンドを全部叩く。{len(names)} 個"
          f"（触らないもの {len(skipped)} 個）")
    print("=" * 72)

    d = sandbox()
    bench = Bench(d, fresh_each=fresh)
    rows = []
    try:
        for v in names:
            r = run(bench, v)
            rows.append(r)
            if listing:
                flags = []
                if not r["moved"]:
                    flags.append("反応なし")
                if r["short"]:
                    flags.append(f"桁 {r['short']}")
                if r["unknown"]:
                    flags.append("迷子")
                if not r["opened"]:
                    flags.append("プロンプトが開かない")
                if not r["alive"]:
                    flags.append("死んだ")
                elif not r["back"]:
                    flags.append("Esc で戻らない")
                print(f"  :{v:<14} {'  '.join(flags) if flags else 'ok'}")
    finally:
        bench.close()
        shutil.rmtree(d, ignore_errors=True)

    quiet = [r["verb"] for r in rows if not r["moved"] and r["verb"] not in QUIET_OK]
    short = [(r["verb"], r["short"]) for r in rows if r["short"]]
    unknown = [r["verb"] for r in rows if r["unknown"] and r["verb"] not in UNKNOWN_OK]
    died = [r["verb"] for r in rows if not r["alive"]]
    stuck = [r["verb"] for r in rows if r["alive"] and not r["back"] and r["verb"] not in BACK_OK]
    unopened = [r["verb"] for r in rows if not r["opened"]]

    print()
    print(f"  ㋐ 反応なし : {len(quiet)} 個"
          f"（何も起きないのが正しいもの {len(QUIET_OK)} 個は除く）")
    if quiet:
        print(f"      {' '.join(quiet)}")
    print(f"  ㋒ 桁くずれ : {len(short)} 個")
    for v, s in short:
        print(f"      :{v}  {s}")
    print(f"  ㋓ 迷子     : {len(unknown)} 個"
          f"（パネルを開いていないと答えないもの {len(UNKNOWN_OK)} 個は除く）"
          + (f"  {' '.join(unknown)}" if unknown else ""))
    print(f"  ㋔ Esc で戻らない : {len(stuck)} 個"
          f"（戻らないのが正しいもの {len(BACK_OK)} 個は除く）"
          + (f"  {' '.join(stuck)}" if stuck else ""))
    # **✗ には理由を付けます。** 「戻らない」の4文字だけでは、パネルが開いたまま
    # なのか画面の1桁が違うだけなのかが分かりません。
    for r in rows:
        if r["verb"] in stuck and r.get("why"):
            print(f"      :{r['verb']}  {r['why']}")
    if unopened:
        print(f"  ！ プロンプトが開かなかった : {' '.join(unopened)}"
              "（その回の結果は当てにならない）")
    print(f"  ㋕ 死んだ   : {len(died)} 個" + (f"  {' '.join(died)}" if died else ""))
    if skipped:
        print()
        print("  触らなかったもの（このツールでは見られない）:")
        for v in skipped:
            print(f"      :{v:<12} {SKIP[v]}")

    print("=" * 72)
    bad = (short + [(v, "") for v in unknown] + [(v, "") for v in died]
           + [(v, "") for v in stuck] + [(v, "") for v in unopened])
    if bad:
        print("  ✗ 上の桁くずれ・迷子・死んだ を見ること")
        print("=" * 72)
        return 1
    if quiet:
        print("  反応の無いものは**候補**です。正しいなら QUIET_OK に理由を書くこと")
        print("=" * 72)
        return 1
    print(f"  {len(names)} 個ぜんぶ、叩けば何かが起きて、桁は揃っていて、迷子はいません")
    print("=" * 72)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
