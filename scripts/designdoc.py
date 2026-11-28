#!/usr/bin/env python3
"""設計書（`docs/*.ja.md`）が名乗る名前は、実在するか。

crmaine の `TestDesignRulesPointAtRealThings` / `TestFeatureMapPointsAtRealCode`
と同じ役目です。設計書は手で書くので、関数名を変えた日に誤った記述のまま残ります。
**誤った記述になった瞬間に失敗するのが、この検査の仕事です。**

    python3 scripts/designdoc.py          通る／落ちる
    python3 scripts/designdoc.py --list   何を見たか

見るもの:

  ① `名前()` と書いた関数が、`crates/` か `gui/` か `scripts/` に実在すること
  ② `パス` と書いたファイル・ディレクトリが実在すること
  ③ 文書どうしのリンク先が実在すること
  ④ **数えた件数が下限を下回らないこと** ── 取りこぼして「所見なし」になるのを
     防ぐためです（`keycover.py` が対象を取り落として2種しか見ないまま百分率を
     出し続けた型）

③ の下限は、文書を1枚増やしたときに上げてください。
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"

# 下限 ── 取りこぼしたときに取りこぼしを報告するため
LEAST_FUNCS = 20
LEAST_PATHS = 12

# 関数として数えないもの（型・マクロ・設定の表記）
NOT_FUNCS = {
    "cian.ssh{}",
    "cian.font{}",
}

# 名前を探す先
HAYSTACK = ["crates", "gui", "scripts"]


def docs() -> list[Path]:
    if not DOCS.is_dir():
        return []
    return sorted(p for p in DOCS.glob("*.ja.md"))


def inline(text: str) -> list[str]:
    """バックティックの中身を全部。"""
    return re.findall(r"`([^`\n]+)`", text)


def looks_like_func(tok: str) -> bool:
    return bool(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_:.]*\(\)", tok)) and tok not in NOT_FUNCS


def looks_like_path(tok: str) -> bool:
    """`/` を含むものだけをパスとして見ます。

    **素のファイル名は場所を名乗っていません。** `keys.rs` は文中で短く呼ぶ
    ための書き方で、`init.lua` は利用者が作る設定なのでリポジトリにありません。
    どちらも「ここに在る」と言っていないので、実在を確認しません。場所を
    名乗るなら `crates/cian-tui/src/keys.rs` の形で書いてください。
    """
    if tok.startswith(("http://", "https://")) or "/" not in tok:
        return False
    return bool(re.fullmatch(r"[\w./-]+", tok))


def func_exists(name: str, cache: dict[str, bool]) -> bool:
    """`fn 名前(` / `function 名前(` / `名前(` のどれかで実在するか。"""
    bare = name[:-2].rsplit(":", 1)[-1].rsplit(".", 1)[-1]
    if bare in cache:
        return cache[bare]
    pattern = rf"(fn|function|const|let)\s+{re.escape(bare)}\b|\b{re.escape(bare)}\s*\("
    hit = subprocess.run(
        ["grep", "-rqE", "--exclude-dir=node_modules", "--exclude-dir=vendor",
         "--exclude-dir=target", pattern, *HAYSTACK],
        cwd=ROOT,
        capture_output=True,
    ).returncode == 0
    cache[bare] = hit
    return hit


def main() -> int:
    show = "--list" in sys.argv
    found = docs()
    if not found:
        print("docs/ に *.ja.md がありません")
        return 1

    bad: list[str] = []
    funcs = paths = links = 0
    cache: dict[str, bool] = {}

    for doc in found:
        text = doc.read_text(encoding="utf-8")
        for tok in inline(text):
            if looks_like_func(tok):
                funcs += 1
                if not func_exists(tok, cache):
                    bad.append(f"{doc.name}: 実在しない関数 `{tok}`")
                elif show:
                    print(f"  関数 {doc.name}: {tok}")
            elif looks_like_path(tok):
                paths += 1
                at = ROOT / tok
                if not at.exists():
                    bad.append(f"{doc.name}: 実在しないパス `{tok}`")
                elif show:
                    print(f"  パス {doc.name}: {tok}")
        # ③ 文書どうしのリンク
        for target in re.findall(r"\]\(([^)#]+)\)", text):
            if target.startswith(("http://", "https://")):
                continue
            links += 1
            if not (doc.parent / target).resolve().exists():
                bad.append(f"{doc.name}: リンク先がありません ({target})")
            elif show:
                print(f"  リンク {doc.name}: {target}")

    print(f"設計書 {len(found)} 枚 ── 関数 {funcs} 件・パス {paths} 件・リンク {links} 件")
    if funcs < LEAST_FUNCS:
        bad.append(f"数えた関数が {funcs} 件しかありません（下限 {LEAST_FUNCS}）。取りこぼしを疑ってください")
    if paths < LEAST_PATHS:
        bad.append(f"数えたパスが {paths} 件しかありません（下限 {LEAST_PATHS}）。取りこぼしを疑ってください")

    if bad:
        for b in bad:
            print(f"  ✗ {b}")
        return 1
    print("  設計書が名乗る名前は、すべて実在します")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
