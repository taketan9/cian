//! The `:` command line: dispatch (`run_command`) and every builtin — cd, mark,
//! mkdir, touch, transfer, ls, file, wc, head/tail, df, zip, and `!`-to-shell.
//! Split out of lib.rs as an `impl App` block.
use super::*;

impl App {
    /// Enter the `:` command line. From the shell this is the only keyboard way
    /// in (typing `:` there goes to the terminal), reached by Ctrl+Enter or the
    /// "Command…" menu item.
    pub(crate) fn enter_command_mode(&mut self) {
        self.command_buffer.clear();
        self.command_cursor = 0;
        self.command_hist_at = None;
        self.mode = Mode::Command;
    }

    /// `:mask *.log` — the standing filter, kept across directory changes.
    pub(crate) fn cmd_mask(&mut self, rest: &str) {
        let Some(pane) = self.active_pane_mut() else { return };
        let was = pane.mask.clone();
        if rest.trim().is_empty() {
            pane.set_mask("");
            self.message = Some(if was.is_empty() {
                tr(self.lang, "usage: :mask *.log   (no mask is set)", "使い方: :mask *.log   （いまマスクはありません）").into()
            } else if self.lang == crate::theme::Lang::Ja {
                format!("マスクを外しました（{was}）")
            } else {
                format!("mask off (was {was})")
            });
            return;
        }
        // A pattern that cannot be read is refused rather than quietly
        // matching nothing — a mask that hides everything looks exactly like
        // an empty directory.
        let spec = rest.trim().to_string();
        if cian_core::mask_matcher(&spec).is_none() {
            // 読めないパターンは断る。黙って0件にすると、空のディレクトリと
            // 見分けがつかない。
            self.message = Some(format!(
                "{}: {spec}",
                tr(self.lang, "a mask cian cannot read", "読めないマスクです")
            ));
            return;
        }
        pane.set_mask(spec.clone());
        let n = pane.entries.iter().filter(|e| !e.is_parent).count();
        self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("マスク {spec}。{n} 件")
        } else {
            format!("mask {spec}. {n} shown")
        });
    }

    /// Put `text` on the `:` line with the caret after it.
    ///
    /// **One door**, because the caret is a second fact about the same line:
    /// an assignment that forgets it leaves the caret at 0, and the next
    /// character typed lands at the *front* of what was just put there. That
    /// is exactly what happened to the paste path the day the caret arrived.
    pub(crate) fn set_command_line(&mut self, text: impl Into<String>) {
        self.command_buffer = text.into();
        self.command_cursor = self.command_buffer.chars().count();
    }

    /// ↑ and ↓ on the `:` line: the lines already run, newest first.
    ///
    /// The line being typed is kept as `command_draft`, so walking up and back
    /// down again returns what was there rather than an empty line — the
    /// thing every shell does and nobody notices until it is missing.
    pub(crate) fn walk_command_history(&mut self, step: i32) {
        if self.command_history.is_empty() {
            return;
        }
        let n = self.command_history.len();
        let at = match (self.command_hist_at, step) {
            // First step back: remember what was being typed.
            (None, -1) => {
                self.command_draft = self.command_buffer.clone();
                Some(n - 1)
            }
            (None, _) => None,
            (Some(0), -1) => Some(0),
            (Some(i), -1) => Some(i - 1),
            (Some(i), _) if i + 1 < n => Some(i + 1),
            // Past the newest: back to what was being typed.
            (Some(_), _) => None,
        };
        self.command_hist_at = at;
        self.command_buffer = match at {
            Some(i) => self.command_history[i].clone(),
            None => std::mem::take(&mut self.command_draft),
        };
        self.command_cursor = self.command_buffer.chars().count();
    }

    /// Tab on the `:` line: finish the verb, or the path being typed.
    ///
    /// **One press completes as far as every candidate agrees** (the common
    /// prefix), and says what the candidates are when more than one is left.
    /// That is the shell behaviour a hand expects, and it never picks for you
    /// — a Tab that guesses is a Tab you have to undo.
    pub(crate) fn complete_command(&mut self) {
        let head: String = self.command_buffer.chars().take(self.command_cursor).collect();
        let tail: String = self.command_buffer.chars().skip(self.command_cursor).collect();
        // The word under the caret: everything back to the last space.
        let cut = head.rfind(' ').map(|i| i + 1).unwrap_or(0);
        let (before, word) = head.split_at(cut);
        let verb_position = before.trim().is_empty();

        let mut hits: Vec<String> = if verb_position {
            crate::palette::command_list()
                .iter()
                .map(|(v, _, _)| (*v).to_string())
                .filter(|v| v.starts_with(word))
                .collect()
        } else {
            match self.cwd() {
                Some(cwd) => cian_core::ops::path_completions(&cwd, word),
                None => Vec::new(),
            }
        };
        hits.sort();
        hits.dedup();
        if hits.is_empty() {
            return;
        }
        // As far as they all agree.
        let common = hits.iter().skip(1).fold(hits[0].clone(), |acc, h| {
            let n = acc
                .chars()
                .zip(h.chars())
                .take_while(|(a, b)| a == b)
                .count();
            acc.chars().take(n).collect()
        });
        if common.chars().count() > word.chars().count() {
            self.command_buffer = format!("{before}{common}{tail}");
            self.command_cursor = before.chars().count() + common.chars().count();
        }
        if hits.len() > 1 {
            // Not a popup: the line is being typed, and something that steals
            // the screen to list eight names is worse than a line of them.
            let mut shown: Vec<String> = hits.iter().take(8).cloned().collect();
            if hits.len() > shown.len() {
                shown.push(format!("… +{}", hits.len() - shown.len()));
            }
            self.message = Some(shown.join("  "));
        }
    }


    pub(crate) fn run_command(&mut self) {
        let raw = self.command_buffer.trim().to_string();
        self.command_buffer.clear();
        self.command_cursor = 0;
        self.command_hist_at = None;
        self.mode = Mode::Normal;
        // Remembered before it runs, so a command that fails is still there
        // for ↑ — that is the one you most want back. The same line twice in
        // a row is kept once; a history that repeats is a history you have to
        // press past.
        if !raw.is_empty() && self.command_history.last().map(String::as_str) != Some(raw.as_str()) {
            self.command_history.push(raw.clone());
            // A session's worth, not a lifetime's.
            if self.command_history.len() > 200 {
                self.command_history.remove(0);
            }
        }
        if raw.is_empty() {
            return;
        }
        // `!cmd` is a shell escape: everything after the bang is the command,
        // so it is split off before tokenising (the command has its own
        // quoting, and `%`-substitution happens inside).
        if let Some(rest) = raw.strip_prefix('!') {
            self.run_bang(rest);
            return;
        }

        // Split into a verb and its arguments. Whitespace-separated is enough:
        // the commands that take a path accept it as the whole remainder, and
        // the ones that take flags take single tokens.
        let mut parts = raw.split_whitespace();
        let raw_verb = parts.next().unwrap_or("");
        let args: Vec<&str> = parts.collect();
        let rest = raw[raw_verb.len()..].trim(); // the arguments as one string
        // A verb typed with the IME on comes through in full-width — `ｑ` for
        // `q`. Verbs are ASCII by definition, so folding one costs nothing;
        // the arguments are left exactly as typed, since a path may genuinely
        // hold full-width characters.
        let folded_verb;
        let verb = if raw_verb.is_ascii() {
            raw_verb
        } else {
            folded_verb = crate::util::fold_ime_word(raw_verb);
            folded_verb.as_str()
        };

        match verb {
            "q" | "quit" => self.should_quit = true,
            "shell" => self.focus(FocusedPane::Shell),
            "help" | "man" | "h" => self.open_manual(),
            "paste" => { let _ = self.paste_clip(); }
            "hidden" => self.toggle_hidden(),
            // **AFXW のマスク。** `/` の絞り込みはディレクトリを移ると消える
            // （それが `/` の仕事）。こちらは付けたまま歩くもので、
            // 「今日はログを見ている」を画面に固定する。引数なしで解除し、
            // 何が外れたかを言う ── 黙って全部出ると、直前に何を見ていたのか
            // が分からなくなる。
            "mask" => self.cmd_mask(rest),
            // 二画面の同期移動。名前は本人が選んだ（`:sync` はシェルの
            // シンクロ入力で埋まっている）。
            "mirror" => self.toggle_mirror(),
            // `r` and the menu have always had this; the command line never
            // did, and the name was taken by the AI renamer.
            "rename" | "ren" => self.start_rename(),
            // `:view` opens the file under the cursor. It used to take
            // `details` / `classic` / `icons` as well, which set a flag only
            // the windowed build collected — so in a terminal they were three
            // words that did nothing. The window has its own `:view` and keeps
            // both looks; the terminal has one look and says so.
            // `classic` / `details` は窓版で `:view` の別名（表示モード）。
            // 端末版は見た目が一つなので、**その面が無いことを言う** ── 名前が
            // 通らないのと、この端末には無いのとは別のことだ。
            "view" | "classic" | "details" => match (verb, rest) {
                ("view", "") => self.look_inside(),
                _ => {
                    self.message = Some(
                        tr(
                            self.lang,
                            "the two looks are the window's. a terminal has one, and :view opens the file under the cursor",
                            "表示モードの切替はウィンドウ版のものです。端末版の見た目は一つで、:view はカーソル位置のファイルを開きます",
                        )
                        .into(),
                    )
                }
            },
            "diff" | "compare" => self.open_diff(),
            "copyto" => self.start_dest_picker(PendingOp::Copy),
            "moveto" => self.start_dest_picker(PendingOp::Move),
            "version" | "about" => self.show_version(),
            // Pixels or half-blocks, for a terminal that offers a picture
            // protocol and then does not draw with it.
            // Not `:gfx` — three letters nobody reaches for, and `image` says
            // what it is about.
            "image" => self.toggle_image_protocol(),
            // The prompt opens seeded when a pattern was given: it used to be
            // parsed off the line and thrown away, so `:grep foo` and `:grep`
            // did the same thing and one of them looked broken.
            "grep" => {
                self.start_grep_prompt();
                if !rest.is_empty() {
                    if let Popup::TextInput { buffer, cursor, .. } = &mut self.popup {
                        *buffer = rest.to_string();
                        *cursor = buffer.chars().count();
                    }
                }
            }
            // A blank file in this pane, with no name until `:w <name>`.
            "new" | "scratch" => self.open_scratch_viewer(),
            "markall" | "selectall" => self.mark_all(),
            "office" => self.open_in_office(),
            "officelink" => self.write_office_link(),
            // Repaint from nothing. A stray control character — one the
            // terminal acted on rather than passing along — can leave the
            // screen holding text cian never drew, and there is otherwise no
            // way back short of restarting.
            "redraw" | "refresh!" => {
                self.full_clear = true;
                self.message = Some(tr(self.lang, "redrawn", "画面を描き直しました").into());
            }
            // The input-method switch: what is configured, which way it is
            // thrown, and `:ime on` / `:ime off` to test the helper.
            "ime" | "inputmethod" => self.ime_report(rest),
            // Which key did the terminal actually send? The answer to every
            // "that shortcut does nothing on my machine".
            "key" => {
                self.key_probe = !self.key_probe;
                let mode = if self.kbd_enhanced { "enhanced" } else { "legacy" };
                self.message = Some(if self.key_probe {
                    format!(
                        "{} [keyboard: {mode}]",
                        tr(self.lang, "showing every key as cian receives it. :keys again to stop", "受け取ったキーをそのまま表示します。止めるには もう一度 :keys"),
                    )
                } else {
                    tr(self.lang, "key report off", "キー表示をやめました").to_string()
                });
            }
            "find" => {
                self.start_find_prompt();
                if !rest.is_empty() {
                    if let Popup::TextInput { buffer, cursor, .. } = &mut self.popup {
                        *buffer = rest.to_string();
                        *cursor = buffer.chars().count();
                    }
                }
            }
            "menu" => self.open_menu_at_cursor(),
            // `:limit 2M` — how fast a transfer to or from a server may go.
            // Bare, it says what the ceiling is; `off` takes it away.
            "limit" | "speed" | "ratelimit" => match rest {
                "" => {
                    let ja = self.lang == Lang::Ja;
                    self.message = Some(match (self.transfer_limit, ja) {
                        (Some(b), true) => {
                            format!("転送速度の上限: {} — :limit off で解除", rate_text(b))
                        }
                        (Some(b), false) => {
                            format!("transfer limit: {} — :limit off to remove", rate_text(b))
                        }
                        (None, true) => "転送速度の上限なし — :limit 2M で設定".to_string(),
                        (None, false) => "no transfer limit — :limit 2M to set one".to_string(),
                    });
                }
                arg => match parse_rate(arg) {
                    Some(b) => {
                        self.transfer_limit = Some(b);
                        self.message = Some(if self.lang == Lang::Ja {
                            format!("転送速度の上限: {}", rate_text(b))
                        } else {
                            format!("transfer limit: {}", rate_text(b))
                        });
                    }
                    // `off`, `none`, `0` — and anything unreadable, which is
                    // told apart from them so a typo is not silently "off".
                    None if matches!(arg.trim().to_lowercase().as_str(), "off" | "none" | "0") => {
                        self.transfer_limit = None;
                        self.message = Some(
                            tr(self.lang, "transfer limit removed", "転送速度の上限を解除").into(),
                        );
                    }
                    None => {
                        self.message = Some(format!("{arg}? — :limit 2M | 500k | off"))
                    }
                },
            },
            "sync" | "broadcast" => {
                self.focus(FocusedPane::Shell);
                let on = self.shell.toggle_broadcast();
                self.message = Some(if on {
                    "⇄ synchronize ON — input goes to all panes in this tab".into()
                } else {
                    "synchronize off".into()
                });
            }
            "count" | "step" => self.start_count(),
            "du" | "diskusage" => self.start_du_here(),
            "palette" => self.start_command_palette(),
            "jump" => self.start_fuzzy_jump(),
            // Singular is the house rule, whatever the command opens a list
            // of: "when in doubt, drop the s" is a rule you can hold in your
            // head, and one that is right more often than it is wrong.
            "toggle" => self.start_toggles(),
            // Named by what you get, because that is what someone types when
            // the editor is not behaving the way their hands expect. `T` is
            // the same switch from a listing; from inside the editor `T` is a
            // vi motion, so it needs a name that is not a letter.
            //
            // Not `:vim` — that is taken, and means the external editor. The
            // way *back* to vim keys is the panel's menu rather than a command
            // anyway: notepad style has no command line to type one at.
            "notepad" | "editstyle" | "vimkey" => self.edit_style_command(verb, rest.trim()),
            "files" | "finder" => self.start_file_finder(),
            "recent" | "oldfiles" => self.start_recent_files(),
            "each" => self.run_each(rest),
            "undo" => self.undo_last(),
            "redo" => self.redo_last(),
            "edit" | "e" => self.edit_selected_file(),
            // Rename by editing a list of names (vidir's interface).
            // Named for what they do, because `:bulkrename` and `:brename`
            // were three characters apart and did different things. The
            // family now reads together: `:rename` one file, `:renamelist`
            // edit them all as text, `:renamepattern` by rule. All three are
            // deterministic and offline; `:airename` used to sit beside them
            // proposing names over the network, which is a fourth way to do a
            // job that already had three better ones.
            "renamelist" => self.start_editor_rename(),
            // Cursor-follow preview in the shell panel's area.
            "preview" => self.toggle_preview(),
            // The operation queue: running + waiting file operations.
            "queue" => self.start_op_queue(),
            // The pane's directory history. It lost its `h` key to pane
            // movement, so it needs a name you can reach.
            //
            // `:history` is an alias here rather than on the commit log
            // (2026-09-20). It meant the commit log in this build and the
            // directory history in the window — **the same word, two
            // features, depending on which front end you were looking at**.
            // `parity.py` reads the words on screen, not what a verb does, so
            // nothing had ever complained.
            "back" | "history" => self.start_history(),
            // Strip UTF-8 BOMs from the selection (UTF-16 left alone).
            "nobom" | "stripbom" => self.start_nobom(),
            // Open the file in a specific vi-family editor in a new shell tab.
            "vi" | "vim" | "nvim" => self.edit_in_new_tab(Some(verb)),
            "macro" => self.start_macros(),
            "ssh" => self.start_ssh(),
            // `remote` is what it is; `sftp` is how, and people do search by
            // protocol. `scp` named the fallback rather than the thing, and
            // `browse`/`remotepane` named an action and a piece of the UI.
            "remote" | "sftp" => self.start_scp(ScpDir::BrowsePane),
            // **AI の入口は2語**（2026-09-20、本人: 実務ではほぼ使わない、たまに
            // コマンドを訊くくらい）。`:ai` が引数で振り分け、`:aicmd` だけ別に
            // 残る ── あれはシェル向けで、返るのが会話ではなく一行のコマンドだ
            // から。古い名前（`:aierror` `:aidiff` `:ailog` `:aicommit`）は
            // 通るが、一覧には出さない。
            "ai" | "chat" => match rest {
                "" => self.open_ai_chat(),
                "commit" => self.start_ai_commit_message(),
                "error" => self.explain_shell_error(),
                "diff" => self.explain_diff(),
                "log" => self.triage_log(),
                // 一語の合言葉でなければ質問。`:ai commit` と打って「commit とは
                // 何か」を訊きたい人は想定していない。**打ち込むだけで送らない**
                // ── 何が機械から出ていくかを最後に見るのは本人。
                q => {
                    if self.ai_ready_for_chat() {
                        self.new_ai_chat_with(q);
                    }
                }
            },
            "aicmd" => {
                if rest.is_empty() {
                    self.start_ai_shell_prompt();
                } else {
                    self.start_ai_shell_cmd(rest);
                }
            }
            // These dispatch to git or svn based on the pane's VCS.
            "stage" | "add" | "svnadd" => self.git_stage(),
            "unstage" | "reset" => self.git_unstage(),
            // Not `:checkout`: in git that switches branches, and here it threw
            // work away. A name that means something else somewhere else, on a
            // command that cannot be undone, is not a convenience.
            "discard" | "revert" | "svnrevert" => self.git_discard_prompt(),
            // **`:log` went to diagnostics** (2026-09-20, his call: 「ログを
            // とってくれ」が一番自然), so the commit log took the name a
            // person says out loud — "git のログ" — and svn's spelling of the
            // same sentence. Both reach one function, which reads whichever
            // VCS the directory is under: the two names are for the hand, not
            // a choice about what happens.
            "gitlog" | "svnlog" => self.start_git_log(),
            // Everything that used to need knowing which of seven commands to
            // type. See `cmd_log`.
            "log" => self.cmd_log(rest),
            // The session log had no verb at all, while the menu item that
            // starts it was labelled `(:log)` — a name git had already taken.
            "sessionlog" => self.start_log_prompt(),
            "shellname" | "tabname" => self.start_shell_name_prompt(),
            "gitdiff" | "gdiff" | "svndiff" => self.git_diff_file(),
            // SVN-only working-copy operations.
            // svn-only, so it says so in the name. `:up` in a file manager
            // reads as "go to the parent", which is not what it did.
            // **一語で両方**（2026-09-20）。svn にしか無い概念なので svn 専用の
            // ままだが、名前から `svn` を落とした ── git のディレクトリで叩いた
            // ときは、無いのではなく入れていないことを理由つきで言う。
            "update" | "svnupdate" => self.svn_update(),
            // Likewise. `:svncommit` names svn's, and the bare `:commit`
            // below is git's — the verb was left free for it (2026-09-10).
            // **`:commit` は git と svn のどちらでも同じ語**（2026-09-20、本人）。
            // どちらのリポジトリかはディレクトリが知っていて、バッジも状態行も
            // すでにそう言っている。`:svncommit` は手が覚えているので残すが、
            // 一覧には出さない。取り返しのつかなさの差は訊く画面が言う ──
            // `commit_prompt` の註にある。
            "commit" | "svncommit" => self.commit_prompt(),
            "resolve" | "svnresolve" => self.svn_resolve(),
            "snip" | "snippet" => self.start_snippets(),
            "aicommit" | "commitmsg" => self.start_ai_commit_message(),
            // Pattern-based (non-AI) bulk rename. With no argument it prompts;
            // with one it applies the pattern straight to the review checklist.
            "renamepattern" => {
                if rest.is_empty() {
                    self.start_bulk_rename();
                } else {
                    let targets = self.target_paths();
                    if targets.is_empty() {
                        self.message = Some(tr(self.lang, "nothing selected to rename", "リネーム対象がありません").into());
                    } else {
                        self.build_bulk_rename(&targets, rest);
                    }
                }
            }
            "aierror" | "explain" => self.explain_shell_error(),
            "aidiff" | "explaindiff" => self.explain_diff(),
            "ailog" | "logtriage" | "triage" => self.triage_log(),
            "duplicate" | "dup" | "dedup" => self.start_dupes(),
            "theme" | "colorscheme" | "colourscheme" => {
                if rest.is_empty() {
                    self.start_theme_picker();
                } else {
                    self.set_theme_by_name(rest);
                }
            }
            "reload" | "source" => self.reload_config(),
            // Re-read both listings. F5's own command, for a keyboard that
            // cannot spare the function keys.
            "refresh" | "rescan" => {
                self.reload_both();
                self.message = Some(tr(self.lang, "refreshed", "更新しました").into());
            }
            "where" | "config" => self.show_config_paths(),
            // Mark / unmark entries whose name matches a glob (`:mark *.rs`).
            "mark" | "select" => self.cmd_mark(rest, true),
            "unmark" | "deselect" => self.cmd_mark(rest, false),

            // Navigation.
            "cd" | "goto" => {
                if rest.is_empty() {
                    self.start_jump_path();
                } else {
                    let _ = self.cmd_cd(rest);
                }
            }
            "pwd" => self.cmd_pwd(),

            // Creation.
            "mkdir" | "md" => self.cmd_mkdir(&args),
            "touch" => self.cmd_touch(&args),

            // Transfers: no argument means "to the other pane", matching the
            // y/m keys; an argument is an explicit destination.
            "cp" | "copy" => self.cmd_transfer(PendingOp::Copy, rest),
            "mv" | "move" => self.cmd_transfer(PendingOp::Move, rest),
            // …but `rm` does not take one, and must say so rather than guess.
            // `:cp` and `:mv` one line up *do* honour an argument, so a hand
            // that has just used those will write `:rm oldfile.txt` — and this
            // deleted whatever was marked or under the cursor instead, which is
            // the one mistake in the group that cannot be taken back.
            "rm" | "del" | "delete" if !rest.is_empty() => {
                self.message = Some(
                    tr(
                        self.lang,
                        "rm works on the marks, or the file under the cursor — it takes no name",
                        "rm はマークかカーソル位置のファイルを消します — 名前は指定できません",
                    )
                    .into(),
                )
            }
            "rm" | "del" | "delete" => self.start_delete(),

            // Inspection.
            "ls" | "dir" => self.cmd_ls(&args),
            // `:ls` is here too: it is the same question asked of the whole
            // listing rather than the selection, and it is what hands type.
            "attr" | "stat" => self.show_attributes(),
            "wc" => self.cmd_wc(),
            "head" => self.cmd_peek(cian_core::inspect::End::Head, &args),
            "tail" => self.cmd_peek(cian_core::inspect::End::Tail, &args),
            "df" => self.cmd_df(&args),

            // Attributes and integrity.
            "chmod" => self.set_attr_command(rest),
            // Bare toggles, like `:hidden` beside it. `true`/`1`/`false`/`0`
            // were also taken, which is four more spellings to read and none
            // of them is what a person types (2026-09-20, his call: 「短すぎる
            // し意味がわからない」).
            "readonly" => match rest {
                "" => self.toggle_readonly_command(),
                "on" => self.set_readonly_command(true),
                "off" => self.set_readonly_command(false),
                _ => self.message = Some(tr(self.lang, "usage: :readonly [on|off]", "使い方: :readonly [on|off]").into()),
            },
            "hash" | "sha256" | "md5" => {
                // `:hash md5` or `:md5` both work.
                let spec = if verb == "hash" { rest } else { verb };
                match cian_core::attrs::HashKind::parse(spec) {
                    Some(k) => self.start_hash(k),
                    None => self.message = Some(format!("unknown hash: {} (md5 or sha256)", spec)),
                }
            }

            // Archiving.
            "zip" => self.cmd_zip(&args),
            "tar" => self.cmd_tar(&args, false),
            // Not `:tgz` — the extension comes from the *name*, so `:tgz foo`
            // made `foo.tar.gz` and the verb was a small lie. `:targz foo.tgz`
            // still gets you a `.tgz`, which is the honest way to ask.
            "targz" | "tar.gz" => self.cmd_tar(&args, true),
            // Extraction auto-detects the format (zip / tar / tar.gz), so the
            // name you use says nothing about what it will open. `:extract` is
            // the honest one; `:unzip` and `:untar` stay because they are what
            // hands type. The rest spelled out a format the code never asked
            // about, and `:unar` is another program's name.
            "unzip" | "extract" | "untar" => self.extract_selected(),

            // **窓版で打てるものは、端末版でも打てる**（2026-09-21、本人）。
            //
            // ここから下は「機能は両方にあるが、端末版では打つ場所が違った」
            // ものたち。窓版の `:` は面が一つなので一覧からでもエディタの語が
            // 通るが、端末版はビューアの中の `:` にしか無かった。
            //
            // ビューアが答える語は、開いていればそのまま渡す。開いていなければ
            // 「先に開いて」と言う ── unknown command と言われるより、何が
            // 足りないかが分かる。
            // 素の `:s` `:g` `:v` は窓版が持っている。**三つは別の機能だ** ──
            // `:s` は置換の入力欄、`:g` は一致した行を消す、`:v` は一致した行
            // だけ残す。ビューアの中では `s/…/…/` `g/…/d` の形でも打てる。
            "s" if matches!(self.popup, Popup::Viewer { .. }) => self.start_replace_bar(),
            // 二つに分ける ── 一つの腕に並べると「同じ機能の別名」になり、
            // 実際は逆のことをする二つなので、機械にも人にも嘘になる。
            "g" if matches!(self.popup, Popup::Viewer { .. }) => self.viewer_line_filter(rest, false),
            "v" if matches!(self.popup, Popup::Viewer { .. }) => self.viewer_line_filter(rest, true),
            v if crate::viewer::viewer_answers(v)
                || v.starts_with("s/")
                || v.starts_with("g/")
                || v.starts_with("v/") =>
            {
                if matches!(self.popup, Popup::Viewer { .. }) {
                    self.run_substitute(raw.as_str());
                } else {
                    self.message = Some(
                        tr(
                            self.lang,
                            "that one works on an open file. Enter or F3 first",
                            "これは開いているファイルに効きます。先に Enter か F3 で開いてください",
                        )
                        .into(),
                    );
                }
            }
            // 機能はあるのに名前が無かったもの。キーは今までどおり効く。
            "branch" => self.toggle_branch_view(),
            "bookmark" => self.start_shortcuts(),
            "visual" => self.visual_start(),
            "zoom" => self.toggle_zoom(),
            "forward" => self.pane_go_forward(),
            "local" => self.leave_remote_pane(),
            "tab" => self.ask_new_tab(),
            "tabclose" => {
                if let Some(t) = self.active_file_tabs_mut() {
                    t.close_active();
                }
            }
            "revealos" | "showinfinder" => self.reveal_in_os(),
            "diffedit" => self.open_diff(),
            "lsar" => self.look_inside(),
            "filelog" => self.start_git_log(),
            // **端末版には無いもの。** 名前が通らないのではなく、その面が無い。
            // 黙って unknown command と言うより、どこで同じことができるかを言う。
            "settings" | "prefs" => {
                self.message = Some(
                    tr(
                        self.lang,
                        "no settings screen in a terminal. edit init.lua (`:log` says where it is)",
                        "端末版に設定画面はありません。init.lua を直接直してください（場所は `:log`）",
                    )
                    .into(),
                )
            }
            other => self.message = Some(format!("unknown command: :{}", other)),
        }
    }

    /// `pwd`: show the focused pane's directory and put it on the clipboard,
    /// since the usual reason to ask is to paste it somewhere.
    pub(crate) fn cmd_pwd(&mut self) {
        let Some(p) = self.active_pane() else {
            self.message = Some(tr(self.lang, "no active pane", "アクティブなペインがありません").into());
            return;
        };
        let path = p.cwd.display().to_string();
        if let Some(cb) = self.clipboard.as_mut() {
            let _ = cb.set_text(path.clone());
        }
        self.message = Some(format!("{}  (copied)", path));
    }

    /// `cd <path>`: enter a directory directly, without the prompt.
    /// `:mark <glob>` / `:unmark <glob>` — (un)mark every entry whose name
    /// matches the wildcard pattern (`*`, `?`; case-insensitive). No pattern
    /// acts on all entries.
    pub(crate) fn cmd_mark(&mut self, pattern: &str, mark: bool) {
        let pat = pattern.trim();
        let Some(p) = self.active_pane_mut() else { return };
        let mut n = 0usize;
        for i in 0..p.entries.len() {
            if pat.is_empty() || glob_match(pat, &p.entries[i].name) {
                let was = p.is_marked(i);
                if mark && !was {
                    p.set_mark_at(i);
                    n += 1;
                } else if !mark && was {
                    p.toggle_mark_at(i);
                    n += 1;
                }
            }
        }
        self.message = Some(format!(
            "{} {} entr{}",
            if mark { "marked" } else { "unmarked" },
            n,
            if n == 1 { "y" } else { "ies" }
        ));
    }

    pub(crate) fn cmd_cd(&mut self, arg: &str) -> Result<()> {
        // `cd -` / `cd ..` / `cd ~` are worth honouring since the muscle memory
        // is universal; everything else is a path.
        let target = match arg {
            "-" => self.active_pane().and_then(|p| p.history.get(1).cloned()),
            _ => Some(expand_path(arg)),
        };
        let Some(target) = target else {
            self.message = Some(tr(self.lang, "no previous directory", "直前のディレクトリがありません").into());
            return Ok(());
        };
        if !target.is_dir() {
            self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("ディレクトリではありません: {}", target.display())
        } else {
            format!("not a directory: {}", target.display())
        });
            return Ok(());
        }
        if let Some(p) = self.active_pane_mut() {
            p.jump_to(target.clone())?;
        }
        self.message = Some(format!("→ {}", target.display()));
        Ok(())
    }

    /// `mkdir <name>` / `mkdir -p a/b/c`, created in the focused directory.
    pub(crate) fn cmd_mkdir(&mut self, args: &[&str]) {
        let parents = args.contains(&"-p");
        let names: Vec<&str> = args.iter().copied().filter(|a| !a.starts_with('-')).collect();
        if names.is_empty() {
            self.message = Some(tr(self.lang, "usage: :mkdir [-p] <name>", "使い方: :mkdir [-p] <名前>").into());
            return;
        }
        let Some(cwd) = self.cwd() else { return };
        let mut made = 0;
        for name in &names {
            match cian_core::ops::make_dir(&cwd, name, parents) {
                Ok(_) => made += 1,
                Err(e) => {
                    self.message = Some(format!("mkdir: {}", e));
                    break;
                }
            }
        }
        if made > 0 {
            self.reload_active();
            if self.message.is_none() {
                self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("mkdir: {} を作成しました", made)
        } else {
            format!("mkdir: created {}", made)
        });
            }
        }
    }

    /// `touch <name>...`: create empty files, or bump the mtime of existing ones.
    /// `:touch` — `touch(1)`'s two halves, told apart by whether a name is given.
    ///
    /// **No name: move the clock on what is selected** (the marks, or the file
    /// under the cursor) to now. **A name: make that file.** Which is what
    /// `touch(1)` does, and the shape his hand already knows (2026-09-20, his
    /// call). Until then only the second half was here, so the one thing the
    /// selection could not have was a fresh timestamp.
    ///
    /// Directories are included. `ops::touch_now` goes through `filetime`
    /// rather than an open handle, which is the only way to stamp one.
    pub(crate) fn cmd_touch(&mut self, args: &[&str]) {
        let names: Vec<&str> = args.iter().copied().filter(|a| !a.starts_with('-')).collect();
        if names.is_empty() {
            let paths = self.target_paths();
            if paths.is_empty() {
                self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
                return;
            }
            let (ok, err) = self.apply_to_each(&paths, |p| cian_core::ops::touch_now(p));
            self.reload_both();
            let what = tr(self.lang, "timestamp", "日時を更新");
            self.message = Some(self.each_report(what, ok, paths.len(), err));
            return;
        }
        let Some(cwd) = self.cwd() else { return };
        let mut n = 0;
        for name in &names {
            match cian_core::ops::touch(&cwd, name) {
                Ok(_) => n += 1,
                Err(e) => {
                    self.message = Some(format!("touch: {}", e));
                    break;
                }
            }
        }
        if n > 0 {
            self.reload_active();
            if self.message.is_none() {
                self.message = Some(format!("touch: {}", n));
            }
        }
    }

    /// `cp`/`mv`: no argument moves the selection to the other pane; an
    /// argument is an explicit destination directory (or, for a single item, a
    /// new path).
    pub(crate) fn cmd_transfer(&mut self, op: PendingOp, arg: &str) {
        if arg.is_empty() {
            self.start_transfer(op);
            return;
        }
        let targets = self.target_paths();
        if targets.is_empty() {
            self.message = Some(tr(self.lang, "nothing to operate on", "操作する対象がありません").into());
            return;
        }
        let dest = expand_path(arg);
        if dest.is_dir() {
            self.confirm_transfer(op, targets, dest);
            return;
        }
        // Not an existing directory: only meaningful as a rename/copy of a
        // single item to that exact path, and only if its parent exists.
        if targets.len() != 1 {
            self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("ディレクトリではありません: {}", dest.display())
        } else {
            format!("not a directory: {}", dest.display())
        });
            return;
        }
        let parent_ok = dest.parent().map(|p| p.as_os_str().is_empty() || p.is_dir()).unwrap_or(false);
        if !parent_ok {
            self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("そのようなディレクトリはありません: {}", dest.display())
        } else {
            format!("no such directory: {}", dest.display())
        });
            return;
        }
        let src = &targets[0];
        let res = match op {
            PendingOp::Move => std::fs::rename(src, &dest).map_err(anyhow::Error::from),
            PendingOp::Copy => cian_core::ops::copy_one(src, dest.parent().unwrap_or(&dest), Conflict::Overwrite)
                .and_then(|_| {
                    // copy_one lands it under the parent with the source name;
                    // if a different name was asked for, put it right.
                    let landed = dest.parent().unwrap_or(&dest).join(src.file_name().unwrap_or_default());
                    if landed != dest { std::fs::rename(&landed, &dest)?; }
                    Ok(())
                }),
        };
        match res {
            Ok(_) => {
                self.reload_both();
                self.message = Some(format!("{} → {}", if op == PendingOp::Move { "mv" } else { "cp" }, dest.display()));
            }
            Err(e) => self.message = Some(format!("{}: {}", if op == PendingOp::Move { "mv" } else { "cp" }, e)),
        }
    }

    /// `ls`: refresh the listing. `ls -a` toggles hidden files, which is the
    /// one flag that makes sense when the pane already *is* the listing.
    pub(crate) fn cmd_ls(&mut self, args: &[&str]) {
        // `:ls -a` still toggles dotfiles (the long-standing behaviour). A plain
        // `:ls` now shows the same Attributes window as the menu, but for every
        // entry in the listing — a detailed `ls -l`-style view.
        if args.iter().any(|a| a.starts_with('-') && a.contains('a')) {
            self.toggle_hidden();
            return;
        }
        let paths: Vec<PathBuf> = match self.active_pane() {
            Some(p) => p.entries.iter().filter(|e| !e.is_parent).map(|e| e.path.clone()).collect(),
            None => Vec::new(),
        };
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "empty directory", "空のディレクトリです").into());
            return;
        }
        // Same cap as the Attributes window — the popup is not scrollable, and a
        // longer list would clip; the trailing "… and N more" says so.
        self.open_popup(Popup::Notice { lines: self.attributes_lines(&paths, 40) });
    }
    pub(crate) fn cmd_wc(&mut self) {
        let paths = self.target_paths();
        if paths.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        }
        let mut lines = vec![format!("{:>9} {:>9} {:>11}  name", "lines", "words", "bytes"), String::new()];
        let mut tot = cian_core::inspect::Counts::default();
        let mut shown = 0;
        for path in paths.iter().take(30) {
            let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            match cian_core::inspect::count(path) {
                Ok(c) => {
                    tot.lines += c.lines;
                    tot.words += c.words;
                    tot.bytes += c.bytes;
                    lines.push(format!("{:>9} {:>9} {:>11}  {}", c.lines, c.words, c.bytes, truncate(&name, 30)));
                    shown += 1;
                }
                Err(e) => lines.push(format!("{:>31}  {}: {}", "", truncate(&name, 20), e)),
            }
        }
        if shown > 1 {
            lines.push(String::new());
            lines.push(format!("{:>9} {:>9} {:>11}  total", tot.lines, tot.words, tot.bytes));
        }
        self.open_popup(Popup::Notice { lines });
    }

    /// `head`/`tail [-n N]`: the first or last N lines of the selected file.
    pub(crate) fn cmd_peek(&mut self, end: cian_core::inspect::End, args: &[&str]) {
        let n = parse_dash_n(args).unwrap_or(10);
        let Some(path) = self.active_pane().and_then(|p| p.selected().map(|e| e.path.clone())) else {
            self.message = Some(tr(self.lang, "nothing selected", "選択されていません").into());
            return;
        };
        match cian_core::inspect::peek(&path, end, n) {
            Ok(rows) => {
                let which = if end == cian_core::inspect::End::Head { "head" } else { "tail" };
                let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let mut lines = vec![format!("{} -n {}  {}", which, n, name), String::new()];
                lines.extend(rows.into_iter().map(|l| truncate(&l, 200)));
                self.open_popup(Popup::Notice { lines });
            }
            Err(e) => self.message = Some(format!("{}", e)),
        }
    }

    /// `df [-h|-k|-m|-g]`: free space on the focused pane's filesystem.
    pub(crate) fn cmd_df(&mut self, args: &[&str]) {
        let unit = match args.iter().find(|a| a.starts_with('-')) {
            Some(flag) => match cian_core::inspect::Unit::parse(flag) {
                Some(u) => u,
                None => {
                    self.message = Some(format!("df: unknown flag {} (try -h -k -m -g)", flag));
                    return;
                }
            },
            None => cian_core::inspect::Unit::Human,
        };
        let Some(cwd) = self.cwd() else { return };
        match cian_core::inspect::disk_space(&cwd) {
            Ok(s) => {
                let lines = vec![
                    format!("filesystem holding  {}", cwd.display()),
                    String::new(),
                    format!("total      {}", unit.format(s.total)),
                    format!("used       {}   ({}%)", unit.format(s.used()), s.percent_used()),
                    format!("available  {}", unit.format(s.available)),
                ];
                self.open_popup(Popup::Notice { lines });
            }
            Err(e) => self.message = Some(format!("df: {}", e)),
        }
    }

    /// `zip [-e] <name>`: bundle the selection. `-e` asks for a password and
    /// AES-encrypts the result.
    pub(crate) fn cmd_zip(&mut self, args: &[&str]) {
        let encrypt = args.contains(&"-e") || args.contains(&"-p");
        let name = args.iter().copied().find(|a| !a.starts_with('-'));
        let Some(name) = name else {
            self.message = Some(tr(self.lang, "usage: :zip [-e] <name.zip>", "使い方: :zip [-e] <名前.zip>").into());
            return;
        };
        let sources = self.target_paths();
        if sources.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected to zip", "zip にする対象が選択されていません").into());
            return;
        }
        let Some(cwd) = self.cwd() else { return };
        let mut fname = name.to_string();
        if !fname.to_lowercase().ends_with(".zip") {
            fname.push_str(".zip");
        }
        let dest = cwd.join(&fname);
        if dest.exists() {
            self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("既に存在します: {}", fname)
        } else {
            format!("already exists: {}", fname)
        });
            return;
        }
        if encrypt {
            // Collect the password on a masked prompt, then build the zip when
            // it is submitted.
            self.open_popup(text_input(
                "zip password",
                "password (AES-256; Explorer cannot open — use 7-Zip):",
                String::new(),
                InputKind::ZipPassword { dest, sources },
            ));
        } else {
            self.start_zip(dest, sources, None);
        }
    }

    /// Kick off zip creation on a worker, with progress and cancel like the
    /// other bulk operations.
    pub(crate) fn start_zip(&mut self, dest: PathBuf, sources: Vec<PathBuf>, password: Option<String>) {
        self.start_op("zipping", move |ctl| {
            cian_core::archive::create_zip(&sources, &dest, password.as_deref(), ctl)
        });
    }

    /// `:tar <name>` / `:targz <name>` — tar up the marked files (or the cursor's)
    /// into the active pane's directory.
    pub(crate) fn cmd_tar(&mut self, args: &[&str], gz: bool) {
        let Some(name) = args.iter().copied().find(|a| !a.starts_with('-')) else {
            self.message = Some(if gz { "usage: :targz <name>" } else { "usage: :tar <name>" }.into());
            return;
        };
        let sources = self.target_paths();
        if sources.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected to archive", "アーカイブにする対象が選択されていません").into());
            return;
        }
        let Some(cwd) = self.cwd() else { return };
        let mut fname = name.to_string();
        let low = fname.to_lowercase();
        if gz {
            if !(low.ends_with(".tar.gz") || low.ends_with(".tgz")) {
                fname.push_str(".tar.gz");
            }
        } else if !low.ends_with(".tar") {
            fname.push_str(".tar");
        }
        let dest = cwd.join(&fname);
        if dest.exists() {
            self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("既に存在します: {}", fname)
        } else {
            format!("already exists: {}", fname)
        });
            return;
        }
        self.start_tar(dest, sources, gz);
    }

    /// Kick off tar creation on a worker (progress + cancel like zip).
    pub(crate) fn start_tar(&mut self, dest: PathBuf, sources: Vec<PathBuf>, gz: bool) {
        let label = if gz { "tarring gz" } else { "tarring" };
        self.start_op(label, move |ctl| cian_core::archive::create_tar(&sources, &dest, gz, ctl));
    }

    /// From the right-click Compress submenu: gather the selection and ask for
    /// the archive name; [`Self::finish_text_input`] builds it on submit.
    pub(crate) fn prompt_compress(&mut self, kind: CompressKind) {
        let sources = self.target_paths();
        if sources.is_empty() {
            self.message = Some(tr(self.lang, "nothing selected to compress", "圧縮対象がありません").into());
            return;
        }
        // A sensible default name: the single selection's stem, else the folder's.
        let default = sources
            .first()
            .filter(|_| sources.len() == 1)
            .and_then(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()))
            .or_else(|| self.active_pane().and_then(|p| p.cwd.file_name().and_then(|n| n.to_str()).map(|s| s.to_string())))
            .unwrap_or_else(|| "archive".to_string());
        let ext = match kind {
            CompressKind::Zip | CompressKind::ZipEnc => ".zip",
            CompressKind::TarGz => ".tar.gz",
        };
        self.open_popup(text_input(
            "compress",
            format!("archive name (adds {}):", ext),
            default,
            InputKind::CompressName { kind, sources },
        ));
    }

    /// `!cmd`: run a shell command in the shell panel, with `%` substituted by
    /// the selected paths, `%f` by the current file, `%d` by the directory.
    pub(crate) fn run_bang(&mut self, cmd: &str) {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            self.message = Some(tr(self.lang, "usage: :!<command>   (% = selection, %f = file, %d = dir)", "使い方: :!<コマンド>   (% = 選択, %f = ファイル, %d = ディレクトリ)").into());
            return;
        }
        let pane = self.active_pane();
        let cwd = pane.map(|p| p.cwd.display().to_string()).unwrap_or_default();
        let file = pane
            .and_then(|p| p.selected().map(|e| e.path.display().to_string()))
            .unwrap_or_default();
        let sel: Vec<String> = pane
            .map(|p| p.target_paths())
            .unwrap_or_default()
            .iter()
            .map(|p| shell_quote(&p.display().to_string()))
            .collect();
        let sel = sel.join(" ");

        // Longer tokens first so `%f`/`%d` are not eaten by `%`.
        let expanded = cmd
            .replace("%f", &shell_quote(&file))
            .replace("%d", &shell_quote(&cwd))
            .replace('%', &sel);
        self.run_in_shell(expanded);
    }
}
