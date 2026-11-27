# 機能から引く地図

この文書は「この機能を直したい。どこから読めばよいか」を引くための案内です。

- **開発の約束**（破ると何が起きるか、どの検査が確認するか）は
  [`CLAUDE.md`](../CLAUDE.md) にあります
- **Electron 版とエンジンの受け渡し**は `engine-protocol.ja.md`（未作成）に書く予定です。
  いまは `crates/cian-server/src/main.rs` の `match` の腕を読んでください
- **なぜそう決めたか**は、コードの中のコメントにあります。この文書は場所だけを示します

機能ごとに、**入口 → 中心 → 守り → テスト**の順に並べています。行番号は書きません。

> **注意**
> 現在この文書は4つの機能だけを収めています。残りは順に書き足します。
> 収めていない機能は、`crates/cian-server/src/main.rs` の要求名から
> 中心の関数を辿ってください。

## 層の並び

どの機能も層の並びは同じです。端末版と Electron 版で前半が違い、後半は共通です。

| 前端 | 並び |
| --- | --- |
| 端末版（正） | 画面 `crates/cian-tui/src/render.rs` → 操作 `keys.rs` / `actions.rs` → 純ロジック `cian-core` |
| Electron 版 | 画面 `gui/index.html` と `gui/renderer.js` → エンジン `crates/cian-server/src/main.rs` → 純ロジック `cian-core` |

**`cian-core` は入出力と画面に依存しません。** 両方の前端が同じ判断を使うためです。
端末版から `cian-core` への依存は一方向に保ってください。

## シェル（内蔵の端末）

| 段 | どこ |
| --- | --- |
| 入口 | 端末版は `Shift+J`、Electron 版も `Shift+J`（`gui/renderer.js` の `openShell()`） |
| 中心 | 疑似端末は `crates/cian-pty`。端末版の持ち主は `crates/cian-tui/src/panes.rs` の `active_session()`、エンジンの持ち主は `crates/cian-server/src/shell.rs` |
| コマンドの打ち込み | 端末版は `run_in_shell()`、エンジンは `write_command()`。**打ち込む道は必ずこの2つを通してください** |
| 守り | コマンドを打ち込む前に、プロンプトの入力途中の文字を消します（`cian_core::shellwhere::line_clear()`）。消す操作はシェルによって違うため、判定を1か所に集めています |
| 生のバイト列 | 貼り付けと `Ctrl+C` は `shellinput` を使い、入力途中の文字を消しません。行を置き換える操作ではないためです |
| テスト | `the_line_is_cleared_with_what_that_shell_understands`、`the_far_shell_wins_over_the_one_cian_started`、`scripts/tui-drive.py` の ⑨ |

**入力途中の文字を消さないと、残った文字と繋がって別のコマンドになります。**
`clear` と入力して Enter を押さないまま「SSH接続」を選択すると、シェルは
`clearssh user@host` を受け取って実行しました。

## リモートペイン（SFTP でサーバーを参照する）

| 段 | どこ |
| --- | --- |
| 入口 | `Shift+S`（接続先の一覧）、`:remote`、`:ssh`、右クリックの「SSH接続」 |
| 中心 | 通信は `crates/cian-scp`。端末版は `crates/cian-tui/src/ssh.rs` の `ssh_connect()`、Electron 版は `gui/renderer.js` の `cmdSshPicker()` と `remoteStep()` |
| エンジンの要求 | `connect` / `disconnect` / `remotelist` / `remoteop` / `remoteview` / `sshshell` / `sshhosts` |
| 接続先の設定 | `init.lua` の `cian.ssh{}`。`ssh.lua` があればそちらが正本です。書き込み先は設定画面に表示されます |
| 表示 | ペイン上部はサーバーのパスを表示します。`PaneView.remote_path` を使ってください。**`cwd` は接続前のローカルのパスです** |
| 守り | パスワードは保存しません。要求のたびに入力を求め、接続までの間だけ保持します |
| テスト | `a_remote_pane_wears_carmine_even_when_it_has_the_keys`、`enter_on_a_remote_directory_lists_it`、`a_remote_side_is_refused_before_anything_is_fetched` |

## 書庫（zip と tar を参照・作成する）

| 段 | どこ |
| --- | --- |
| 入口 | 一覧の行で `Enter`（中に入ります）、`:unzip` / `:extract` / `:untar`（展開）、`:zip` / `:tar` / `:targz`（作成）、右クリックの「圧縮」 |
| 中心 | `crates/cian-core/src/archive.rs` に全部あります。判定は `kind()`、一覧は `list()`、展開は `extract()`、作成は `create_zip()` と `create_tar()`、中身の変更は `zip_modify()` と `tar_modify()` |
| 対応する形式 | `.zip` `.jar` `.xpi` `.whl` `.epub` `.tar` `.tar.gz` `.tgz` の8つです。`.gz` 単体・`.bz2` `.xz` `.zst` `.7z` `.rar` は対応していません |
| エンジンの要求 | `archivelist` / `archiveview` / `archiveedit` / `archivesave` / `enterarchive` / `extract` / `compress` / `zipadd` |
| 守り | 展開先の外を指す項目は作成しません（`safe_join()`）。リンクの行き先にも同じ判定をかけます。暗号化された zip は中身を変更しません |
| テスト | `archive::` の19件すべて。`a_link_in_a_tar_comes_out_as_a_link`、`member_paths_cannot_escape_the_destination`、`creates_tar_and_tar_gz_that_round_trip` |

> **補足**
> 一覧のアイコンと色は `.gz` `.7z` `.rar` `.bz2` `.xz` `.zst` も書庫として表示します。
> 対応する形式と一致していないため、利用者には「書庫に見えるのに開かない」状態です。

## AI（設定したときだけ有効）

| 段 | どこ |
| --- | --- |
| 入口 | `:ai`（質問）、`:aicmd`（説明からコマンドを作成）、右クリックの「AI - simple」 |
| 有効の判定 | `init.lua` の `endpoint` が空のときは無効です。設定していない状態が初期設定です |
| 中心 | 前置きの作成は `crates/cian-core/src/aiprompt.rs`、接続先の判定は `crates/cian-core/src/shellwhere.rs` の `describe()`、履歴は `crates/cian-core/src/chatlog.rs` |
| エンジンの要求 | `ai` / `aicommit` / `aierror` / `aihistory` |
| 端末版の入口 | `crates/cian-tui/src/ai.rs` の `start_ai_shell_prompt()` が入力欄を表示し、`start_ai_shell_cmd()` が要求を送ります |
| Electron 版の入口 | `gui/renderer.js` の `promptAiCmd()` が入力欄を表示し、`cmdAiCmd()` が要求を送ります |
| 守り | 作成したコマンドは確認のうえプロンプトに置くだけで、実行しません。質問は入力欄に書き込むだけで、送信は利用者が Enter を押下したときです |
| テスト | `ai_chat_round_trips_a_mock_reply`、`clean_ai_command_strips_fences_and_prose`、`ai_context_facts_are_folded_into_prompts` |

**`:aicmd` は引数が必須です。** `:ai` は引数を省略できますが、`:aicmd` は
やりたいことの説明が無いとエンジンが要求を断ります。メニューから選択したときは、
先に入力欄で説明を受け取ってください。
