#!/usr/bin/env python3
"""動作検証を、1つのコマンドで走らせる。

**この道具は新しい検査ではありません。** 既にある検査と道具を、順番と準備つきで
まとめて走らせ、**走らせられなかったものを最後に名前で出します**。

    python3 scripts/verify.py              準備してから全部
    python3 scripts/verify.py --list       何を走らせるかだけ出します（走らせません）
    python3 scripts/verify.py --prepare    準備だけ
    python3 scripts/verify.py kotoba 窓版  名前の一部を指定して
    python3 scripts/verify.py --self-test  わざと失敗する段を混ぜ、赤を赤と報告するか

## 分類していない検査があると、この道具は失敗します

`scripts/*.py` を全部拾い、**どの節で走らせるか分類済みかを確かめます**。
分類していないものが1つでもあると、走る前に失敗します ── 足した検査が
一覧から落ちて「所見なし」になるのを防ぐためです（実際に `settingscover.py` が
片方の一覧から落ちていました）。

**勝手に拾って走らせることはしません。** `scripts/` には資材を公開する道具も
あり（`release-asset.py`）、「見つけたから走らせる」は危険です。

## 終了コードをパイプ越しに読みません

各段の出力はファイルに落とし、**終了コードは別に受け取ります**。
`cargo test | tail` の 0 を cargo の 0 と読み、2件落ちていたのに「全部緑」と
報告した事故があるためです。

## 記録の置き場は `~/.cache/cian-verify`

`/tmp` は macOS の掃除で消えます（2回踏んでいます）。
"""

from __future__ import annotations

import argparse
import os
import unicodedata
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VENV = Path.home() / ".cache" / "cian-tui-env"
MOCK = Path.home() / ".cache" / "cian-mock"
LOGS = Path.home() / ".cache" / "cian-verify"

# ── 機械の物差し（②で走らせます。引数は要りません）────────────────────
GATES = {
    "audit.py": "死にコード・重複・名前の割れ（辿り着けない道を含みます）",
    "claudemd.py": "CLAUDE.md が名乗る関門が実在するか",
    "configcover.py": "init.lua の設定を、窓版がいくつ読んでいるか",
    "designdoc.py": "設計書が名乗る関数・パス・リンクが実在するか",
    "i18n.py": "窓版の文言が、両方の言葉で言えるか",
    "keycover.py": "マニュアルが挙げるキーが、一度は押されているか",
    "kotoba.py": "画面に出る言葉がハウススタイルに従っているか",
    "names.py": "両前端が、同じ機能を同じ名前で打たせているか",
    "parity.py": "両前端が、同じものを同じ言葉で呼んでいるか",
    "requests.py": "依頼の台帳の約束が、いまも守られているか",
    "settingscover.py": "設定画面が init.lua のどこまでを触れているか",
    "unreached.py": "いまの版が、届かなかったところを言っているか",
    "widths.py": "画面の幅を字数で測っているところ（日本語でずれます）",
}

# ── 検査でないもの（理由つきで外します）────────────────────────────────
NOT_A_CHECK = {
    "append_before_tests.py": "テストの手前に書き足す道具です",
    "fetch-vim.py": "同梱する vim を落としてくる道具です",
    "release-asset.py": "公開済みの Release に資材を足す道具です",
    "verify.py": "この道具そのものです",
}

# ── 引数が要るもの（別に扱います）──────────────────────────────────────
NEEDS_ARG = {
    "buildstamp.py": "資材の版名乗りを見る検査です。配る資材を引数に取るため、"
                     "CI が staging の直後に当てます",
    "winstart.py": "配る Windows の exe が素の Windows で起動するかを見る検査です。"
                   "exe を引数に取るため、CI が資材を上げる前に当てます",
}

# ── 本物で動かすもの（③で走らせます）────────────────────────────────────
HARNESS = {
    "tui-drive.py": "端末版を本物の pty で動かします",
    "sweep.py": "端末版の verb を一つずつ叩きます",
    "inject.py": "わざと詰まらせて、固まらないことを見ます",
    "remote.py": "自分専用の sshd を立て、本物の SFTP で通します",
}

# ── ここから届かないもの（毎回最後に出します）──────────────────────────
UNREACHED = [
    ("Windows そのもの", "`ci.yml` の Windows ジョブと、会社の実機"),
    ("彼の JIS 配列と IME", "実機のみ。`drive.js` は本物のキーを送りますが US 配列です"),
    ("AI の本物の応答", "実機のみ。ここでは `auth_mode = \"mock\"` で経路だけ通します"),
    ("配る資材の版名乗り", "`scripts/buildstamp.py <資材>`。CI の成果物に当てます"),
    ("見た目の良し悪し", "実機のみ。桁と文言は機械が見ますが、良いかどうかは見ません"),
]


def w(s: str) -> int:
    """その文字列が端末で占める桁数。

    **字数で測ると日本語でずれます。** cian 自身で6回出たバグと同じ形なので、
    この道具の出力も桁で揃えます（`scripts/widths.py` が見ているのと同じ話です）。
    """
    return sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in s)


def pad(s: str, n: int) -> str:
    """桁で左詰めします。桁が足りない場合は詰めません（切りません）。"""
    return s + " " * max(0, n - w(s))


class Step:
    def __init__(self, name, what, cmd, timeout=600, env=None, note=""):
        self.name, self.what, self.cmd = name, what, cmd
        self.timeout, self.env, self.note = timeout, env or {}, note
        self.rc = None
        self.took = 0.0
        self.log: Path | None = None


def unclassified() -> list[str]:
    """どの節にも分類されていない `scripts/*.py` を返します。"""
    known = set(GATES) | set(NOT_A_CHECK) | set(NEEDS_ARG) | set(HARNESS)
    return [p.name for p in sorted((ROOT / "scripts").glob("*.py")) if p.name not in known]


def run(step: Step) -> bool:
    """1段を走らせます。出力はファイル、**終了コードは別に受け取ります**。"""
    LOGS.mkdir(parents=True, exist_ok=True)
    step.log = LOGS / (step.name.replace("/", "_").replace(" ", "_") + ".log")
    env = dict(os.environ)
    env.update(step.env)
    at = time.time()
    with open(step.log, "w", encoding="utf-8") as f:
        f.write(f"$ {' '.join(step.cmd)}\n\n")
        f.flush()
        try:
            step.rc = subprocess.call(step.cmd, cwd=ROOT, stdout=f, stderr=subprocess.STDOUT,
                                      env=env, timeout=step.timeout)
        except subprocess.TimeoutExpired:
            step.rc = None  # 時間切れ
            sweep_leftovers()
    step.took = time.time() - at
    return step.rc == 0


def sweep_leftovers() -> None:
    """時間切れのあとに残った子を落とします。**この作業ツリーのものだけ**です。

    直接の子は `subprocess` が落としますが、窓版の Electron と注入で立てた
    エンジンは孫なので残ります。実際に `/dev/zero` を読み続けるエンジンが
    1つ残り、メモリを使い続けたことがあります。
    """
    for one in (f"{ROOT}/gui/node_modules/[e]lectron",
                f"{ROOT}/target/debug/[c]ian-server",
                f"{ROOT}/target/debug/[c]ian-tui"):
        subprocess.call(f'pkill -f "{one}"', shell=True)


def mark(step: Step) -> str:
    if step.rc == 0:
        return "ok"
    if step.rc is None:
        return "時間切れ"
    return "✗ "


# ── 準備 ────────────────────────────────────────────────────────────────

def git_head() -> str:
    out = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                         capture_output=True, text=True)
    return out.stdout.strip() if out.returncode == 0 else "?"


def stale_stamps() -> list[str]:
    """検証に使う実行ファイルが、いまの HEAD のものかを見ます。

    **これが無いと、古い実行ファイルを測ったまま全部が緑になります。**
    実際に端末版が3つ前のコミットのままでした。`-dirty` は許します ── 作業中は
    木が汚れているのが普通で、ここで見たいのは**どのコミットか**だけです。
    版そのものの名乗り（`-dirty` を許さない物差し）は `scripts/buildstamp.py`
    が配る資材に当てます。
    """
    import importlib.util

    spec = importlib.util.spec_from_file_location("buildstamp", ROOT / "scripts" / "buildstamp.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    head, out = git_head(), []
    for name in ("cian-tui", "cian-server"):
        at = ROOT / "target" / "debug" / name
        if not at.exists():
            out.append(f"{name} が建っていません")
            continue
        stamps = mod.baked(str(at))
        shas = {s.rstrip(")").rsplit("(", 1)[-1].removesuffix("-dirty") for s in stamps if "(" in s}
        if head not in shas:
            out.append(f"{name} は {'/'.join(sorted(shas)) or '不明'}（HEAD は {head}）")
    return out



def prepare(quiet=False) -> list[str]:
    """走らせるために要るものを揃えます。戻り値は揃えられなかったものです。"""
    missing = []

    def say(ok, what, how=""):
        if not quiet:
            print(f"  {'ok' if ok else '✗ '} {pad(what, 34)} {how}")

    # ① pty の venv と pyte ── 端末版を本物の pty で動かすために要ります
    py = VENV / "bin" / "python"
    if not py.exists():
        subprocess.call([sys.executable, "-m", "venv", str(VENV)])
    ok = py.exists()
    if ok and subprocess.call([str(py), "-c", "import pyte"],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) != 0:
        subprocess.call([str(py), "-m", "pip", "install", "-q", "pyte"],
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    ok = ok and subprocess.call([str(py), "-c", "import pyte"],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) == 0
    say(ok, "pty の venv と pyte", str(VENV))
    if not ok:
        missing.append("pty の venv（端末版の検査が走りません）")

    # ② 窓版の依存 ── electron が無いと窓版は1行も動きません
    ok = (ROOT / "gui" / "node_modules" / "electron" / "package.json").exists()
    if not ok:
        subprocess.call(["npm", "install", "--silent"], cwd=ROOT / "gui")
        ok = (ROOT / "gui" / "node_modules" / "electron" / "package.json").exists()
    say(ok, "窓版の依存（electron）", "gui/node_modules")
    if not ok:
        missing.append("窓版の依存（窓版の検査が走りません）")

    # ③ AI の mock 設定 ── 網も鍵も要らずに、AI の経路を端まで通せます
    MOCK.mkdir(parents=True, exist_ok=True)
    (MOCK / "init.lua").write_text(
        'cian.ai {\n'
        '  endpoint = "https://example.invalid/llmaoai",\n'
        '  model = "gpt-5-mini",\n'
        '  auth_mode = "mock",\n'
        '  python = "python3",\n'
        '}\n', encoding="utf-8")
    say(True, "AI の mock 設定", str(MOCK / "init.lua"))

    # ④ エンジンと端末版 ── 窓版・注入・リモートはビルド済みの実体を使います
    #
    # **`-p cian-tui` では端末版の実行ファイルは建ちません。** `cian-tui` という
    # 名前の実行ファイルは `cian-bin`（パッケージ名は `cian`）が作り、
    # `cian-tui` はそれが使うライブラリです。取り違えると、検査は**古い実行
    # ファイル**を相手にしたまま緑になります ── 実際にこの道具の最初の版が、
    # 3つ前のコミットの端末版を測っていました。
    at = time.time()
    rc = subprocess.call(["cargo", "build", "-q", "-p", "cian-server"], cwd=ROOT)
    rc += subprocess.call(["cargo", "build", "-q", "-p", "cian", "--bin", "cian-tui"], cwd=ROOT)
    say(rc == 0, "エンジンと端末版のビルド", f"{time.time() - at:.0f}s")
    if rc != 0:
        missing.append("ビルド（これが通らないうちは何も測れません）")

    # ⑤ 建った実体が、いまの HEAD のものか
    stale = stale_stamps()
    say(not stale, "実体がいまの HEAD のものか", "；".join(stale) if stale else git_head())
    if stale:
        missing.append("古い実行ファイルを測っています（" + "；".join(stale) + "）")

    # ⑥ 窓版の Electron が残っていると互いを殺します。**この作業ツリーのものだけ**
    #    を落とします（前に取り違えて、別のアプリの窓を落としました）。
    subprocess.call(f'pkill -f "{ROOT}/gui/node_modules/[e]lectron"', shell=True)
    say(True, "居残りの Electron を落とす", "この作業ツリーのものだけ")
    return missing


# ── 段取り ──────────────────────────────────────────────────────────────

def plan() -> list[tuple[str, list[Step]]]:
    py = str(VENV / "bin" / "python")
    mock = {"CIAN_CONFIG_DIR": str(MOCK)}
    build = [
        Step("cargo test", "ロジックが合っているか",
             ["cargo", "test", "-q", "--workspace"], timeout=1800),
        Step("clippy", "警告が1つも無いか（CI と同じ物差し）",
             ["cargo", "clippy", "-q", "--workspace", "--all-targets", "--", "-D", "warnings"],
             timeout=1800),
    ]
    machine = [
        Step(g.removesuffix(".py"), what, [sys.executable, f"scripts/{g}"], timeout=600)
        for g, what in GATES.items()
    ]
    real = [
        Step("端末版 一巡", "本物の pty で、起動・枠・当たり・一巡・桁",
             [py, "scripts/tui-drive.py"], timeout=900),
        Step("端末版 総ざらい", "verb を一つずつ（反応・桁・迷子・生存）",
             [py, "scripts/sweep.py"], timeout=1800),
        Step("詰まらせる", "固まらないか・断る理由が本当か",
             [sys.executable, "scripts/inject.py"], timeout=900),
        Step("リモート", "自分専用の sshd を立てて本物の SFTP で通す",
             [sys.executable, "scripts/remote.py"], timeout=900),
        Step("窓版 一巡", "本物のキーとクリック（例外 0 件が基準）",
             ["node", "gui/drive.js"], timeout=900),
        Step("窓版 コマンド", "パレットのコマンドを一つずつ",
             ["node", "gui/drive.js", "--commands"], timeout=900),
        Step("窓版 値", "設定の値が画面に戻ってくるか",
             ["node", "gui/drive.js", "--values"], timeout=900),
        Step("窓版 AI", "説明からコマンド生成を、答えが返る側まで",
             ["node", "gui/drive.js", "--ai"], timeout=900, env=mock,
             note="mock の設定で走ります（本物の接続先ではありません）"),
    ]
    return [("① 建つか・通るか", build),
            ("② 機械の物差し", machine),
            ("③ 本物で動かす", real)]


def main() -> int:
    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument("pick", nargs="*", help="名前の一部")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--prepare", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()

    print("=" * 72)
    print("  動作検証 ── 走らせられるもの全部")
    print("=" * 72)

    unknown = unclassified()
    if unknown:
        print("\n分類していない scripts/*.py があります。走らせる前に止めます:")
        for u in unknown:
            print(f"  ✗ {u}")
        print("  検査なら GATES に1行、道具なら NOT_A_CHECK に理由を書いてください")
        return 1

    groups = plan()
    if a.self_test:
        # **赤を赤と報告するか。** 検査を足したら、わざと壊して鳴るのを見ます。
        groups.append(("わざと落とす（自己検査）", [
            Step("わざと失敗", "この段は必ず失敗します", [sys.executable, "-c", "raise SystemExit(3)"]),
            Step("わざと時間切れ", "この段は必ず時間切れです",
                 [sys.executable, "-c", "import time; time.sleep(30)"], timeout=2),
        ]))

    if a.list:
        for title, steps in groups:
            print(f"\n{title}")
            for s in steps:
                print(f"  ・{pad(s.name, 20)} {s.what}")
        print("\nここから届かないもの")
        for what, how in UNREACHED:
            print(f"  ・{pad(what, 24)} {how}")
        return 0

    print("\n準備")
    missing = prepare()
    if a.prepare:
        return 1 if missing else 0

    bad, late, ran = [], [], 0
    for title, steps in groups:
        shown = [s for s in steps if not a.pick or any(k in s.name or k in s.what for k in a.pick)]
        if not shown:
            continue
        print(f"\n{title}")
        for s in shown:
            run(s)
            ran += 1
            tail = ""
            if s.rc != 0 and s.log:
                # 先頭の `$ コマンド` は飛ばします ── 落ちた理由が読みたいためです。
                lines = [x for x in s.log.read_text(encoding="utf-8", errors="replace")
                         .splitlines()[1:] if x.strip()]
                tail = lines[-1][:48] if lines else ""
            print(f"  {pad(mark(s), 8)} {pad(s.name, 20)} {s.took:6.1f}s  "
                  f"{pad(s.what[:34], 36)} {tail}")
            if s.note:
                print(f"           {s.note}")
            if s.rc is None:
                late.append(s)
            elif s.rc != 0:
                bad.append(s)

    print("\n" + "=" * 72)
    print(f"  {ran} 段のうち、落ちたもの {len(bad)} 件、時間切れ {len(late)} 件")
    for s in bad + late:
        print(f"  {mark(s)} {s.name} ── 記録: {s.log}")
    if missing:
        print("\n揃えられなかったもの（走らせていません）")
        for m in missing:
            print(f"  ✗ {m}")
    print("\nここから届かないもの ── 実機で見てもらうしかないもの")
    for what, how in UNREACHED:
        print(f"  ・{pad(what, 24)} {how}")
    print("=" * 72)
    return 1 if (bad or late or missing) else 0


if __name__ == "__main__":
    raise SystemExit(main())
