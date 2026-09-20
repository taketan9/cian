#!/usr/bin/env python3
"""同じ機能を、両前端が同じ名前で呼んでいるか。

`parity.py` は**画面に出る言葉**を見ます。これは**打つ名前**を見ます ──
2026-09-20 に二つ噛んだので足しました:

  * `:history` が端末版ではコミットログ、窓版ではペインの移動履歴だった。
    同じ語、別の機能。画面の言葉は同じなので `parity.py` には映らない
  * 重複検出の本名が窓で `dup`、端末で `duplicate`。`:dupli` まで打って
    Tab を押すと、前端によって当たったり外れたりする

数えるもの:

  ① 本名の食い違い   同じ機能を、両前端が別の名前で「正」としている
  ② 片方にしか無い名前  打てば動くものが、もう片方では unknown command
  ③ 別名の落差       別名がどちらかにしか無い（手が覚えた綴りが片方で死ぬ）

**①は 0 が天井です。** ②と③は「窓版にしか無い機能」「端末版にしか無い
機能」があるぶん 0 にはならないので、いまの数を天井として書き、増えたら
落とします（`i18n.py` と同じ作法）。

    python3 scripts/names.py          # 数える
    python3 scripts/names.py --list   # 一つずつ出す
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TUI = ROOT / "crates" / "cian-tui" / "src" / "commands.rs"
GUI = ROOT / "gui" / "renderer.js"
PALETTE = ROOT / "crates" / "cian-tui" / "src" / "palette.rs"
VIEWER = ROOT / "crates" / "cian-tui" / "src" / "viewer.rs"

# ②③の天井。**2026-09-21 に 0 になりました**（依頼221）── 窓版で打てる43語が
# 端末版で打てなかったのを、一覧の `:` からビューアへ回す・機能はあるのに名前が
# 無かったものに名前を付ける・本当に窓版だけのものは理由を言う、の3つで潰した。
# 0 を割るには、どちらかにしか無い名前を足すことになります。
ONLY_ONE_SIDE_CEILING = 0
ALIAS_GAP_CEILING = 0

# 同じ機能の、意図した別名。ここに書いたものは③に数えません（理由つき）。
ALIAS_OK = {
    "chat": "`:ai` の別名。窓版だけが持つが、端末版の `:ai` と同じ機能",
}


def tui_commands() -> list[list[str]]:
    """`run_command` の match の腕。各腕の名前を、書かれた順に。"""
    text = TUI.read_text(encoding="utf-8")
    start = text.index("match verb {")
    end = text.index('other => self.message = Some(format!("unknown command', start)
    body = text[start:end]
    out = []
    for line in body.splitlines():
        # 腕は12桁の字下げ。入れ子の match（`:readonly` の on/off など）は
        # もっと深いので、ここで落ちます。
        if not line.startswith(" " * 12) or line.startswith(" " * 13):
            continue
        stripped = line.strip()
        if not stripped.startswith('"'):
            continue
        head = stripped.split("=>")[0]
        names = re.findall(r'"([^"]+)"', head)
        if names:
            out.append(names)
    return out


def viewer_verbs() -> list[list[str]]:
    """ビューアの `:` が答える語。

    端末版はこれらを**一覧の `:` からも**受ける（2026-09-21、依頼221）ので、
    「打てる名前」として数えます ── `commands.rs` の腕は `VIEWER_VERBS` を
    見るガードで、名前が直接書いていないため、表のほうを読みます。
    """
    text = VIEWER.read_text(encoding="utf-8")
    m = re.search(r"pub\(crate\) const VIEWER_VERBS: &\[&\[&str\]\] = &\[(.*?)\n\];", text, re.DOTALL)
    if not m:
        return []
    return [re.findall(r'"([^"]+)"', g) for g in re.findall(r"&\[([^\]]*)\]", m.group(1))]


def gui_commands() -> list[list[str]]:
    """窓版の `buildCommands()`。`name` が本名、`alias` が別名。"""
    text = GUI.read_text(encoding="utf-8")
    start = text.index("function buildCommands()")
    end = text.index("\n}", start)
    body = text[start:end]
    out = []
    for m in re.finditer(r"\{\s*name:\s*'([^']+)'(?:,\s*alias:\s*\[([^\]]*)\])?", body):
        names = [m.group(1)]
        if m.group(2):
            names += re.findall(r"'([^']+)'", m.group(2))
        out.append(names)
    return out


def main() -> int:
    listing = "--list" in sys.argv
    tui = tui_commands() + viewer_verbs()
    gui = gui_commands()
    if len(tui) < 50 or len(gui) < 50:
        print("=" * 72)
        print(f"  数える元が読めていません ── 端末版 {len(tui)} 個 / 窓版 {len(gui)} 個")
        print("  `run_command` の match か `buildCommands()` の形が変わった可能性があります")
        print("=" * 72)
        return 1

    tui_all = {n for arm in tui for n in arm}
    gui_all = {n for arm in gui for n in arm}
    # 名前 → その機能の全別名
    tui_of = {n: arm for arm in tui for n in arm}
    gui_of = {n: arm for arm in gui for n in arm}

    # ① 本名の食い違い。両前端が知っている名前を手がかりに機能を突き合わせ、
    #    その機能の「先頭の名前」が違うものを出す。
    primary_clash = []
    seen = set()
    for name in sorted(tui_all & gui_all):
        t, g = tui_of[name], gui_of[name]
        key = (t[0], g[0])
        if t[0] != g[0] and key not in seen:
            seen.add(key)
            primary_clash.append((t[0], g[0], name))

    # ② 片方にしか無い名前
    only_tui = sorted(tui_all - gui_all)
    only_gui = sorted(gui_all - tui_all)

    # ③ 別名の落差（本名が一致している機能のなかで）
    alias_gap = []
    for name in sorted(tui_all & gui_all):
        t, g = tui_of[name], gui_of[name]
        if t[0] != g[0]:
            continue
        for extra in sorted(set(t) - set(g)):
            if extra not in ALIAS_OK:
                alias_gap.append((t[0], extra, "端末版だけ"))
        for extra in sorted(set(g) - set(t)):
            if extra not in ALIAS_OK:
                alias_gap.append((t[0], extra, "窓版だけ"))
    alias_gap = sorted(set(alias_gap))

    print("=" * 72)
    print(f"  打つ名前 ── 端末版 {len(tui_all)} 個 / 窓版 {len(gui_all)} 個")
    print("=" * 72)
    print(f"  ① 本名の食い違い   {len(primary_clash)} 件（天井 0）")
    print(f"  ② 片方にしか無い   端末版だけ {len(only_tui)} / 窓版だけ {len(only_gui)}"
          f"（合計の天井 {ONLY_ONE_SIDE_CEILING}）")
    print(f"  ③ 別名の落差       {len(alias_gap)} 件（天井 {ALIAS_GAP_CEILING}）")

    if listing or primary_clash:
        print()
        print("  ① 同じ機能を、別の名前で「正」としているもの")
        for t, g, via in primary_clash:
            print(f"    端末版 :{t}   ↔   窓版 :{g}      （どちらも :{via} で届く）")
    if listing:
        print()
        print("  ② 端末版にしか無い名前")
        print("    " + "  ".join(f":{n}" for n in only_tui))
        print()
        print("  ② 窓版にしか無い名前")
        print("    " + "  ".join(f":{n}" for n in only_gui))
        print()
        print("  ③ 別名の落差")
        for primary, extra, side in alias_gap:
            print(f"    :{primary} の別名 :{extra} は {side}")

    bad = len(primary_clash) > 0
    if len(only_tui) + len(only_gui) > ONLY_ONE_SIDE_CEILING:
        print(f"\n  ✗ ② が増えました（{len(only_tui) + len(only_gui)} > {ONLY_ONE_SIDE_CEILING}）")
        bad = True
    if len(alias_gap) > ALIAS_GAP_CEILING:
        print(f"\n  ✗ ③ が増えました（{len(alias_gap)} > {ALIAS_GAP_CEILING}）")
        bad = True
    print("=" * 72)
    if bad:
        print("  同じ機能は、同じ名前で呼べるようにしてください")
    else:
        print("  同じ機能を、両前端が同じ名前で呼んでいます")
    print("=" * 72)
    return 1 if bad else 0


if __name__ == "__main__":
    raise SystemExit(main())
