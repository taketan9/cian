//! The UI-toggles menu (`T` / `:toggle`): one place to flip the handful of
//! live on/off settings, inspired by AstroNvim's `<Leader>u`. Each row shows its
//! current state and toggles in place, so the menu stays open for several flips.

use super::*;

/// One switch in the toggles menu.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ToggleId {
    /// Show dotfiles in the panes.
    Dotfiles,
    /// Broadcast keyboard input to every shell pane in the tab.
    Sync,
    /// Ring the bell / post a desktop toast when a long job finishes.
    Notify,
    /// Re-read and checksum-verify files after an SFTP transfer.
    Verify,
    /// Cursor-follow preview in the shell panel's area.
    Preview,
    /// Let sweeps read cloud placeholders (and so download them).
    ReadCloud,
    /// Interface language (English ↔ Japanese).
    Lang,
    /// Which grammar the built-in editor answers to: vim ↔ notepad.
    EditStyle,
}

impl App {
    /// Open the toggles menu (`T` / `:toggles`).
    pub(crate) fn start_toggles(&mut self) {
        // A panel open beside the listing steps aside rather than being
        // written over — `T` reaches this from a listing while the panel is
        // still the thing in `self.popup`, so without this the switches ate
        // the open file, unsaved edits and all. Same fault the manual and the
        // context menu had; see `stash_viewer`.
        self.open_popup(Popup::Toggles { cursor: 0 });
    }

    /// The rows shown in the menu: `(id, label, state text, on?)`. `on` drives
    /// the accent so an enabled switch reads at a glance.
    pub(crate) fn toggle_rows(&self) -> Vec<(ToggleId, String, String, bool)> {
        let ja = self.lang == Lang::Ja;
        let onoff = |b: bool| {
            if b {
                tr(self.lang, "on", "ON").to_string()
            } else {
                tr(self.lang, "off", "OFF").to_string()
            }
        };
        let dotfiles = self.active_pane().map(|p| p.show_hidden).unwrap_or(false);
        let sync = self.shell.is_broadcasting();
        let notify = self.notify_runtime.or(self.config.options.notify).unwrap_or(true);
        let verify = self.verify_runtime.or(self.config.options.verify_transfers).unwrap_or(false);
        vec![
            (ToggleId::Dotfiles, tr(self.lang, "Dotfiles", "隠しファイル").into(), onoff(dotfiles), dotfiles),
            (ToggleId::Sync, tr(self.lang, "Input sync (all shells)", "入力同期（全シェル）").into(), onoff(sync), sync),
            (ToggleId::Notify, tr(self.lang, "Task-done notification", "完了通知").into(), onoff(notify), notify),
            (ToggleId::Verify, tr(self.lang, "Verify transfers", "転送後ベリファイ").into(), onoff(verify), verify),
            (
                ToggleId::Preview,
                tr(self.lang, "Cursor preview (shell panel)", "カーソル追従プレビュー").into(),
                onoff(self.preview_on),
                self.preview_on,
            ),
            (
                ToggleId::ReadCloud,
                // **印を先頭に置かない。** 他の行のラベルは `│   ` の次から
                // 始まるのに、この行だけ ☁ のぶん2桁ぶん右にずれていて、
                // メニューの左端がここだけ揃っていなかった（2026-09-06 の指摘）。
                // 英語は文の中に入っているので最初から揃っている。
                tr(self.lang, "Read ☁ cloud-only files", "クラウド上（☁）のファイルも読む").into(),
                onoff(cian_core::cloud::include()),
                cian_core::cloud::include(),
            ),
            (
                ToggleId::Lang,
                tr(self.lang, "Language", "言語").into(),
                if ja { "日本語".into() } else { "English".into() },
                ja,
            ),
            // Named by what it *is*, not by on/off: neither of the two is the
            // absence of the other, and "Editor: off" would say nothing. The
            // accent goes to vim because that is the one this switch is a
            // departure from.
            (
                ToggleId::EditStyle,
                tr(self.lang, "Editor keys", "エディタのキー操作").into(),
                match self.edit_style {
                    crate::EditStyle::Vim => tr(self.lang, "vim", "vim").to_string(),
                    crate::EditStyle::Notepad => {
                        tr(self.lang, "notepad", "メモ帳").to_string()
                    }
                },
                self.edit_style == crate::EditStyle::Vim,
            ),
        ]
    }

    /// Move the menu cursor by `delta`, clamped.
    pub(crate) fn toggles_move(&mut self, delta: isize) {
        let n = self.toggle_rows().len();
        if let Popup::Toggles { cursor } = &mut self.popup {
            if n > 0 {
                *cursor = (*cursor as isize + delta).clamp(0, n as isize - 1) as usize;
            }
        }
    }

    /// Flip the highlighted switch, leaving the menu open.
    pub(crate) fn toggles_apply(&mut self) {
        let cursor = match self.popup {
            Popup::Toggles { cursor } => cursor,
            _ => return,
        };
        let Some((id, ..)) = self.toggle_rows().into_iter().nth(cursor) else { return };
        match id {
            ToggleId::Dotfiles => self.toggle_hidden(),
            ToggleId::Sync => {
                let was = self.shell.is_broadcasting();
                let on = self.shell.toggle_broadcast();
                self.message = Some(if on {
                    tr(self.lang, "⇄ input synchronize ON", "⇄ 入力同期 ON").into()
                } else if !was {
                    // It refused rather than turned off. `set_broadcast` will
                    // not switch on with nothing to synchronise *to*, and said
                    // so by staying off — which from the menu is indis-
                    // tinguishable from the key not working at all.
                    tr(
                        self.lang,
                        "input sync needs two or more shell panes — Shift+F8 / Shift+F9 to split one",
                        "入力同期にはシェルペインが2つ以上必要です — Shift+F8 / Shift+F9 で分割",
                    )
                    .into()
                } else {
                    tr(self.lang, "input synchronize off", "入力同期 OFF").into()
                });
            }
            // These two changed a value and said nothing, so the only sign
            // anything had happened was the row's own state text — which is
            // easy to miss when the eye is on the key you just pressed. Every
            // other switch here reports; these now do too.
            ToggleId::Notify => {
                let cur = self.notify_runtime.or(self.config.options.notify).unwrap_or(true);
                self.notify_runtime = Some(!cur);
                self.message = Some(if !cur {
                    tr(
                        self.lang,
                        "task-done notification ON — a bell and a toast when a long job finishes",
                        "完了通知 ON — 時間のかかる処理が終わったら知らせます",
                    )
                    .into()
                } else {
                    tr(self.lang, "task-done notification off", "完了通知 OFF").into()
                });
            }
            ToggleId::Verify => {
                let cur =
                    self.verify_runtime.or(self.config.options.verify_transfers).unwrap_or(false);
                self.verify_runtime = Some(!cur);
                self.message = Some(if !cur {
                    tr(
                        self.lang,
                        "verify transfers ON — files are re-read and checksummed after an upload",
                        "転送後ベリファイ ON — 転送後に読み直してチェックサムを照合します",
                    )
                    .into()
                } else {
                    tr(self.lang, "verify transfers off", "転送後ベリファイ OFF").into()
                });
            }
            ToggleId::Preview => {
                self.toggle_preview();
                // It turned on, but there is nowhere for it to appear: the
                // preview draws in the shell panel's area, and this view has
                // no shell panel. Without this the switch reads as broken.
                if self.preview_on && !self.has_shell_panel() {
                    self.message = Some(
                        tr(
                            self.lang,
                            "preview on — but it draws in the shell panel, which is not open here (Shift+J)",
                            "プレビュー ON — ただし表示先のシェル枠が開いていません（Shift+J）",
                        )
                        .into(),
                    );
                }
            }
            ToggleId::ReadCloud => {
                let on = !cian_core::cloud::include();
                cian_core::cloud::set_include(on);
                self.message = Some(if on {
                    tr(self.lang,
                       "⚠ sweeps will now download cloud-only files",
                       "⚠ 一括処理がクラウド上のファイルをダウンロードします").into()
                } else {
                    tr(self.lang, "sweeps skip cloud-only files", "一括処理はクラウド上のファイルを飛ばします").into()
                });
            }
            // The whole interface, menus included — see `MenuItem::Lang`.
            ToggleId::Lang => {
                self.lang = self.lang.toggled();
                if !self.menu_lang_pinned {
                    self.menu_lang = self.lang;
                }
            }
            ToggleId::EditStyle => self.flip_edit_style(),
        }
    }

    /// Swap the editor's grammar, and say which one is on now.
    ///
    /// Shared by the three ways in: `T` in the listings, the panel's own menu
    /// (the only one reachable once the editor has the keyboard, since `T` is
    /// a vi motion there and a character in the other grammar), and
    /// `:vim` / `:notepad`.
    pub(crate) fn flip_edit_style(&mut self) {
        self.set_edit_style(match self.edit_style {
            crate::EditStyle::Vim => crate::EditStyle::Notepad,
            crate::EditStyle::Notepad => crate::EditStyle::Vim,
        });
    }

    /// `:notepad`, `:editstyle vim|notepad`, or either with no argument to
    /// flip. Shared by the file pane's command line and the editor's own,
    /// because the editor is where you are when you reach for this and the
    /// two must not drift.
    pub(crate) fn edit_style_command(&mut self, verb: &str, arg: &str) {
        if verb == "notepad" || verb == "plain" {
            self.set_edit_style(crate::EditStyle::Notepad);
            return;
        }
        match arg {
            "vim" | "vi" => self.set_edit_style(crate::EditStyle::Vim),
            "notepad" | "plain" => self.set_edit_style(crate::EditStyle::Notepad),
            "" => self.flip_edit_style(),
            other => self.message = Some(format!("editstyle: vim | notepad (got {other:?})")),
        }
    }

    pub(crate) fn set_edit_style(&mut self, to: crate::EditStyle) {
        self.edit_style = to;
        // A file already open changes grammar under the cursor, which is the
        // point of flipping this while one is: the switch is reached for
        // *because* the editor is not behaving the way the hands expect.
        // Notepad is always typing, so a panel that was sitting in normal mode
        // starts taking text.
        self.sync_edit_style();
        self.message = Some(match self.edit_style {
            crate::EditStyle::Vim => tr(
                self.lang,
                "editor: vim keys — i to type, Esc to stop",
                "エディタ: vim キー操作 — i で入力、Esc で抜ける",
            )
            .into(),
            crate::EditStyle::Notepad => tr(
                self.lang,
                "editor: notepad keys — just type; Shift+arrows select",
                "エディタ: メモ帳ふう — そのまま入力、Shift+矢印で選択",
            )
            .into(),
        });
    }
}
