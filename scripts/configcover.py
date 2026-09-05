#!/usr/bin/env python3
"""init.lua に書いた設定のうち、窓版が実際に読んでいるのはどれか。

**この家でいちばんよく出るバグに、物差しだけが無かった。**

「設定を直したのに効かない」は cian の最頻のバグで、renderer.js には
その顛末が書いてある ── *the window was ignoring seventeen of the twenty
settings cian-tui reads — silently, which is the worst way to ignore a config*。
17 個は直った。**そして 2026-09-06 に、同じものが3つ残っていた**
（`tab_width` `editor` `home`）。エンジンは送っていて、窓版は一度も読んで
いなかった。

黙って落ちるので、動かして気づくことはない ── `cargo test` も `drive.js` も
`audit.py` も、**送られてきた値を誰も読まないこと**は見ていない。だから数える。

    python3 scripts/configcover.py          # 被覆
    python3 scripts/configcover.py --list   # 読まれていないものの一覧

## 何と何を比べているか

* **送る側** ── `crates/cian-server/src/main.rs` の `"cfg": { … }`。窓版が
  設定について知れることの全部で、ここに無いものは窓版には届かない
* **読む側** ── `gui/renderer.js` の中の `c.<名前>`（`const c = s.cfg` の
  `c`）と、`cfg.<名前>`（menu の出し分けに使う保存先）

**下限を書いてある。** `keycover.py` は窓版を `tr()` に通した回に 72 → 2 種へ
落ちたまま百分率を出し続けた。数を数える検査には下限が要る ── 送る側の名前が
1つも拾えなくなったら、それは「全部読まれている」ではなく「読めていない」。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ENGINE = ROOT / "crates" / "cian-server" / "src" / "main.rs"
RENDERER = ROOT / "gui" / "renderer.js"

# **窓版が読まなくて正しいもの。**
#
# 免除は「窓が読み忘れている」ではなく「**エンジンが自分で使う**」という判断。
# だから**裏を取る** ── ここに名前を書くだけでは通らず、エンジンの中に
# `options.<名前>` が実在しなければ検査が落ちる。免除の一覧が、直さない
# ための置き場になるのを防ぐ唯一の方法。
ENGINE_SIDE = {
    "shell": "どのシェルを起こすかはエンジンが決める",
    "transfer_limit": "SFTP の絞りはエンジンの中で掛かる。窓は速度を知らなくてよい",
    "home": "引数が無いときに開く場所を、エンジンが `main()` で決める（端末版の `default_home` と同じ順）",
    "editor": "外部エディタを起こすのはエンジン（`editexternal` → `cian_core::editor::resolve`）",
}

# 送る側の下限。`"cfg": { … }` から名前が拾えなくなったら、検査ではなく
# 読み取りが壊れている。
FLOOR = 12


def cfg_block() -> tuple[int, int]:
    """`"cfg": { … }` の範囲。括弧の対応で切り出す。

    行数で切ると、間に1行入っただけで静かに減る。
    """
    src = ENGINE.read_text(encoding="utf-8")
    at = src.index('"cfg": {')
    depth = 0
    for i in range(at + len('"cfg": '), len(src)):
        if src[i] == "{":
            depth += 1
        elif src[i] == "}":
            depth -= 1
            if depth == 0:
                return at, i
    raise SystemExit(f"！ {ENGINE} の \"cfg\" の括弧が閉じていません")


def sends() -> list[str]:
    """`"cfg": { … }` が窓版へ渡している設定の名前。"""
    src = ENGINE.read_text(encoding="utf-8")
    at, end = cfg_block()
    body = src[at:end]
    # ネストした中身（`note_roots` の内側など）まで拾わないよう、深さ1だけ見る。
    names, depth = [], 0
    for m in re.finditer(r'[{}]|"([a-z_]+)":', body):
        if m.group(0) == "{":
            depth += 1
        elif m.group(0) == "}":
            depth -= 1
        elif depth == 1:
            names.append(m.group(1))
    return names


def engine_uses() -> set[str]:
    """エンジンが自分で読んでいる設定の名前（`options.<名前>`）。

    免除の裏を取るためだけにある。**書いたつもりで使っていない**免除は、
    読み忘れと同じ静けさで通ってしまう。
    """
    src = ENGINE.read_text(encoding="utf-8")
    # **`"cfg"` の中は数えない。** そこは `"lang": cfg.options.lang` のように
    # 窓へ**渡している**だけで、エンジンが使っている証拠にはならない ──
    # 数えていた最初の版は、送っている名前なら何でも免除を通した（`lang` を
    # でっちあげて確かめた）。
    at, end = cfg_block()
    outside = src[:at] + src[end:]
    # 改行をまたぐ書き方も拾う（`cian_lua::load()\n    .options\n    .home`）。
    # rustfmt が折り返しただけで免除が「使っていない」に化けたことがある。
    return set(re.findall(r"\boptions\s*\.\s*([a-z_]+)\b", outside))


def reads() -> set[str]:
    """窓版が実際に触っている名前。

    `c.foo`（`const c = s.cfg` を受けた側）と `cfg.foo`（保存しておく側）の
    両方。**綴りで拾う** ── 使われ方まで見ようとすると、この検査そのものが
    信用できない大きさになる。
    """
    src = RENDERER.read_text(encoding="utf-8")
    return set(re.findall(r"\bc\.([a-z_]+)\b", src)) | set(
        re.findall(r"\bcfg\.([a-z_]+)\b", src)
    )


def main() -> int:
    listing = "--list" in sys.argv
    sent = sends()
    if len(sent) < FLOOR:
        print(f"！ 送る側から {len(sent)} 個しか拾えませんでした（下限 {FLOOR}）")
        print(f"   {ENGINE} の \"cfg\" の書き方が変わっていませんか")
        return 1
    seen = reads()

    # 免除の裏取り。ここが落ちたら、直すのは検査ではなく免除の一覧のほう。
    used = engine_uses()
    unbacked = [n for n in ENGINE_SIDE if n not in used]
    if unbacked:
        print(f"！ 免除に書いてあるのにエンジンが使っていません: {', '.join(unbacked)}")
        print(f"   {ENGINE.name} に `options.<名前>` がありません ──"
              " 免除は「エンジンが使う」という主張なので、使っていないなら免除ではない")
        return 1

    kept, missing, waived = [], [], []
    for name in sent:
        if name in ENGINE_SIDE:
            waived.append(name)
        elif name in seen:
            kept.append(name)
        else:
            missing.append(name)

    checked = len(kept) + len(missing)
    pct = round(100 * len(kept) / checked) if checked else 0
    print("=" * 72)
    print(f"  init.lua の設定 {len(sent)} 個のうち、窓版が読んでいるもの "
          f"{len(kept)} / {checked}（{pct}%）"
          + (f" ── エンジン側で使うもの {len(waived)} 件は除く" if waived else ""))
    if missing:
        print()
        print("  **送られているのに、窓版が一度も読んでいないもの**"
              " ── 黙って落ちるので、動かしても気づけません:")
        for name in missing:
            print(f"    ✗ {name}")
        if not listing:
            print()
            print("  端末版が同じ設定で何をしているかは "
                  "`grep -rn 'options.<名前>' crates/cian-tui/src`")
    if listing:
        print()
        print("  読んでいるもの :", " ".join(kept))
        print("  免除          :", " ".join(f"{n}（{ENGINE_SIDE[n]}）" for n in waived))
    print("=" * 72)
    return 1 if missing else 0


if __name__ == "__main__":
    raise SystemExit(main())
