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
        self.mode = Mode::Command;
    }

    pub(crate) fn run_command(&mut self) {
        let raw = self.command_buffer.trim().to_string();
        self.command_buffer.clear();
        self.mode = Mode::Normal;
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
            "man" | "help" | "h" => self.open_manual(),
            "paste" => { let _ = self.paste_clip(); }
            "hidden" => self.toggle_hidden(),
            // `r` and the menu have always had this; the command line never
            // did, and the name was taken by the AI renamer.
            "rename" | "ren" => self.start_rename(),
            // `:view` opens the file under the cursor. It used to take
            // `details` / `classic` / `icons` as well, which set a flag only
            // the windowed build collected — so in a terminal they were three
            // words that did nothing. The window has its own `:view` and keeps
            // both looks; the terminal has one look and says so.
            "view" => match rest {
                "" => self.look_inside(),
                other => {
                    self.message = Some(format!(
                        "{other}? — :view opens the file under the cursor",
                    ))
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
            "files" => self.start_file_finder(),
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
            "back" => self.start_history(),
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
            "ai" | "chat" => self.open_ai_chat(),
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
            // One function reads whichever VCS is there, so `:gitlog` and
            // `:svnlog` offered a choice that does not exist.
            "log" | "history" => self.start_git_log(),
            // The session log had no verb at all, while the menu item that
            // starts it was labelled `(:log)` — a name git had already taken.
            "sessionlog" => self.start_log_prompt(),
            "shellname" | "tabname" => self.start_shell_name_prompt(),
            "gitdiff" | "gdiff" | "svndiff" => self.git_diff_file(),
            // SVN-only working-copy operations.
            // svn-only, so it says so in the name. `:up` in a file manager
            // reads as "go to the parent", which is not what it did.
            "svnupdate" => self.svn_update(),
            // Likewise. cian has no git commit, so a bare `:commit` was a
            // generic name for a specific thing — and left the obvious verb
            // taken if git commit is ever added.
            "svncommit" => self.svn_commit_prompt(),
            "svnresolve" | "resolve" => self.svn_resolve(),
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
            "duplicate" | "dup" => self.start_dupes(),
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
                        "rm はマークまたはカーソル位置に対して働きます — 名前は取りません",
                    )
                    .into(),
                )
            }
            "rm" | "del" | "delete" => self.start_delete(),

            // Inspection.
            "ls" | "dir" => self.cmd_ls(&args),
            // `:ls` is here too: it is the same question asked of the whole
            // listing rather than the selection, and it is what hands type.
            "stat" | "attr" => self.show_attributes(),
            "wc" => self.cmd_wc(),
            "head" => self.cmd_peek(cian_core::inspect::End::Head, &args),
            "tail" => self.cmd_peek(cian_core::inspect::End::Tail, &args),
            "df" => self.cmd_df(&args),

            // Attributes and integrity.
            "chmod" => self.set_attr_command(rest),
            "readonly" => match rest {
                "on" | "true" | "1" => self.set_readonly_command(true),
                "off" | "false" | "0" => self.set_readonly_command(false),
                _ => self.message = Some(tr(self.lang, "usage: :readonly on|off", "使い方: :readonly on|off").into()),
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
            "extract" | "unzip" | "untar" => self.extract_selected(),

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
    pub(crate) fn cmd_touch(&mut self, args: &[&str]) {
        let names: Vec<&str> = args.iter().copied().filter(|a| !a.starts_with('-')).collect();
        if names.is_empty() {
            self.message = Some(tr(self.lang, "usage: :touch <name>", "使い方: :touch <名前>").into());
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
