## どれを落とせばいい？

資材は3点と `SHA256SUMS` です（2026-09-20 に13点から絞りました）。

| 使いたいもの | 落とすもの |
|---|---|
| Windows で窓版を使いたい | `cian-src-win.zip`（18MB）と `cian-server-win-x64.exe.zip`（3MB）。展開して `node gui\pack.js` を1回叩くと `cian.exe` ができます。`npm install` も Rust も要りません。手順は同梱の `packaging\windows\SRC.ja.txt` |
| エンジンだけ差し替えたい | `cian-server-win-x64.exe.zip`。前面は JavaScript なので、更新はたいていこれ1つで足ります |
| 端末で使いたい（端末版） | `cian-tui-win-x64.exe.zip`（5MB）。一本の実行ファイルで、Electron も書体も要りません。お使いの端末（WezTerm・Windows Terminal）で走らせます |
| 壊れずに届いたかの確認 | `SHA256SUMS` ── `sha256sum -c SHA256SUMS` / `Get-FileHash` |
| エンジンごとソースから作りたい | `cian-source-offline.zip`（約180MB）。毎回は出していません。Actions から `release` を everything = true で手動実行すると並びます |

### 社内での組み立て

持ち込むのはこの2つ（22MB）です。Electron は社内にあるものを使います。
`cian-src-win.zip` には vim も入っていて、組み立てると `cian.exe` の隣に並びます
（vim の入っていないパソコンでも `:vim` が使えるように）。

```
cd C:\work\cian-src-win
node gui\pack.js --out dist --platform win32 ^
  --electron C:\electron\electron-v38.0.0-win32-x64 ^
  --engine C:\落とした場所\cian-server-win-x64.exe --zip
```

`dist\cian\cian.exe` ができます。配るのは `--zip` が並べる
`dist\cian-win-x64.zip` のほうで、受け取った人は展開して `cian.exe` を
ダブルクリックします（Electron ごと入っています）。

エンジンは社内では作れません ── Rust と C コンパイラが要るので、そこは落とした
ものを渡してください。`--engine` は落とした名前のままで構いません。

### 資材の形（ほかの道具から読むとき）

crmaine はこの資材を取り寄せて組み直しています。**名前・並び・フォルダ名を
変えた版では、下の表に1行足します** ── 取り寄せる側が、止まってから気づくの
ではなく、先回りできるように。

| 資材 | 中の形 |
|---|---|
| `cian-src-win.zip` | `cian-src-win/` の下に `gui/`（画面一式、`vendor/` 込み）・`vim/`（`vim92/vim.exe` など）・`packaging/windows/`・`examples/`・`Cargo.toml` ほか |
| `cian-server-win-x64.exe.zip` | `cian-server-win-x64.exe` が1つ |
| `cian-tui-win-x64.exe.zip` | `cian-tui-win-x64.exe` が1つ |

**cian は vim を、エンジンの隣から上へ3階まで探します**（`vim/` の直下か、
`vim/<版>/` の下の `vim.exe`）。エンジンの隣に `vim/` を置けば1階目で当たります。

形が変わった版:

| 版 | 変わったこと |
|---|---|
| v3.4.3 | `cian-src-win.zip` に `vim/` が入った（10MB → 18MB） |
| v3.3.9 | 画面だけの zip（`cian-gui-win-x64.zip`）が無くなった。画面は `cian-src-win.zip` の `gui/` から。エンジンと端末版は裸の `.exe` が無くなり、`.exe.zip` だけに |

### 外したもの（2026-09-20）

以前は13点ありました。

- `cian-win-x64.zip`（123MB）── CI が焼いていた同梱版。社内で `pack.js` が同じ
  ものを作ります
- `cian-gui-win-x64.zip`（14MB）── 前面だけの zip。`cian-src-win.zip` が同じ中身を
  リポジトリの形で持っています（`pack.js` はその形でしか叩けません）
- 裸の `.exe` 2つ ── `.exe` の直接ダウンロードを止める網があり、zip なら通ります
- Mac の5点 ── ビルドは今も毎回走っていて、成果物（Actions）から取れます

```
gh run download <run-id> -n cian-gui-macos     # 窓版一式（エンジン同梱）
gh run download <run-id> -n cian-tui-macos     # 端末版
gh run download <run-id> -n cian-server-macos  # エンジンだけ
```

### 出してしまった版に、後から資材を足すには

`python3 scripts/release-asset.py <タグ> --zip <資材名>` を使ってください。手では
やらないこと。公開済みのバイト列を落として包み、取り出して一致を確かめ、
`SHA256SUMS` を継ぎ足し、上げ直してから、もう一度落として照合します。手でやると、
この最後の一手を飛ばしても成功したように見えます。

### Windows で SmartScreen が出たら

「詳細情報」→「実行」で通ります。署名を買っていないので、知らない発行元として
止められることがあります（実機の管理端末では出ずに動いています、2026-09-14）。

出ないようにしたいなら、展開の仕方を変えてください。エクスプローラの
「すべて展開」は落としてきた印を中の全ファイルに写しますが、`tar` は写しません:

```
tar -xf cian-src-win.zip -C C:\work
```

zip のプロパティで「許可する」→ それから展開、でも同じです。どちらも必須では
ありません。

### Mac で成果物を落としたら

macOS は落としたものに「隔離」の印を付けます。cian は公証していないので、
展開したフォルダで一度だけ:

```
xattr -dr com.apple.quarantine .
```

詳しい手順は zip の中の `GUI.ja.txt` にあります。

---
