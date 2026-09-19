## どれを落とせばいい？

**資材は3点とチェックサムだけです**（2026-09-20 に13点から絞りました）。物差しは
**「その機械で作れないものだけ置く」**です。

| 使いたいもの | 落とすもの |
|---|---|
| **Windows で窓版を使いたい** | **`cian-src-win.zip`**（10MB）と **`cian-server-win-x64.exe.zip`**（3MB）の2つ。展開して `node gui\pack.js` を1回叩けば `cian.exe` ができます。**`npm install` も Rust も要りません。** 手順は同梱の `packaging\windows\SRC.ja.txt` |
| エンジンだけ差し替えたい | `cian-server-win-x64.exe.zip` だけ。前面は JavaScript なので、更新はたいていこれ1つで足ります |
| **端末で使いたい（端末版）** | **`cian-tui-win-x64.exe.zip`**（5MB）── 一本の実行ファイルだけ。Electron も書体も要りません。お使いの端末（WezTerm・Windows Terminal）で走らせます |
| 壊れずに届いたかの確認 | `SHA256SUMS` ── `sha256sum -c SHA256SUMS` / `Get-FileHash` |
| エンジンごとソースから作りたい | `cian-source-offline.zip`（約180MB）── 毎回は出していません。Actions から `release` を **everything = true** で手動実行すると並びます |

**組み立ては1行です。** 持ち込むのはこの2つ（13MB）だけ。Electron は社内にある
ものを使います。

```
cd C:\work\cian-src-win
node gui\pack.js --out dist --platform win32 ^
  --electron C:\electron\electron-v38.0.0-win32-x64 ^
  --engine C:\落とした場所\cian-server-win-x64.exe --zip
```

`dist\cian\cian.exe` ができます。**配るのは `--zip` が並べる
`dist\cian-win-x64.zip`** のほうで、受け取った人は展開して `cian.exe` を
ダブルクリックするだけです（Electron ごと入っています）。

**エンジンそのものは社内では作れません** ── Rust と C コンパイラが要るので、
そこは落としたものを渡してください。`--engine` は落とした名前のままで構いません。

### 何を外したか（2026-09-20）

以前は13点ありました。外したのは、**この道で作れるもの**と、**誰も取らないもの**です。

- `cian-win-x64.zip`（123MB）── CI が焼いていた同梱版。社内で `pack.js` が同じ
  ものを作るので、123MB を運ぶ理由が無くなりました
- `cian-gui-win-x64.zip`（14MB）── 前面だけの zip。`cian-src-win.zip` が同じ中身を
  **リポジトリの形**で持っています（`pack.js` はその形でしか叩けません）
- 裸の `.exe` 2つ ── **zip が通る網は裸の exe も通りますが、逆は通りません。**
  実際に `.exe` の直接ダウンロードを止められた網があるので、残すなら zip です
- **Mac の5点** ── ビルド自体は今も毎回走っていて、成果物（Actions）から取れます。
  リリースには付けません

```
gh run download <run-id> -n cian-gui-macos     # 窓版一式（エンジン同梱）
gh run download <run-id> -n cian-tui-macos     # 端末版
gh run download <run-id> -n cian-server-macos  # エンジンだけ
```

### 出してしまった版に、後から資材を足すには

`python3 scripts/release-asset.py <タグ> --zip <資材名>` を使ってください。手では
やらないこと。公開済みのバイト列を落として包み、取り出して一致を確かめ、
`SHA256SUMS` を継ぎ足し、上げ直してから**もう一度落として照合**します。手でやると、
この最後の一手を飛ばしても成功したように見えます。

### Windows で SmartScreen が出たら

「詳細情報」→「実行」で通ります。署名を買っていないので、知らない発行元として
止められることがあります（**実機の管理端末では出ずに動いています**、2026-09-14）。

出ないようにしたいなら、展開の仕方を変えてください。エクスプローラの
「すべて展開」は落としてきた印を中の全ファイルに写しますが、`tar` は写しません:

```
tar -xf cian-src-win.zip -C C:\work
```

zip のプロパティで「許可する」→ それから展開、でも同じです。**どちらも必須では
ありません。**

### Mac で成果物を落としたら

macOS は落としたものに「隔離」の印を付けます。cian は公証していないので、
展開したフォルダで一度だけ:

```
xattr -dr com.apple.quarantine .
```

詳しい手順は zip の中の `GUI.ja.txt` にあります。

---
