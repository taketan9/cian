#!/usr/bin/env python3
"""同梱する vim を落として、要るものだけ残す。

**会社の Windows には vim が無く、入れることもできない。** `:vim` が
「PATH にありません」としか言えなかったのがそこで、cian の隣に置いた vim を
使えるようにした（`cian-core::editor::beside_exe`）。これはその vim を
資材に積むための道具だ。

`cian-src-win.zip` と `cian-source-offline.zip` の両方がこれを呼ぶ。
**pwsh と bash で二度書かない** ── 片方を直したときに割れる。

## 何を残して、何を落とすか

配布元の zip は gvim 込みで 20.6MB ある。cian が起動するのは**端末版の
vim** だけで、GUI 版も右クリック拡張も要らない。`:help` と翻訳メッセージと
綴り辞書も落とす ── 3つで 25MB あり、**無くても vim は動く**（`:help` が
「見つかりません」と言うだけ）。残るのは 8.3MB。

落としたものが要るとわかったら、ここに足して数字を言い直す。**黙って重く
するのも、黙って軽くするのも、同じだけ悪い**（フォントが抜けた 6MB 軽い zip
が黙って出たことがある）。

## 版を上げるとき

`VERSION` を書き換える。**ここでしか決めていない** ── 展開先の階の名前
（`vim92` など）は zip の中身そのままで、cian 側は名前で決め打ちせずに
探している（`editor::vim_in`）ので、数字が上がっても実装は触らなくていい。
"""

import io
import os
import shutil
import sys
import urllib.request
import zipfile

# 配布元 https://github.com/vim/vim-win32-installer/releases
# **上げるのは意図してやること。** 黙って追従すると、社内に配った vim が
# ある日入れ替わる。
VERSION = "9.2.1122"
URL = (
    "https://github.com/vim/vim-win32-installer/releases/download"
    f"/v{VERSION}/gvim_{VERSION}_x64.zip"
)

# 実行に要るもの。`vim64.dll` が本体で、`vim.exe` はその入口。
# `xxd` と `diff` は vim が自分で呼ぶ（`:%!xxd`、`vimdiff`）。
BINARIES = [
    "vim.exe",
    "vim64.dll",
    "libiconv-2.dll",
    "libintl-8.dll",
    "libsodium.dll",
    "xxd.exe",
    "diff.exe",
]

# ランタイム。**構文色分けとインデントはここにある** ── 落とすと vim は
# 動くが、白黒のメモ帳になる。vimmer に渡すものとしては意味が無い。
RUNTIME_DIRS = [
    "syntax",
    "ftplugin",
    "indent",
    "autoload",
    "colors",
    "plugin",
    "compiler",
    "keymap",
]
RUNTIME_FILES = [
    "defaults.vim",
    "filetype.vim",
    "ftplugin.vim",
    "ftplugof.vim",
    "indent.vim",
    "menu.vim",
    "scripts.vim",
    "synmenu.vim",
    "optwin.vim",
    "rgb.txt",
    # **ライセンスは落とさない。** vim は charityware で、再配布するなら
    # これを一緒に運ぶ。要る・要らないの判断をする場所ではない。
    "LICENSE.txt",
]


def wanted(rel: str) -> bool:
    """zip の中の `vim/vim92/…` という道のうち、残すもの。"""
    parts = rel.split("/")
    # `vim/<版>/…` の形以外は捨てる（zip の根に置かれた install.exe など）
    if len(parts) < 3 or parts[0] != "vim":
        return False
    tail = parts[2:]
    if len(tail) == 1:
        return tail[0] in BINARIES or tail[0] in RUNTIME_FILES
    return tail[0] in RUNTIME_DIRS


def main() -> int:
    out = sys.argv[1] if len(sys.argv) > 1 else "vim"
    print(f"vim {VERSION} を落とします")
    with urllib.request.urlopen(URL) as r:
        blob = r.read()
    print(f"  取得 {len(blob) / 1024 / 1024:.1f} MB")

    if os.path.isdir(out):
        shutil.rmtree(out)

    kept = 0
    total = 0
    with zipfile.ZipFile(io.BytesIO(blob)) as z:
        for info in z.infolist():
            if info.is_dir() or not wanted(info.filename):
                continue
            # `vim/vim92/syntax/c.vim` → `<out>/vim92/syntax/c.vim`
            rel = info.filename.split("/", 1)[1]
            dest = os.path.join(out, *rel.split("/"))
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            with z.open(info) as src, open(dest, "wb") as dst:
                shutil.copyfileobj(src, dst)
            kept += 1
            total += info.file_size

    # **出口で検証する。** 落とし損ねても続くと、vim の入っていない zip が
    # 黙って出る ── 気づくのは向こうの机の上になる。
    exe = None
    for root, _, files in os.walk(out):
        if "vim.exe" in files:
            exe = os.path.join(root, "vim.exe")
            break
    if exe is None:
        print("vim.exe が残っていません ── 配布元の形が変わった可能性があります")
        return 1
    for name in RUNTIME_DIRS:
        if not os.path.isdir(os.path.join(os.path.dirname(exe), name)):
            print(f"{name}/ が残っていません ── 色分けの無い vim になります")
            return 1

    print(f"  残した {kept} ファイル / {total / 1024 / 1024:.1f} MB")
    print(f"  {os.path.relpath(exe)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
