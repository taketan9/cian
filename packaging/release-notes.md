## どれを落とせばいい？

| 使いたいもの | 落とすもの |
|---|---|
| **Windows で窓版（Electron）** | **`cian-gui-win-x64.zip`** — Electron 本体だけ別途必要 |
| **Mac で窓版（Electron）** | **`cian-gui-macos.zip`** — 同上。エンジンは Intel と Apple Silicon の両方入り |
| エンジンだけ差し替えたい | `cian-server-win-x64.exe`（10MB）／`cian-server-macos.bin` |
| **`.exe` が社内の網に止められる** | **`cian-server-win-x64.exe.zip`**（4MB）／`cian-server-macos.bin.zip` — 中身は同じものが1つだけ |
| 壊れずに届いたかの確認 | `SHA256SUMS` — `sha256sum -c SHA256SUMS` / `Get-FileHash` |

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

**いまは窓版（Windows と Mac）を出しています。** 端末版・Mac の `.app`・
Linux・オフラインビルド用のソース一式は、必要になったら Actions から
`release` を **everything = true** で手動実行すれば揃います。

### Windows で落としたら、まずブロックを外す

zip を右クリック →「プロパティ」→ 下に「セキュリティ: 他のコンピューターから
取得したものです」があれば **「許可する」にチェック** → OK。**展開する前に。**

外さないと展開後の全ファイルに印が残り、`.exe` が黙って起動しません
（窓は出るのに中身が空、という形で出ます）。

窓版の詳しい手順は zip の中の `GUI.txt` にあります。

### Mac で落としたら

macOS は落としたものに「隔離」の印を付けます。cian は公証していないので、
展開したフォルダで一度だけ:

```
xattr -dr com.apple.quarantine .
```

詳しい手順は zip の中の `GUI.ja.txt` にあります。

---
