#!/usr/bin/env python3
"""設定画面が、cian の設定の**どこまで**を触れているか。

本人の心配（2026-09-11）:「実装とずれた設定画面になっていないか心配だ」。
もっともで、`ssh.lua` の行き先を間違えたのはその日のうちだった。

**画面に出ないものは、無いことにされる。** `Options` の 20 個と `cian.ai{}` と
SSH ホストは触れるが、cian が読む設定はそれだけではない ── キー割当も
ブックマークもマクロもテーマもある。画面がそれらに一言も触れなければ、
「設定画面に無い＝設定できない」と読まれる。

    python3 scripts/settingscover.py          # 被覆
    python3 scripts/settingscover.py --list   # 一つずつ

## 何と何を比べているか

* **面 A ── `cian.*` の API**（`cian-lua/src/lib.rs` の `cian.set(…)`）。
  init.lua に書けることの全部
* **面 B ── `:where` が挙げるファイル**（`cian-tui/src/actions.rs`）。cian が
  読む設定ファイルの全部

どちらも「画面が触る」か「画面が**どこで直すか**を出している」
（`settings_schema::elsewhere`）のどちらかであること。片方も無いものは、
**画面から見えない設定**として挙げる。

**免除には理由を書く。** `configcover.py` と同じで、消すのではなく
「なぜ画面に出さないのか」を残す。

## 下限

`keycover.py` が 72 → 2 種に落ちたまま百分率を出し続けた形を避ける。
数える元が読めなくなったら、それは「全部触れている」ではなく「読めていない」。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LUA = ROOT / "crates" / "cian-lua" / "src" / "lib.rs"
SCHEMA = ROOT / "crates" / "cian-lua" / "src" / "settings_schema.rs"
WHERE = ROOT / "crates" / "cian-tui" / "src" / "actions.rs"
RENDERER = ROOT / "gui" / "renderer.js"
SERVER = ROOT / "crates" / "cian-server" / "src" / "main.rs"

# 画面に出さない API と、その理由。**理由が要ります。**
API_OK = {
    "set_option": "画面の本体。20 個ぜんぶ出ている",
    "ai": "AI の節で出ている",
    "ssh": "SSH の節で出ている",
    # 出さないもの ── どれも「画面より良い道がある」か「形が畳めない」。
    "set_theme": "`:theme` のギャラリーで選ぶ。21 個を一覧から選ぶのは画面の仕事ではない",
    "set_keymap": "キー割当。押して決めるものなので、欄に綴りを打たせる形が合わない",
    "snippets": "まだ無い。同じ依頼に入っていて、次の仕事",
    "font": "`cian.font{ face }` は窓版だけ。書体の一覧を出す道がまだ無い",
    "ime": "IME の切替は外部コマンドを呼ぶ設定で、動くかどうかは機械による（`:ime` で試す）",
    "sharepoint": "会社ごとの対応表。行が長く、画面より init.lua が読みやすい",
    "open": "拡張子ごとの開き方。同上",
    "on_open": "Lua の関数そのもの。欄には入らない",
    "ai_context": "AI に渡す文脈。長い文章なので init.lua で",
    "spawn": "設定ではなく、init.lua から外部コマンドを起こすための道具",
}

# 画面に出さないファイルと、その理由。
FILE_OK = {
    "init.lua": "画面の本体",
    "ssh.lua": "SSH の節が読み書きする",
}


def api_names() -> list[str]:
    text = LUA.read_text(encoding="utf-8")
    out: set[str] = set()
    for m in re.finditer(r"cian\.set\(\s*\n?\s*\"([a-z_]+)\"", text):
        out.add(m.group(1))
    return sorted(out)


def where_files() -> list[str]:
    """`:where` が挙げるファイル ── cian が読む設定ファイルの全部。"""
    text = WHERE.read_text(encoding="utf-8")
    m = re.search(r'for name in \[([^\]]*"state\.toml"[^\]]*)\]', text)
    if not m:
        return []
    return re.findall(r'"([a-z_]+\.(?:lua|toml))"', m.group(1))


def named_in_screen() -> set[str]:
    """画面が「どこで直すか」を出しているファイル（`elsewhere` の表）。"""
    text = SCHEMA.read_text(encoding="utf-8")
    return set(re.findall(r'file:\s*"([a-z_]+\.(?:lua|toml))"', text))


def screen_touches() -> set[str]:
    """画面が実際に読み書きする API。エンジンの窓口から数える。"""
    text = SERVER.read_text(encoding="utf-8")
    out = set()
    if "settings_read" in text and "settings_schema::fields" in text:
        out.add("set_option")
    if 'get_field_in(&text, "ai"' in text or 'set_field_in(&out, "ai"' in text:
        out.add("ai")
    if "settings_ssh::set_host_in" in text or "settings_hosts_read" in text:
        out.add("ssh")
    return out


def main() -> int:
    listing = "--list" in sys.argv
    api = api_names()
    files = where_files()
    touched = screen_touches()
    named = named_in_screen()

    # **下限。** 読めなくなったら「全部触れている」ではなく「読めていない」。
    if len(api) < 10 or len(files) < 5 or not touched:
        print("=" * 72)
        print(f"  数える元が読めていません ── API {len(api)} 個 / ファイル {len(files)} 個"
              f" / 画面が触る {len(touched)} 個")
        print("  `cian.set(` の書き方か `:where` の一覧が変わった可能性があります")
        print("=" * 72)
        return 1

    api_gap = [a for a in api if a not in touched and a not in API_OK]
    file_gap = [f for f in files if f not in named and f not in FILE_OK]
    # 理由を書いたのに、画面も触っていて、`elsewhere` にも出ていないもの ──
    # 免除が古くなっている合図。
    stale = [a for a in API_OK if a not in api]

    print("=" * 72)
    print(f"  設定画面の被覆 ── API {len(touched)}/{len(api)} 個を直接触り、"
          f"ファイル {len(named)}/{len(files)} 個の行き先を出しています")
    print("=" * 72)
    if listing:
        print()
        print("  画面が直接触る   : " + " ".join(sorted(touched)))
        print("  画面が場所を出す : " + " ".join(sorted(named)))
        print()
        for a in api:
            if a in touched:
                continue
            print(f"    cian.{a:<12} {API_OK.get(a, '**理由が書かれていません**')}")
    bad = False
    if api_gap:
        bad = True
        print()
        print(f"  ✗ 画面からも見えず、理由も書かれていない API {len(api_gap)} 個:")
        for a in api_gap:
            print(f"      cian.{a}")
        print("    触るか、`API_OK` に**なぜ出さないのか**を書いてください")
    if file_gap:
        bad = True
        print()
        print(f"  ✗ 画面がどこで直すかを言っていない設定ファイル {len(file_gap)} 個:")
        for f in file_gap:
            print(f"      {f}")
        print("    `settings_schema::elsewhere` に足すか、`FILE_OK` に理由を")
    if stale:
        bad = True
        print()
        print(f"  ✗ 免除が古くなっています（もう API にありません）: {' '.join(stale)}")
    print("=" * 72)
    if bad:
        return 1
    print("  cian が読む設定は、画面が触るか、どこで直すかを画面が言っています")
    print("=" * 72)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
