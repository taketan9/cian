## どれを落とせばいい？

| 使いたいもの | 落とすもの |
|---|---|
| **Windows で、とにかく動かしたい** | **`cian-win-x64.zip`**（124MB）── Electron ごと入っています。展開して **`cian.exe` をダブルクリック**。用意するものはありません |
| Windows で、社内にある Electron を使いたい | `cian-gui-win-x64.zip`（14MB）── 前面だけ。`run.bat` が Electron を探します |
| **Mac で窓版（Electron）** | **`cian-gui-macos.zip`** — Electron 本体は別途必要。エンジンは Intel と Apple Silicon の両方入り |
| エンジンだけ差し替えたい | `cian-server-win-x64.exe`（10MB）／`cian-server-macos.bin` |
| **`.exe` が社内の網に止められる** | **`cian-server-win-x64.exe.zip`**（4MB）／`cian-server-macos.bin.zip` — 中身は同じものが1つだけ |
| 壊れずに届いたかの確認 | `SHA256SUMS` — `sha256sum -c SHA256SUMS` / `Get-FileHash` |

**Windows の zip は2つあって、違いは「Chromium を誰が持ってくるか」だけです。**
`cian-win-x64.zip` は中に Electron を抱えているので 124MB（展開 307MB）、
そのかわり何も探しません。`cian-gui-win-x64.zip` は 14MB で、機械に既にある
Electron を `run.bat` が探します ── 何台にも配るなら、そちらが1台 14MB です。
**前面の中身は同じものです。**

**zip 版のエンジンは、中身も名前も生のものと同じです。** 会社の運用端末で
`.exe` の直接ダウンロードが止められることがあり、実際に止まったので置いています。
展開すると `cian-server-win-x64.exe` がそのまま出てきます（フォルダは挟みません）。
落とせるなら生の `.exe` のままで構いません。crmaine の `gui/pack.js` はどちらでも
受け取ります。

**出してしまった版に、後から資材を足すには** ── `release.yml` は次の版から
自動で zip も並べますが、既に出た版には
`python3 scripts/release-asset.py <タグ> --zip <資材名>` を使ってください。
公開済みのバイト列を落として包み、取り出して一致を確かめ、`SHA256SUMS` を
継ぎ足し、上げ直してから**もう一度落として照合**します。手でやると、この
最後の一手を飛ばしても成功したように見えます。

**出しているのは窓版（Windows と Mac）とエンジンだけです。** 端末版・Mac の
`.app`・Linux の資材は、もう作っていません（2026-09-10、配布は窓版一本）。
端末版はソースからは建ちます。オフラインビルド用のソース一式が要るときは、
Actions から `release` を **everything = true** で手動実行してください。

### Windows で SmartScreen が出たら

「詳細情報」→「実行」で通ります。署名を買っていないので、知らない発行元として
止められることがあります（**実機の管理端末では出ずに動いています**、2026-09-14）。

出ないようにしたいなら、展開の仕方を変えてください。エクスプローラの
「すべて展開」は落としてきた印を中の全ファイルに写しますが、`tar` は写しません:

```
tar -xf cian-win-x64.zip -C C:\cian
```

zip のプロパティで「許可する」→ それから展開、でも同じです。**どちらも必須では
ありません。**

詳しい手順は zip の中にあります ── 同梱版は `START.ja.txt`、前面だけの方は
`GUI.txt` です。

### Mac で落としたら

macOS は落としたものに「隔離」の印を付けます。cian は公証していないので、
展開したフォルダで一度だけ:

```
xattr -dr com.apple.quarantine .
```

詳しい手順は zip の中の `GUI.ja.txt` にあります。

---
