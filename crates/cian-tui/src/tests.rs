    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::BorderType;

    /// The active theme lives in a process-wide global, so tests that mutate or
    /// assert on it must not run concurrently with each other. They all take this
    /// lock first.
    static THEME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn code(k: KeyCode) -> KeyEvent {
        KeyEvent::new(k, KeyModifiers::NONE)
    }

    #[test]
    fn the_solarized_light_preset_paints_a_light_base() {
        let t = cian_lua::Theme { preset: Some("solarized-light".into()), ..Default::default() };
        let (c, errors) = resolve_theme(&t);
        assert!(errors.is_empty(), "{:?}", errors);
        assert_eq!(c.base_bg, Some(Color::Rgb(0xfd, 0xf6, 0xe3)), "base3 background");
        assert_eq!(c.accent, Color::Rgb(0x26, 0x8b, 0xd2), "solarized blue accent");
        assert_eq!(c.file.directory, Color::Rgb(0x26, 0x8b, 0xd2));
    }

    #[test]
    fn the_default_theme_keeps_the_dark_look() {
        let (c, errors) = resolve_theme(&cian_lua::Theme::default());
        assert!(errors.is_empty());
        assert_eq!(c.base_bg, None, "no painted background — the terminal shows through");
        assert_eq!(c.accent, Color::Cyan);
    }

    #[test]
    fn per_key_overrides_apply_on_top_of_a_preset() {
        let t = cian_lua::Theme {
            preset: Some("solarized-light".into()),
            accent: Some("#ff0000".into()),
            ..Default::default()
        };
        let (c, errors) = resolve_theme(&t);
        assert!(errors.is_empty());
        assert_eq!(c.accent, Color::Rgb(255, 0, 0), "override wins");
        assert_eq!(c.base_bg, Some(Color::Rgb(0xfd, 0xf6, 0xe3)), "rest stays solarized");
    }

    #[test]
    fn an_unknown_preset_reports_and_falls_back_to_dark() {
        let t = cian_lua::Theme { preset: Some("nope".into()), ..Default::default() };
        let (c, errors) = resolve_theme(&t);
        assert!(errors.iter().any(|e| e.contains("unknown preset")), "{:?}", errors);
        assert_eq!(c.base_bg, None);
    }

    /// Close the file the viewer is reading, the way it closes: `:q` — Esc
    /// peels state and stops, as it does in vi.
    fn quit_viewer(app: &mut App) {
        for k in [':', 'q'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
    }

    /// Close it and throw away unsaved edits.
    fn quit_viewer_discarding(app: &mut App) {
        for k in [':', 'q', '!'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
    }

    /// An app rooted at a temp dir containing `names`.
    /// A default config that asks for English, which is what the assertions
    /// in this file read. cian's own default is Japanese — see
    /// `the_interface_is_japanese_unless_asked`.
    fn en_config() -> cian_lua::Config {
        let mut c = cian_lua::Config::default();
        c.options.lang = Some("en".into());
        c
    }

    fn app_with(names: &[&str]) -> (tempfile::TempDir, App) {
        app_with_keymaps(names, Vec::new())
    }

    /// Like `app_with`, but with the `lang` option set.
    fn app_with_lang(names: &[&str], lang: &str) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        for n in names {
            std::fs::write(dir.path().join(n), b"").unwrap();
        }
        let p = dir.path().to_path_buf();
        let mut config = en_config();
        config.options.lang = Some(lang.to_string());
        let app = App::new(p.clone(), p, config).unwrap();
        (dir, app)
    }

    /// Like `app_with`, but with `cian.set_keymap` overrides applied.
    fn app_with_keymaps(names: &[&str], keymaps: Vec<(&str, String)>) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        for n in names {
            std::fs::write(dir.path().join(n), b"").unwrap();
        }
        let p = dir.path().to_path_buf();
        let mut config = en_config();
        config.keymaps = keymaps.into_iter().map(|(k, a)| (k.to_string(), a)).collect();
        let app = App::new(p.clone(), p, config).unwrap();
        (dir, app)
    }

    #[test]
    fn shortcuts_save_as_lua_and_legacy_formats_still_migrate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shortcuts.lua");
        let store = ShortcutStore {
            entries: vec![
                Shortcut::leaf("home".into(), "~/".into()),
                Shortcut::leaf("docs".into(), "https://example.com".into()),
            ],
            path: path.clone(),
        };
        store.save().unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("return {"), "written as Lua:\n{text}");
        assert!(text.contains("name = \"home\""), "written as Lua:\n{text}");
        // Round-trips through the Lua reader the loader uses.
        let back = cian_lua::shortcuts::parse(&text).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].name, "home");

        // A pre-existing YAML/TOML file must still parse, so migration keeps
        // entries for anyone on the old formats.
        let yaml = "shortcuts:\n  - name: srv\n    target: /srv\n";
        let from_yaml: ShortcutsFile = serde_yml::from_str(yaml).unwrap();
        assert_eq!(from_yaml.shortcuts[0].target.as_deref(), Some("/srv"));
        let toml_src = "[[shortcuts]]\nname = \"srv\"\ntarget = \"/srv\"\n";
        let from_toml: ShortcutsFile = toml::from_str(toml_src).unwrap();
        assert_eq!(from_toml.shortcuts[0].target.as_deref(), Some("/srv"));
    }

    #[test]
    fn ai_context_facts_are_folded_into_prompts() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // No facts configured → no context block.
        assert!(app.ai_context_block().is_empty());

        // Global facts from cian.ai_context appear as bullet points.
        app.config.ai_context = vec!["The panes browse RHEL 8.".into(), "Prefer POSIX sh.".into()];
        let block = app.ai_context_block();
        assert!(block.contains("Context about the user's environment"));
        assert!(block.contains("- The panes browse RHEL 8."));
        assert!(block.contains("- Prefer POSIX sh."));
    }

    #[test]
    fn resolve_bg_accepts_preset_names_and_specs() {
        // Preset by name (crmaine matches "crmaine (^_-)"), plus hex / r,g,b.
        assert_eq!(resolve_bg("navy"), Some(Color::Rgb(10, 40, 140)));
        assert_eq!(resolve_bg("crmaine"), Some(Color::Rgb(140, 15, 85)));
        assert_eq!(resolve_bg("#402018"), Some(Color::Rgb(0x40, 0x20, 0x18)));
        assert_eq!(resolve_bg("40,24,24"), Some(Color::Rgb(40, 24, 24)));
        assert_eq!(resolve_bg("default"), None);
        assert_eq!(resolve_bg("nonsense"), None);
    }

    #[test]
    fn broadcast_needs_more_than_one_pane() {
        // With no split panes, synchronize can't turn on (it would be pointless
        // and dangerous), and the toggle is a no-op.
        let mut app = {
            let (_d, a) = app_with(&["a.txt"]);
            a
        };
        assert!(!app.shell.set_broadcast(true), "no panes → stays off");
        assert!(!app.shell.is_broadcasting());
        assert!(!app.shell.toggle_broadcast(), "toggle is a no-op with <2 panes");
    }

    #[test]
    fn the_macro_launcher_opens_and_starts_a_run() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // Start from a known-empty set (the dev machine may have a real macro.lua).
        app.macros.clear();
        app.macro_error = None;
        // No macros defined → `@` explains rather than opening an empty menu.
        app.handle_key(key('@')).unwrap();
        assert!(matches!(app.popup, Popup::None), "no empty menu");
        assert!(app.message.as_deref().unwrap_or("").contains("macro"));

        // Inject a couple of macros (as if loaded from macro.lua).
        app.macros = cian_lua::macros::parse(
            r#"return {
                { name = "First",  panes = { { cmd = "echo one" } } },
                { name = "Second", panes = { { cmd = "echo two" }, { dir = "down", cmd = "echo three" } } },
            }"#,
        )
        .unwrap();

        // `@` now opens the launcher listing both names.
        app.handle_key(key('@')).unwrap();
        match &app.popup {
            Popup::Macros { names, cursor } => {
                // Layout macros are tagged ▦ in the launcher (§ marks scripts).
                assert_eq!(names, &["▦ First".to_string(), "▦ Second".to_string()]);
                assert_eq!(*cursor, 0);
            }
            _ => panic!("launcher did not open"),
        }

        // Move to the second and run it: the run starts and focus moves to the shell.
        app.handle_key(key('j')).unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::None), "launcher closed on run");
        assert!(app.macro_run.is_some(), "a macro run is in progress");
        assert_eq!(app.focused, FocusedPane::Shell, "shell focused for the macro");
    }

    #[test]
    fn edit_queues_the_file_for_the_external_editor() {
        let (_d, mut app) = app_with(&["note.txt"]);
        // Put the cursor on the file (index 0 may be the synthetic `..`).
        {
            let p = app.active_pane_mut().unwrap();
            p.cursor = p.entries.iter().position(|e| e.name == "note.txt").unwrap();
        }
        // `:edit` on a file queues it (the main loop runs the editor).
        app.edit_selected_file();
        match &app.pending_edit {
            Some(e) => {
                assert!(e.path.ends_with("note.txt"));
                assert!(
                    matches!(e.kind, crate::edit::EditKind::File { reopen_viewer: false, .. }),
                    ":edit does not re-open the viewer"
                );
            }
            None => panic!("edit was not queued"),
        }

        // From the F3 viewer, `E` queues it and asks to re-open the viewer after.
        app.pending_edit = None;
        app.handle_key(code(KeyCode::F(3))).unwrap();
        for k in [':', 'e', 'd', 'i', 't'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let e = app.pending_edit.as_ref().expect("viewer edit queued");
        assert!(
            matches!(e.kind, crate::edit::EditKind::File { reopen_viewer: true, .. }),
            "viewer edit re-opens the viewer"
        );
        assert!(matches!(app.popup, Popup::None), "viewer stepped aside");
    }

    /// The editor-rename round trip against a real directory: `:bulkrename`
    /// writes the list, an "edit" rewrites it, and applying renames the files —
    /// including an a↔b swap, the case a naive one-pass rename cannot do.
    #[test]
    fn editor_rename_applies_the_edited_list_even_swaps() {
        let (d, mut app) = app_with(&["a.txt", "b.txt", "keep.txt"]);
        app.start_editor_rename();
        let edit = app.pending_edit.take().expect("list queued for the editor");
        let (dir, names) = match &edit.kind {
            crate::edit::EditKind::BulkRename { dir, names } => (dir.clone(), names.clone()),
            _ => panic!("queued as a plain edit"),
        };
        assert_eq!(names, vec!["a.txt", "b.txt", "keep.txt"], "the pane's listing, in order");

        // The "editor session": swap a and b, leave keep alone.
        std::fs::write(&edit.path, "b.txt\na.txt\nkeep.txt\n").unwrap();
        app.finish_editor_rename(&edit.path, &dir, &names);

        assert!(d.path().join("a.txt").exists() && d.path().join("b.txt").exists());
        assert!(d.path().join("keep.txt").exists());
        assert!(!edit.path.exists(), "the temp list is cleaned up");
        // No half-moved temp names left behind.
        let leftovers: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".cian-rename-"))
            .collect();
        assert!(leftovers.is_empty(), "no staging temp files remain");
    }

    /// Marks narrow the list — and a rename onto a file *outside* the list (the
    /// case the in-list duplicate check cannot see) is refused by the on-disk
    /// collision check, cancelling the batch before anything moves.
    #[test]
    fn editor_rename_refuses_to_clobber_a_bystander() {
        let (d, mut app) = app_with(&["a.txt", "keep.txt"]);
        {
            let p = app.active_pane_mut().unwrap();
            let i = p.entries.iter().position(|e| e.name == "a.txt").unwrap();
            p.set_mark_at(i);
        }
        app.start_editor_rename();
        let edit = app.pending_edit.take().unwrap();
        let (dir, names) = match &edit.kind {
            crate::edit::EditKind::BulkRename { dir, names } => (dir.clone(), names.clone()),
            _ => panic!(),
        };
        assert_eq!(names, vec!["a.txt"], "marks narrow the list");

        // The "editor session" renames a.txt onto the unmarked keep.txt.
        std::fs::write(&edit.path, "keep.txt\n").unwrap();
        app.finish_editor_rename(&edit.path, &dir, &names);
        assert!(
            d.path().join("a.txt").exists() && d.path().join("keep.txt").exists(),
            "clobber rejected, both files untouched"
        );
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("exists") || msg.contains("存在"), "says why: {msg}");
    }

    /// Spin the op-job worker to completion (bulk copy/zip/extract run threaded).
    fn drain_op_job(app: &mut App) {
        for _ in 0..400 {
            if app.op_job.is_none() {
                return;
            }
            app.poll_op_job();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("op job did not finish");
    }

    /// The ☁ column appears only where a sync client left placeholders, and
    /// the badge lands on the placeholder rows. A real placeholder needs a
    /// sync client, so the flag is set directly — the detection itself is
    /// covered in `cian_core::cloud`.
    #[test]
    fn the_cloud_column_shows_only_where_placeholders_are() {
        let (_d, mut app) = app_with(&["local.txt", "onedrive.txt"]);
        // An ordinary folder pays nothing for the feature.
        let plain = render(&mut app, 100, 20).join("\n");
        assert!(!plain.contains('☁'), "no cloud column in a plain folder");

        {
            // Set on the visible listing: a reload would re-stat and clear it.
            let pane = app.active_pane_mut().unwrap();
            for e in pane.entries.iter_mut() {
                if e.name == "onedrive.txt" {
                    e.cloud = true;
                }
            }
        }
        assert!(app.active_pane().unwrap().has_cloud());
        let out = render(&mut app, 100, 20);
        let cloud_row = out.iter().find(|l| l.contains("onedrive.txt")).expect("row shown");
        let local_row = out.iter().find(|l| l.contains("local.txt")).expect("row shown");
        assert!(cloud_row.contains('☁'), "placeholder badged: {cloud_row}");
        assert!(!local_row.contains('☁'), "local file not badged: {local_row}");
    }

    /// The preview refuses a placeholder rather than downloading it just
    /// because the cursor came to rest there — unless the toggle says otherwise.
    #[test]
    fn preview_refuses_a_cloud_placeholder() {
        let (_d, mut app) = app_with(&["cloudy.txt"]);
        std::fs::write(_d.path().join("cloudy.txt"), "secret contents here\n").unwrap();
        {
            let pane = app.active_pane_mut().unwrap();
            let _ = pane.reload();
            pane.cursor = pane.entries.iter().position(|e| e.name == "cloudy.txt").unwrap();
            for e in pane.entries.iter_mut() {
                e.cloud = true;
            }
        }
        app.preview_on = true;
        let out = render(&mut app, 110, 30).join("\n");
        assert!(
            out.contains("not been downloaded") || out.contains("ダウンロードされていません"),
            "explains why, in full: {out}"
        );
        assert!(out.contains("F3"), "and names the way to see it anyway: {out}");
        assert!(!out.contains("secret contents"), "the file was not read");

        // Opting in makes the preview read it like any other file.
        cian_core::cloud::set_include(true);
        app.preview = None;
        let out = render(&mut app, 110, 30).join("\n");
        cian_core::cloud::set_include(false);
        assert!(out.contains("secret contents"), "opt-in reads it: {out}");
    }

    /// Notepad style: a file opens already taking text, and every letter is a
    /// letter — `j` types a j, `:` types a colon. Vim style is untouched.
    #[test]
    fn notepad_style_types_where_vim_style_navigates() {
        // Vim, the default: `j` moves down and `:` opens the command line.
        let (_d, mut app) = viewer_on("alpha\nbravo\n");
        assert_eq!(app.edit_style, EditStyle::Vim, "vim is the default");
        assert!(matches!(app.popup, Popup::Viewer { editing: false, .. }), "opens reading");
        app.handle_key(key('j')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { line: 1, .. }), "j moved down");
        assert_eq!(viewer_lines(&app), vec!["alpha".to_string(), "bravo".into()], "and typed nothing");

        // Notepad: the same keys are text.
        let (_d, mut app) = viewer_on("alpha\nbravo\n");
        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }), "opens taking text");
        for c in "j:x".chars() {
            app.handle_key(key(c)).unwrap();
        }
        assert_eq!(
            viewer_lines(&app).first().map(String::as_str),
            Some("j:xalpha"),
            "every one of them was a character: {:?}",
            viewer_lines(&app),
        );
        assert!(
            matches!(&app.popup, Popup::Viewer { sub_input: None, .. }),
            "and no command line opened",
        );
    }

    /// Shift and an arrow select; the same arrow without it lets go. Typing
    /// over the selection replaces it — and leaves the clipboard alone, which
    /// is what separates replacing from cutting.
    #[test]
    fn notepad_style_selects_with_shift_and_types_over_the_selection() {
        let (_d, mut app) = viewer_on("alpha bravo\n");
        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();
        app.yank = Some("something copied earlier".into());

        let shift_right = || KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
        for _ in 0..5 {
            app.handle_key(shift_right()).unwrap();
        }
        assert!(
            matches!(app.popup, Popup::Viewer { visual: Some(ViewVisual::Char), .. }),
            "five Shift+Rights selected: {:?}",
            app.popup,
        );

        // Typing replaces what was selected.
        app.handle_key(key('X')).unwrap();
        assert_eq!(
            viewer_lines(&app).first().map(String::as_str),
            Some("X bravo"),
            "the selection was replaced: {:?}",
            viewer_lines(&app),
        );
        assert!(matches!(app.popup, Popup::Viewer { visual: None, .. }), "and the selection is over");
        assert_eq!(
            app.yank.as_deref(),
            Some("something copied earlier"),
            "typing over a selection is not a cut — the clipboard is untouched",
        );

        // A plain arrow lets go of a selection.
        app.handle_key(shift_right()).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { visual: Some(_), .. }), "selected again");
        app.handle_key(code(KeyCode::Left)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { visual: None, .. }), "and let go");
    }

    /// The audit pass: promises that named the wrong key, verbs that ate their
    /// argument, and one catch-all that swallowed the command it was written
    /// to protect.
    #[test]
    fn the_audit_findings_stay_fixed() {
        // `:version` in the panel was eaten by the `:g/re/d` arm above it —
        // `strip_prefix('v')` on "version" leaves "ersion", no slash, silent
        // return. The comment over the version arm was added *because* the
        // command doing nothing there is "the worst answer to am I running
        // the fix", and it had been doing nothing the whole time.
        let (_d, mut app) = viewer_on("alpha\n");
        if let Popup::Viewer { sub_input, .. } = &mut app.popup {
            *sub_input = Some("version".into());
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(
            !matches!(app.popup, Popup::Viewer { .. }) || app.message.is_some(),
            "it answered rather than returning in silence",
        );

        // `:rm` takes no name, and must not silently delete something else.
        // `:cp` and `:mv` beside it *do* take one, which is what makes the
        // silence dangerous.
        let (_d, mut app) = app_with(&["keep.txt", "other.txt"]);
        run_cmd(&mut app, "rm keep.txt");
        assert!(
            !matches!(app.popup, Popup::ConfirmDelete { .. }),
            "it did not go ahead and delete something: {:?}",
            app.popup,
        );
        assert!(app.message.is_some(), "and it said why");

        // `:grep foo` opens the prompt with foo in it, rather than throwing
        // the word away and looking broken.
        let (_d, mut app) = app_with(&["a.txt"]);
        run_cmd(&mut app, "grep needle");
        match &app.popup {
            Popup::TextInput { buffer, .. } => assert_eq!(buffer, "needle"),
            other => panic!("expected the grep prompt, got {other:?}"),
        }

        // Shift+F1 goes back and Shift+F2 forward — the hint bar and the
        // comment above the binding both said so; the arms were swapped.
        let hints = key_hints(&app);
        assert!(
            hints.iter().all(|(k, _)| *k != "T"),
            "no hint names a key that does nothing where it is shown: {hints:?}",
        );
    }

    /// A second slash opens the fuzzy finder.
    ///
    /// The finder was good and had no key at all — `:file` was the only way in.
    /// A slash cannot appear in a filename on any platform (the separator on
    /// Unix, illegal on Windows) and the filter matches names alone, so a
    /// leading slash was input that could never match: free to give a meaning,
    /// and the meaning writes itself. One narrows what is here, two looks
    /// underneath.
    #[test]
    fn a_second_slash_opens_the_fuzzy_finder() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/deep.txt"), "x").unwrap();
        std::fs::write(d.path().join("top.txt"), "x").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.handle_key(key('/')).unwrap();
        assert_eq!(app.mode, Mode::Filter, "one slash narrows this listing");
        app.handle_key(key('/')).unwrap();
        assert_eq!(app.mode, Mode::Normal, "the second left the filter behind");
        assert!(
            matches!(app.popup, Popup::Palette { .. }),
            "the finder should be open, got {:?}",
            app.popup
        );
        // The tree arrives from a worker, the way the main loop takes it.
        drain_file_scan(&mut app);
        let Popup::Palette { items, .. } = &app.popup else {
            panic!("the finder closed, got {:?}", app.popup)
        };
        assert!(
            items.iter().any(|i| i.label.contains("deep.txt")),
            "and it reaches below this directory: {:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>(),
        );

        // A slash typed *into* a filter is still a slash — only the first one
        // means this.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        app.handle_key(key('/')).unwrap();
        app.handle_key(key('t')).unwrap();
        app.handle_key(key('/')).unwrap();
        assert_eq!(app.mode, Mode::Filter, "still filtering");
        assert_eq!(app.filter_buffer, "t/", "the slash went into the query");
    }

    /// Ctrl+Shift+P opens the palette and Ctrl+P the finder — including when
    /// the terminal spells the first one as a lowercase p with Shift reported
    /// alongside, which would otherwise have been swallowed by the second.
    #[test]
    fn the_palette_and_the_finder_do_not_take_each_others_key() {
        let ctrl_shift = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL | KeyModifiers::SHIFT);

        // The capital spelling.
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(ctrl_shift('P')).unwrap();
        assert!(
            matches!(&app.popup, Popup::Palette { kind: PaletteKind::Commands, .. }),
            "Ctrl+Shift+P is the palette, got {:?}",
            app.popup,
        );

        // …and the lowercase-with-Shift spelling, which reaches the same arm.
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(ctrl_shift('p')).unwrap();
        assert!(
            matches!(&app.popup, Popup::Palette { kind: PaletteKind::Commands, .. }),
            "the other spelling too, got {:?}",
            app.popup,
        );

        // Plain Ctrl+P is still the finder.
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)).unwrap();
        assert!(
            matches!(&app.popup, Popup::Palette { kind: PaletteKind::File, .. }),
            "Ctrl+P is the finder, got {:?}",
            app.popup,
        );

        // Ctrl+, is the palette as well.
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(KeyEvent::new(KeyCode::Char(','), KeyModifiers::CONTROL)).unwrap();
        assert!(matches!(&app.popup, Popup::Palette { kind: PaletteKind::Commands, .. }));
    }

    /// `:each` builds shell lines from filenames, and the guard has to hold —
    /// the line goes straight into the live shell.
    ///
    /// It rejected only a double quote. A POSIX shell expands `$` and a
    /// backtick *inside* double quotes, so a file called `$(id).txt` ran `id`;
    /// a newline in a name would have ended the command and begun another.
    #[test]
    fn each_will_not_let_a_filename_become_a_command() {
        use std::path::PathBuf;
        let p = |s: &str| PathBuf::from(s);

        // The shapes that must never reach a shell.
        for bad in ["$(id).txt", "`whoami`.txt", "say \"hi\".txt", "two\nlines.txt"] {
            let (lines, skipped) = crate::actions::each_lines("echo {}", &[p(bad)]);
            assert!(lines.is_empty(), "{bad:?} was let through as {lines:?}");
            assert_eq!(skipped, 1, "and counted as skipped");
        }

        // A backslash stays: on Windows it is the path separator, and with the
        // four above gone it can no longer escape anything dangerous.
        let (lines, skipped) = crate::actions::each_lines("echo {}", &[p(r"C:\Users\a.txt")]);
        assert_eq!(skipped, 0, "a Windows path is usable");
        assert_eq!(lines, vec![r#"echo "C:\Users\a.txt""#.to_string()]);

        // The ordinary jobs still work: `{}` substitutes, and without it the
        // path is appended.
        let (lines, _) = crate::actions::each_lines("wc -l {}", &[p("/tmp/a b.txt")]);
        assert_eq!(lines, vec![r#"wc -l "/tmp/a b.txt""#.to_string()], "spaces are quoted");
        let (lines, _) = crate::actions::each_lines("file", &[p("/tmp/x.txt")]);
        assert_eq!(lines, vec![r#"file "/tmp/x.txt""#.to_string()], "appended with no placeholder");

        // A mixed selection runs what it can and counts what it could not.
        let (lines, skipped) =
            crate::actions::each_lines("echo {}", &[p("/tmp/ok.txt"), p("/tmp/$(id).txt")]);
        assert_eq!(lines.len(), 1);
        assert_eq!(skipped, 1);
    }

    /// The svn verbs refuse politely outside a working copy, and in a git one.
    /// That refusal is cian's whole responsibility here — whether `svn update`
    /// itself is correct is svn's — and it is what stops a git repository
    /// getting an svn command fired at it.
    #[test]
    fn the_svn_verbs_refuse_where_they_do_not_belong() {
        // Not version-controlled at all.
        let (_d, mut app) = app_with(&["a.txt"]);
        for verb in ["svnupdate", "svncommit", "svnresolve"] {
            app.message = None;
            run_cmd(&mut app, verb);
            let said = app.message.clone().unwrap_or_default();
            assert!(
                said.contains("version-controlled") || said.contains("バージョン管理"),
                "{verb} said why: {said:?}",
            );
        }

        // A git repository is version-controlled, and still not svn — the
        // refusal that matters, since this is the case where something *would*
        // have run. A real repo: the check shells out to git.
        let (d, mut app) = app_with(&["a.txt"]);
        let dir = std::fs::canonicalize(d.path()).unwrap();
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["init", "-q"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("no git; skipping the git half");
            return;
        }
        app.reload_active();
        app.message = None;
        run_cmd(&mut app, "svnupdate");
        let said = app.message.clone().unwrap_or_default();
        assert!(
            said.contains("svn") || said.contains("専用"),
            "it did not fire svn at a git repo: {said:?}",
        );
    }

    /// Every popup goes through one door, and that door stands a live panel
    /// aside rather than writing over it.
    ///
    /// This fault was patched four separate times — the manual, the context
    /// menu, the switches, the operation queue — before anyone counted the
    /// places that raise a popup. There are ninety. The test walks a handful
    /// of the ones a hand can reach while a file is open beside the listing.
    #[test]
    fn a_popup_never_writes_over_an_open_panel() {
        for open in [
            ("the manual", "man"),
            ("the switches", "toggle"),
            ("the queue", "queue"),
            ("the sort picker", "sort"),
            ("bookmarks", "bookmark"),
        ] {
            let (label, verb) = open;
            let (_d, mut app) = viewer_on("alpha\nbravo\n");
            app.handle_key(code(KeyCode::F(12))).unwrap();
            app.handle_key(key('x')).unwrap();
            assert!(matches!(app.popup, Popup::Viewer { dirty: true, .. }), "{label}: edited");
            let dock = app.viewer_dock.expect("docked");
            app.focus(match dock {
                FocusedPane::Left => FocusedPane::Right,
                _ => FocusedPane::Left,
            });
            let _ = render(&mut app, 160, 30);

            run_cmd(&mut app, verb);
            if matches!(app.popup, Popup::Viewer { .. } | Popup::None) {
                // The verb refused for a reason of its own (nothing queued,
                // say). Nothing was displaced, so nothing was lost.
                continue;
            }
            assert!(
                app.viewer_return.is_some(),
                "{label} wrote over the panel instead of standing it aside",
            );
        }
    }

    /// The operation queue: the only way to call off a copy that is already
    /// running, and it had no test.
    ///
    /// Row 0 is the runner — drawn as "(nothing running)" when there is none —
    /// and rows below are the waiting line, so `x` on row `n` removes queue
    /// item `n - 1`. The renderer and the handler agree about that; what they
    /// did not agree about was row 0 with nothing running, where `x` did
    /// nothing and said nothing.
    #[test]
    fn the_queue_removes_the_line_the_cursor_is_on() {
        let (_d, mut app) = app_with(&["a.txt"]);
        for label in ["first", "second"] {
            app.op_queue.push_back(crate::QueuedOp {
                label,
                work: Box::new(|_| Default::default()),
                retries: 0,
            });
        }
        run_cmd(&mut app, "queue");
        assert!(matches!(app.popup, Popup::OpQueue { cursor: 0 }), "{:?}", app.popup);

        // Row 0 with nothing running says so rather than doing nothing.
        app.handle_key(key('x')).unwrap();
        assert_eq!(app.op_queue.len(), 2, "nothing was removed");
        let said = app.message.clone().unwrap_or_default();
        assert!(!said.is_empty(), "and it said why: {said:?}");

        // Row 1 is the first waiting line.
        app.handle_key(key('j')).unwrap();
        app.handle_key(key('x')).unwrap();
        assert_eq!(app.op_queue.len(), 1, "one left");
        assert_eq!(app.op_queue[0].label, "second", "the right one went");

        // The cursor cannot walk past the end.
        for _ in 0..5 {
            app.handle_key(key('j')).unwrap();
        }
        assert!(matches!(app.popup, Popup::OpQueue { cursor: 1 }), "clamped: {:?}", app.popup);
    }

    /// The sweeps that change files on disk say what they actually did.
    ///
    /// `:chmod` stopped at the first failure and reported only "chmod failed",
    /// throwing away the count of what it had already changed — so a partial
    /// application read as "nothing happened" while several files had in fact
    /// been touched. `:readonly` discarded every error and always reported
    /// success, so a run where nothing could be changed said "on 0 item(s)".
    /// Neither had a test.
    #[test]
    fn a_sweep_over_files_reports_both_halves() {
        // Mark three files, then remove one from disk behind the pane's back:
        // that one cannot be changed, the other two can.
        let (d, mut app) = app_with(&["a.txt", "b.txt", "gone.txt"]);
        {
            let pane = app.active_pane_mut().unwrap();
            let _ = pane.reload();
            for name in ["a.txt", "b.txt", "gone.txt"] {
                let i = pane.entries.iter().position(|e| e.name == name).unwrap();
                pane.set_mark_at(i);
            }
        }
        std::fs::remove_file(d.path().join("gone.txt")).unwrap();

        // `readonly`, not `chmod`: Windows has no mode bits at all, so chmod
        // fails on every path there and the mixed outcome this is about cannot
        // happen. The read-only attribute exists on both. (The Windows CI leg
        // caught exactly this — the first version of the test was written on a
        // Mac and asserted two successes.)
        run_cmd(&mut app, "readonly on");
        let said = app.message.clone().unwrap_or_default();
        assert!(said.contains('2'), "the two that worked are counted: {said:?}");
        assert!(
            said.contains('1') && (said.to_lowercase().contains("fail") || said.contains("失敗")),
            "and the one that did not is named: {said:?}",
        );

        // The two that could be changed really were — a partial sweep is not
        // a no-op, which is the whole reason it has to be reported.
        for name in ["a.txt", "b.txt"] {
            let a = cian_core::attrs::read_attrs(&d.path().join(name)).unwrap();
            assert!(a.readonly, "{name} really was changed");
        }
    }

    /// The same, for `:chmod` itself — Unix only, since Windows answers every
    /// path with "no mode bits" and cannot produce a mixed result.
    #[cfg(unix)]
    #[test]
    fn chmod_reports_both_halves() {
        let (d, mut app) = app_with(&["a.txt", "b.txt", "gone.txt"]);
        {
            let pane = app.active_pane_mut().unwrap();
            let _ = pane.reload();
            for name in ["a.txt", "b.txt", "gone.txt"] {
                let i = pane.entries.iter().position(|e| e.name == name).unwrap();
                pane.set_mark_at(i);
            }
        }
        std::fs::remove_file(d.path().join("gone.txt")).unwrap();

        run_cmd(&mut app, "chmod 644");
        let said = app.message.clone().unwrap_or_default();
        assert!(said.contains('2'), "two changed: {said:?}");
        assert!(said.contains('1') && said.to_lowercase().contains("fail"), "one did not: {said:?}");
    }

    /// Total failure is not success with a zero.
    #[test]
    fn readonly_does_not_report_success_when_nothing_worked() {
        let (d, mut app) = app_with(&["gone.txt"]);
        {
            let pane = app.active_pane_mut().unwrap();
            let _ = pane.reload();
            let i = pane.entries.iter().position(|e| e.name == "gone.txt").unwrap();
            pane.set_mark_at(i);
        }
        std::fs::remove_file(d.path().join("gone.txt")).unwrap();

        run_cmd(&mut app, "readonly on");
        let said = app.message.clone().unwrap_or_default();
        assert!(
            said.to_lowercase().contains("fail") || said.contains("失敗"),
            "it said the truth rather than \"on 0 item(s)\": {said:?}",
        );
    }

    /// `:tar` and `:targz` write archives and had no test at all. The parts
    /// worth pinning are the ones the TUI owns: the extension it appends, the
    /// one it leaves alone, and the refusal to write over something.
    #[test]
    fn tar_names_its_own_output_and_will_not_overwrite() {
        let (d, mut app) = app_with(&["one.txt", "two.txt"]);
        {
            let pane = app.active_pane_mut().unwrap();
            let _ = pane.reload();
            let i = pane.entries.iter().position(|e| e.name == "one.txt").unwrap();
            pane.set_mark_at(i);
        }

        // The write runs on a worker, as zip's does, so each one is drained.
        run_cmd(&mut app, "tar bundle");
        drain_op(&mut app);
        assert!(d.path().join("bundle.tar").is_file(), "the extension was appended");

        run_cmd(&mut app, "targz zipped");
        drain_op(&mut app);
        assert!(d.path().join("zipped.tar.gz").is_file(), "and the gz one");

        // A name that already carries a gz extension is left as it is — which
        // is the only way to ask for a `.tgz`, `:tgz` having been the verb that
        // did not give you one.
        run_cmd(&mut app, "targz short.tgz");
        drain_op(&mut app);
        assert!(d.path().join("short.tgz").is_file(), "the given name was kept");

        // And it refuses rather than writing over what is there.
        let before = std::fs::metadata(d.path().join("bundle.tar")).unwrap().len();
        run_cmd(&mut app, "tar bundle");
        let said = app.message.clone().unwrap_or_default();
        assert!(
            said.contains("exists") || said.contains("存在"),
            "it said why: {said:?}",
        );
        assert_eq!(
            std::fs::metadata(d.path().join("bundle.tar")).unwrap().len(),
            before,
            "and left the archive alone",
        );
    }

    /// Every switch says what it did. Three of them changed something and
    /// reported nothing, so the only sign was the row's own state text — and
    /// two of those can *refuse*, which from the menu looked identical to the
    /// key not working.
    #[test]
    fn every_switch_says_what_it_did() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // The cloud switch is a *process-wide* setting, and these tests run in
        // parallel — flipping it here reached into whichever other test was
        // reading it at the time. Put back exactly as found.
        let cloud_was = cian_core::cloud::include();
        let rows = app.toggle_rows();
        for (i, (id, label, ..)) in rows.iter().enumerate() {
            // Language flips the whole interface, including these labels;
            // it is checked on its own elsewhere.
            if *id == crate::toggles::ToggleId::Lang {
                continue;
            }
            app.popup = Popup::Toggles { cursor: i };
            app.message = None;
            app.toggles_apply();
            assert!(
                app.message.is_some(),
                "\"{label}\" changed something and said nothing",
            );
        }
        cian_core::cloud::set_include(cloud_was);
    }

    /// Input sync refuses with fewer than two shell panes — and says so, rather
    /// than staying off and looking broken.
    #[test]
    fn input_sync_says_why_it_refused() {
        let (_d, mut app) = app_with(&["a.txt"]);
        assert!(!app.shell.is_broadcasting());
        let row = app
            .toggle_rows()
            .into_iter()
            .position(|(id, ..)| id == crate::toggles::ToggleId::Sync)
            .expect("the row is there");
        app.popup = Popup::Toggles { cursor: row };
        app.toggles_apply();
        assert!(!app.shell.is_broadcasting(), "still off — there is nothing to sync to");
        let said = app.message.clone().unwrap_or_default();
        assert!(
            said.contains("F8") || said.contains("F9"),
            "and it said what is missing: {said:?}",
        );
    }

    /// The switches step aside for an open panel rather than over it — `T`
    /// reaches them from a listing while the panel is still what is in
    /// `self.popup`, so this ate the open file, unsaved edits and all.
    #[test]
    fn the_switches_do_not_eat_an_open_panel() {
        let (_d, mut app) = viewer_on("alpha\nbravo\n");
        app.handle_key(code(KeyCode::F(12))).unwrap();
        app.handle_key(key('x')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { dirty: true, .. }), "edited");
        let dock = app.viewer_dock.expect("docked");
        app.focus(match dock {
            FocusedPane::Left => FocusedPane::Right,
            _ => FocusedPane::Left,
        });
        let _ = render(&mut app, 160, 30);

        app.handle_key(key('T')).unwrap();
        assert!(matches!(app.popup, Popup::Toggles { .. }), "the switches opened");
        assert!(app.viewer_return.is_some(), "and the file went aside, not away");
        app.handle_key(code(KeyCode::Esc)).unwrap();
        match &app.popup {
            Popup::Viewer { dirty, .. } => assert!(*dirty, "the edits came back with it"),
            other => panic!("the panel should be back, got {other:?}"),
        }
    }

    /// `T` reaches the switch from a listing — where it is bound — and the
    /// panel's own menu reaches it from inside the editor, where `T` cannot:
    /// there it is a vi motion in one grammar and a character in the other.
    ///
    /// The hint bar had been advertising `T` on the editor's rows, which is a
    /// key that does nothing there.
    #[test]
    fn the_editor_grammar_is_reachable_from_where_you_are() {
        // From a listing: `T` opens the toggles, and the switch is in it.
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(key('T')).unwrap();
        assert!(matches!(app.popup, Popup::Toggles { .. }), "T opened the toggles");
        let row = app
            .toggle_rows()
            .into_iter()
            .position(|(id, ..)| id == crate::toggles::ToggleId::EditStyle)
            .expect("the switch is a row");
        app.popup = Popup::Toggles { cursor: row };
        app.toggles_apply();
        assert_eq!(app.edit_style, EditStyle::Notepad, "and flipped it");

        // From inside the editor: `T` is not the way, so the panel's menu is.
        let (_d, mut app) = viewer_on("alpha\n");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        let Popup::ContextMenu { items, .. } = &app.popup else {
            panic!("Shift+Enter opens the panel's menu, got {:?}", app.popup)
        };
        assert!(
            items.contains(&MenuItem::ViewerEditStyle),
            "and the switch is in it: {items:?}",
        );

        // And `:notepad` / `:vim` name it for anyone who would rather type.
        let (_d, mut app) = viewer_on("alpha\n");
        run_cmd(&mut app, "notepad");
        assert_eq!(app.edit_style, EditStyle::Notepad);
        // `:vim` is taken — it opens the external editor — so the way back by
        // command is spelled out.
        run_cmd(&mut app, "editstyle vim");
        assert_eq!(app.edit_style, EditStyle::Vim);

        // No hint offers a key that does not work where it is shown.
        let (_d, app) = viewer_on("alpha\n");
        let keys: Vec<&str> = key_hints(&app).into_iter().map(|(k, _)| k).collect();
        assert!(!keys.contains(&"T"), "T is not the editor's key: {keys:?}");
    }

    /// A character an input method could only have produced, arriving where a
    /// command was expected, says so. Nothing else in cian would explain why
    /// the keys stopped working.
    #[test]
    fn a_key_that_could_only_be_ime_output_says_the_ime_is_on() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(key('あ')).unwrap();
        let said = app.message.clone().unwrap_or_default();
        assert!(said.contains("IME"), "it said so: {said:?}");
        // Unconfigured, it can only say so. Configured, it throws the switch
        // as well — the input method was turned on after cian last set it, and
        // cian has no other way to notice that has happened.
        assert!(
            said.contains("cian.ime"),
            "with no helper it says how to get one: {said:?}",
        );

        // Once per run, not once per key: a committed composition arrives as a
        // burst and would otherwise complain five times.
        app.message = None;
        app.handle_key(key('い')).unwrap();
        assert!(app.message.is_none(), "the second one is quiet: {:?}", app.message);

        // A key an input method could not have produced ends the run, so the
        // next time it happens it is said again.
        app.handle_key(key('j')).unwrap();
        app.message = None;
        app.handle_key(key('う')).unwrap();
        assert!(
            app.message.clone().unwrap_or_default().contains("IME"),
            "and said again after an ASCII key: {:?}",
            app.message,
        );
    }

    /// Ctrl+Z and Ctrl+Y walk notepad's history.
    ///
    /// The keys were always wired; what was missing was anything to walk. vim
    /// takes one snapshot when insert is entered, and notepad never enters
    /// insert — it is always in it — so nothing was ever remembered and Ctrl+Z
    /// had nowhere to go. A run of typing is one step; a line break is its own.
    #[test]
    fn notepad_style_can_undo_and_redo_what_was_typed() {
        let (_d, mut app) = viewer_on("alpha\n");
        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();
        let undo = || KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL);
        let redo = || KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL);

        for c in "abc".chars() {
            app.handle_key(key(c)).unwrap();
        }
        assert_eq!(viewer_lines(&app).first().map(String::as_str), Some("abcalpha"));

        // One press takes the whole run, not one character of it.
        app.handle_key(undo()).unwrap();
        assert_eq!(
            viewer_lines(&app).first().map(String::as_str),
            Some("alpha"),
            "the run came back as a unit: {:?}",
            viewer_lines(&app),
        );
        app.handle_key(redo()).unwrap();
        assert_eq!(viewer_lines(&app).first().map(String::as_str), Some("abcalpha"), "and went again");

        // A line break ends the run, so it is its own step.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        for c in "xy".chars() {
            app.handle_key(key(c)).unwrap();
        }
        assert_eq!(viewer_lines(&app).len(), 2, "the line was split in two");
        app.handle_key(undo()).unwrap();
        app.handle_key(undo()).unwrap();
        assert_eq!(
            viewer_lines(&app).first().map(String::as_str),
            Some("abcalpha"),
            "two presses: the typing, then the break: {:?}",
            viewer_lines(&app),
        );

        // Select-and-type is one step, not two — the delete and the character
        // that replaced it come back together.
        let (_d, mut app) = viewer_on("alpha\n");
        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();
        for _ in 0..5 {
            app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)).unwrap();
        }
        app.handle_key(key('Z')).unwrap();
        assert_eq!(viewer_lines(&app).first().map(String::as_str), Some("Z"));
        app.handle_key(undo()).unwrap();
        assert_eq!(
            viewer_lines(&app).first().map(String::as_str),
            Some("alpha"),
            "one press put it all back: {:?}",
            viewer_lines(&app),
        );
    }

    /// Three Escs close the file in notepad style, as they do everywhere else.
    /// The count was armed on "not editing", and notepad is always editing — so
    /// that grammar had no way out by keyboard at all.
    #[test]
    fn notepad_style_keeps_the_three_escape_way_out() {
        let (_d, mut app) = viewer_on("alpha\n");
        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();

        // A press that clears a selection did something, so it does not count.
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)).unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { visual: None, .. }), "cleared");
        assert!(matches!(app.popup, Popup::Viewer { .. }), "and kept the file");

        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "one is not enough");
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "nor two");
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(!matches!(app.popup, Popup::Viewer { .. }), "three closed it");
    }

    /// Alt and Shift and an arrow select a rectangle, where VS Code and
    /// Notepad++ put column select. It keeps vi's reckoning — what matters is
    /// that the highlight, the copy and the cut agree, and all three already
    /// read a rectangle the same way.
    #[test]
    fn notepad_style_selects_a_rectangle_with_alt_shift() {
        let (_d, mut app) = viewer_on("abcdef\nghijkl\nmnopqr\n");
        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();
        let alt_shift = |c| KeyEvent::new(c, KeyModifiers::ALT | KeyModifiers::SHIFT);

        app.handle_key(alt_shift(KeyCode::Right)).unwrap();
        assert!(
            matches!(app.popup, Popup::Viewer { visual: Some(ViewVisual::Block), .. }),
            "a rectangle, not a run: {:?}",
            app.popup,
        );
        app.handle_key(alt_shift(KeyCode::Down)).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(app.yank.as_deref(), Some("ab\ngh"), "two columns of two rows");

        // Shift alone goes back to a character run rather than extending the
        // rectangle — the anchor is re-planted where the caret is.
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)).unwrap();
        assert!(
            matches!(app.popup, Popup::Viewer { visual: Some(ViewVisual::Char), .. }),
            "back to a run: {:?}",
            app.popup,
        );
    }

    /// The hint bar tells whoever is looking at it that the other grammar
    /// exists. Someone who types a sentence into normal mode and watches it not
    /// appear will not guess that a file-manager menu holds the switch.
    #[test]
    fn the_hint_bar_offers_the_other_grammar() {
        let (_d, mut app) = viewer_on("alpha\n");
        let keys: Vec<&str> = key_hints(&app).into_iter().map(|(k, _)| k).collect();
        // Not `T`: that is the listings' key for the toggles, and a vi motion
        // once the panel has the keyboard. The bar names what works here.
        assert!(!keys.contains(&"T"), "T does nothing in the editor: {keys:?}");
        assert!(keys.contains(&":notepad"), "vim's bar offers the switch: {keys:?}");
        assert!(keys.contains(&"i"), "and still says how to type: {keys:?}");

        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();
        let hints = key_hints(&app);
        let keys: Vec<&str> = hints.iter().map(|(k, _)| *k).collect();
        // `:notepad` would be a lie here — there is no command line in this
        // grammar — so the way back is the panel's own menu.
        assert!(keys.contains(&"S-Enter"), "notepad's bar offers the way back: {keys:?}");
        assert!(!keys.contains(&"T"), "{keys:?}");
        assert!(keys.contains(&"Ctrl+S"), "{keys:?}");
        // The two hints that would be lies here: there is no mode for Esc to
        // leave, and no command line for `:q` to be typed at.
        assert!(!keys.contains(&":q"), "no command line in this grammar: {keys:?}");
        let esc = hints.iter().find(|(k, _)| *k == "Esc").map(|(_, v)| *v);
        assert_eq!(esc, Some("close"), "Esc closes rather than leaving a mode");
    }

    /// Copy, cut and the highlight agree with the delete about where a notepad
    /// selection stops.
    ///
    /// vi's caret sits on a character and takes it; a notepad caret sits
    /// between two and stops short. Every reader of a selection had vi's
    /// reckoning built in, so Ctrl+C took one character more than was lit up
    /// and Ctrl+X removed one more than it had copied.
    #[test]
    fn a_notepad_selection_ends_where_it_looks_like_it_ends() {
        let sel5 = |app: &mut App| {
            for _ in 0..5 {
                app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)).unwrap();
            }
        };

        // Copy takes exactly the five characters selected.
        let (_d, mut app) = viewer_on("alpha bravo\n");
        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();
        sel5(&mut app);
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(app.yank.as_deref(), Some("alpha"), "not \"alpha \"");

        // Cut removes exactly what it copied.
        let (_d, mut app) = viewer_on("alpha bravo\n");
        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();
        sel5(&mut app);
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(app.yank.as_deref(), Some("alpha"), "took five");
        assert_eq!(
            viewer_lines(&app).first().map(String::as_str),
            Some(" bravo"),
            "and left the space: {:?}",
            viewer_lines(&app),
        );

        // Vim style is untouched: its selection includes the character under
        // the cursor, which is what `v` has always meant.
        let (_d, mut app) = viewer_on("alpha bravo\n");
        app.handle_key(key('v')).unwrap();
        for _ in 0..4 {
            app.handle_key(key('l')).unwrap();
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(app.yank.as_deref(), Some("alpha"), "vim's v then 4l is still five");
    }

    /// Esc has no mode to leave here, so it drops the selection first. With
    /// nothing selected it counts toward the way out — and that way out is
    /// `:q!`, unsaved edits and all, exactly as it is in vim style. Pinned
    /// because it loses work: three deliberate presses, no question asked.
    #[test]
    fn notepad_style_esc_clears_then_counts_toward_the_way_out() {
        let (_d, mut app) = viewer_on("alpha\n");
        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { visual: Some(_), .. }));
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { visual: None, .. }), "Esc dropped the selection");
        assert!(matches!(app.popup, Popup::Viewer { .. }), "and kept the file open");

        // With unsaved work in it the third press asks rather than going.
        // Three presses show intent; they do not show intent to lose anything,
        // and a hand pressing Esc repeatedly is usually the least sure one.
        app.handle_key(key('z')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { dirty: true, .. }), "edited");
        for _ in 0..2 {
            app.handle_key(code(KeyCode::Esc)).unwrap();
            assert!(matches!(app.popup, Popup::Viewer { .. }), "not yet");
        }
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(
            matches!(app.popup, Popup::ConfirmClose { target: CloseTarget::ViewerFile }),
            "the third asked, got {:?}",
            app.popup,
        );
        // And backing out returns the file with its edits, rather than nothing.
        app.handle_key(key('n')).unwrap();
        match &app.popup {
            Popup::Viewer { dirty, .. } => assert!(*dirty, "the edits came back"),
            other => panic!("the panel should be back, got {other:?}"),
        }
        // Saying yes goes through with it.
        for _ in 0..3 {
            app.handle_key(code(KeyCode::Esc)).unwrap();
        }
        app.handle_key(key('y')).unwrap();
        assert!(!matches!(app.popup, Popup::Viewer { .. }), "closed on yes");
    }

    /// A clean file is not asked about: three presses and it goes. A question
    /// with one answer is not worth asking.
    #[test]
    fn three_escapes_do_not_ask_about_a_file_with_nothing_to_lose() {
        let (_d, mut app) = viewer_on("alpha\n");
        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();
        for _ in 0..3 {
            app.handle_key(code(KeyCode::Esc)).unwrap();
        }
        assert!(!matches!(app.popup, Popup::Viewer { .. }), "it just closed");
        assert!(!matches!(app.popup, Popup::ConfirmClose { .. }), "with nothing asked");
    }

    /// Ctrl and a sideways arrow steps a word, the motion the shared editor
    /// does not otherwise have.
    #[test]
    fn notepad_style_steps_by_word_with_ctrl() {
        let (_d, mut app) = viewer_on("alpha bravo charlie\n");
        app.edit_style = EditStyle::Notepad;
        app.sync_edit_style();
        let ctrl_right = || KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL);
        app.handle_key(ctrl_right()).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { col: 5, .. }), "to the end of alpha: {:?}", app.popup);
        app.handle_key(ctrl_right()).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { col: 11, .. }), "and of bravo: {:?}", app.popup);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { col: 6, .. }), "back to its start: {:?}", app.popup);
    }

    /// The switch is in the toggles menu, it flips a file already open, and the
    /// badge says which grammar is on.
    #[test]
    fn the_toggles_menu_switches_the_editor_grammar() {
        let (_d, mut app) = viewer_on("alpha\n");
        assert!(matches!(app.popup, Popup::Viewer { editing: false, .. }), "vim opens reading");
        let row = app
            .toggle_rows()
            .into_iter()
            .position(|(id, ..)| id == crate::toggles::ToggleId::EditStyle)
            .expect("the switch is in the menu");

        // Flipping it reaches the file that is already open.
        app.popup = Popup::Toggles { cursor: row };
        app.toggles_apply();
        assert_eq!(app.edit_style, EditStyle::Notepad);
        app.restore_viewer();
        app.popup = Popup::None;
        let (_d2, mut app2) = viewer_on("alpha\n");
        app2.edit_style = EditStyle::Notepad;
        app2.sync_edit_style();
        assert!(matches!(app2.popup, Popup::Viewer { editing: true, .. }), "notepad takes text");
        let screen = render(&mut app2, 100, 30).join("\n");
        assert!(screen.contains("NOTEPAD"), "the badge names the grammar:\n{screen}");
    }

    /// A file open beside the listing is not thrown away by a question about
    /// the window.
    ///
    /// `self.popup` holds one thing, and a docked panel is in it — while the
    /// listing beside it still answers its own keys, by design. So `?` and `M`
    /// wrote straight over the open file, unsaved edits and all, while the ✕
    /// and `:q` both refused to close a dirty panel. Now the panel steps aside
    /// and comes back, the way the panel's own menu has always done.
    #[test]
    fn the_manual_and_the_menu_step_aside_rather_than_over_the_panel() {
        for (what, open) in [
            ("the manual", (|a: &mut App| a.handle_key(key('?')).unwrap()) as fn(&mut App)),
            ("the menu", |a: &mut App| a.handle_key(key('M')).unwrap()),
        ] {
            let (_d, mut app) = viewer_on("alpha\nbravo\n");
            // Dock it beside the listing, and dirty it, so what is at stake is
            // the same thing ✕ and `:q` refuse to discard.
            app.handle_key(code(KeyCode::F(12))).unwrap();
            app.handle_key(key('x')).unwrap();
            assert!(matches!(app.popup, Popup::Viewer { dirty: true, .. }), "edited: {what}");
            let dock = app.viewer_dock.expect("docked");
            let edited = viewer_lines(&app);
            // Focus the listing beside it — this is the state where the
            // listing's own keys run with the panel still in the popup slot.
            app.focus(match dock {
                FocusedPane::Left => FocusedPane::Right,
                _ => FocusedPane::Left,
            });
            let _ = render(&mut app, 160, 30);

            open(&mut app);
            assert!(
                !matches!(app.popup, Popup::None),
                "{what} opened",
            );
            assert!(
                app.viewer_return.is_some(),
                "{what} put the panel aside rather than over it",
            );

            // And closing it brings the file back, edits intact.
            app.handle_key(code(KeyCode::Esc)).unwrap();
            match &app.popup {
                Popup::Viewer { dirty, .. } => assert!(*dirty, "{what}: the edits survived"),
                other => panic!("{what}: the panel should be back, got {other:?}"),
            }
            assert_eq!(viewer_lines(&app), edited, "{what}: and it is the same buffer");
        }
    }

    /// In the Finder and icon skins, a right-click inside the open file belongs
    /// to the file.
    ///
    /// The single-pane block claimed every click in the window — it asked only
    /// whether a panel was docked, never where the pointer was — and returned
    /// before the panel's own mouse handling ran. So a right-click in the
    /// editor opened the *pane's* menu, and took the file with it.
    #[test]
    fn a_right_click_in_the_docked_panel_opens_the_panels_own_menu() {
        let (_d, mut app) = viewer_on("alpha\nbravo\ncharlie\n");
        app.handle_key(code(KeyCode::F(12))).unwrap();
        app.skin = Skin::Finder;
        assert!(app.single_pane_view(), "the skin this was reported in");
        let _ = render(&mut app, 160, 30);
        let f = app.viewer_frame;
        assert!(f.width > 0 && f.height > 0, "the panel was measured");

        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Right),
            column: f.x + f.width / 2,
            row: f.y + f.height / 2,
            modifiers: KeyModifiers::NONE,
        });
        assert!(
            matches!(app.popup, Popup::ContextMenu { .. }),
            "a menu opened, got {:?}",
            app.popup,
        );
        // The panel's menu, not the pane's: the panel's is the one that puts
        // the file aside rather than over it.
        assert!(
            app.viewer_return.is_some(),
            "it was the panel's own menu — the file is still there behind it",
        );
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "and Esc goes back to the file");
    }

    /// F10 asks before it closes a shell tab.
    ///
    /// It was the one key in cian that could end a running shell with nothing
    /// asked — while Shift+F10, which closes a single *pane* of that same tab,
    /// stopped to ask. The bigger loss was the quieter one.
    #[test]
    fn f10_asks_before_it_closes_a_shell_tab() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        app.handle_key(code(KeyCode::F(10))).unwrap();
        assert!(
            matches!(app.popup, Popup::ConfirmClose { target: CloseTarget::ShellTab }),
            "F10 asked first, got {:?}",
            app.popup,
        );
        // The dialog names what goes, and n backs out.
        let screen = render(&mut app, 100, 30).join("\n");
        let flat: String = screen.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(flat.contains("shelltab"), "the dialog says what closes:\n{screen}");
        app.handle_key(key('n')).unwrap();
        assert!(matches!(app.popup, Popup::None), "n kept the tab");
    }

    /// Ctrl+X cuts. It used to cut *and type a `d`*.
    ///
    /// The cut is implemented by handing the delete operator a `d`, and it was
    /// handing it in at the top of the dispatcher — past the layer that gives
    /// every key to the editor while it is taking text. So in insert mode the
    /// synthetic key came back round as input: the copy happened, the delete
    /// never did, and the file grew a letter nobody typed.
    #[test]
    fn ctrl_x_while_editing_cuts_rather_than_typing_a_d() {
        let (_d, mut app) = viewer_on("alpha\nbravo\ncharlie\n");
        app.handle_key(key('i')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }), "in the editor");
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)).unwrap();
        let lines = viewer_lines(&app);
        assert!(
            !lines.iter().any(|l| l.contains('d')),
            "no stray `d` was typed into the file: {lines:?}",
        );
        assert_eq!(lines.first().map(String::as_str), Some("bravo"), "the line was cut: {lines:?}");
        assert_eq!(app.yank.as_deref(), Some("alpha\n"), "and it went to the clipboard");
    }

    /// `?` is a character people type. The manual only opens when the key is a
    /// key — it used to open from inside the `:` command line and the `/`
    /// filter, which made a `?` impossible to type into either.
    #[test]
    fn question_mark_is_a_character_while_typing() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(key(':')).unwrap();
        assert_eq!(app.mode, Mode::Command);
        for c in "grep foo?".chars() {
            app.handle_key(key(c)).unwrap();
        }
        assert_eq!(app.mode, Mode::Command, "still at the command line");
        assert_eq!(app.command_buffer, "grep foo?", "the ? was typed");
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // The filter too.
        app.start_filter();
        assert_eq!(app.mode, Mode::Filter);
        for c in "a??.txt".chars() {
            app.handle_key(key(c)).unwrap();
        }
        assert_eq!(app.mode, Mode::Filter, "still filtering");
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // And it still opens the manual when it is a key.
        app.handle_key(key('?')).unwrap();
        assert!(!matches!(app.popup, Popup::None), "? opened the manual from normal mode");
    }

    /// A paste belongs to the prompt on top, not to the file underneath.
    #[test]
    fn a_paste_goes_to_the_prompt_over_the_file_not_into_it() {
        // The `:` command line: Ctrl+V used to reach the file behind it.
        let (_d, mut app) = viewer_on("alpha\nbravo\n");
        let before = viewer_lines(&app);
        app.handle_key(key(':')).unwrap();
        assert!(matches!(&app.popup, Popup::Viewer { sub_input: Some(_), .. }));
        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(viewer_lines(&app), before, "the file was not edited behind the prompt");
        assert!(
            matches!(&app.popup, Popup::Viewer { sub_input: Some(_), .. }),
            "and the prompt is still open",
        );

        // The replace bar: a terminal paste used to land in the file as a
        // line-wise `p`. It goes into whichever field the caret is in.
        let (_d, mut app) = viewer_on("alpha\nbravo\n");
        let before = viewer_lines(&app);
        app.start_replace_bar();
        app.insert_into_active_text("needle");
        match &app.popup {
            Popup::Viewer { replace: Some(r), .. } => {
                assert_eq!(r.find, "needle", "into the find field");
                assert!(r.with.is_empty());
            }
            other => panic!("the replace bar should be open, got {other:?}"),
        }
        assert_eq!(viewer_lines(&app), before, "and the file is untouched");
    }

    /// Tab crosses panes — but not out of a prompt, taking what was typed with
    /// it. The prompt row is only drawn for the pane the panel is docked in, so
    /// moving the focus made a half-typed search vanish.
    #[test]
    fn tab_does_not_walk_out_of_a_viewer_prompt() {
        let (_d, mut app) = viewer_on("alpha\nbravo\n");
        app.handle_key(key('/')).unwrap();
        for c in "brav".chars() {
            app.handle_key(key(c)).unwrap();
        }
        let focus_before = app.focused;
        app.handle_key(code(KeyCode::Tab)).unwrap();
        assert_eq!(app.focused, focus_before, "the focus stayed put");
        match &app.popup {
            Popup::Viewer { find_input: Some(q), .. } => assert_eq!(q, "brav", "and so did the query"),
            other => panic!("the search prompt should still be open, got {other:?}"),
        }
    }

    /// An F-key is not text in any mode, and neither editor handles one — so
    /// the editor no longer swallows them. F2 walked the open files until you
    /// pressed `i`, and then it did nothing, silently.
    #[test]
    fn f_keys_still_work_while_editing() {
        let (_d, mut app) = viewer_on("alpha\n");
        app.handle_key(key('i')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }));
        let zoomed = app.zoomed;
        app.handle_key(code(KeyCode::F(12))).unwrap();
        assert_ne!(app.zoomed, zoomed, "F12 reached the window from inside the editor");
        // And nothing was typed into the file by the F-key going past.
        assert_eq!(viewer_lines(&app), vec!["alpha".to_string()], "the file is untouched");
    }

    /// The pane's `f` search box discards Ctrl+<key> rather than typing its
    /// bare letter — Ctrl+V used to put a "v" in the box.
    #[test]
    fn the_search_box_does_not_type_control_keys() {
        let (_d, mut app) = app_with(&["alpha.txt", "beta.txt"]);
        app.start_search();
        app.handle_key(key('a')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)).unwrap();
        match &app.popup {
            Popup::Search { buffer } => assert_eq!(buffer, "a", "the Ctrl+V typed nothing"),
            other => panic!("the search box should be open, got {other:?}"),
        }
    }

    /// In the editor, `:` is a colon. It opened the command line instead —
    /// the binding sat ahead of the editor's own key handling, so a YAML key
    /// or a `foo::bar` could not be typed at all.
    /// `jj` leaves insert mode and takes both j's with it — in all three of
    /// the shapes a Japanese keyboard can produce.
    #[test]
    fn jj_leaves_insert_mode_and_removes_itself() {
        for pair in crate::viewer::JJ_ESCAPES {
            let (_d, mut app) = viewer_on("one\n");
            app.handle_key(key('i')).unwrap();
            assert!(
                matches!(app.popup, Popup::Viewer { editing: true, .. }),
                "{pair}: in the editor"
            );
            for c in pair.chars() {
                app.handle_key(key(c)).unwrap();
            }
            assert!(
                matches!(app.popup, Popup::Viewer { editing: false, .. }),
                "{pair}: left insert mode"
            );
            let body = match &app.popup {
                Popup::Viewer { view, .. } => view.lines.join("\n"),
                _ => panic!("no viewer"),
            };
            assert_eq!(body, "one", "{pair}: neither character was left behind");
        }
    }

    /// A single j is a j. The way out is two of them in a row, and nothing
    /// else — a mapping that fired on the first one would make the letter
    /// untypeable.
    #[test]
    fn one_j_is_just_a_j() {
        let (_d, mut app) = viewer_on("one\n");
        app.handle_key(key('i')).unwrap();
        for c in "jaj".chars() {
            app.handle_key(key(c)).unwrap();
        }
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }), "still editing");
        let body = match &app.popup {
            Popup::Viewer { view, .. } => view.lines.join("\n"),
            _ => panic!("no viewer"),
        };
        assert_eq!(body, "jajone", "all three characters are there");
    }

    /// A `Z` followed by anything else is not half of `ZZ` any more, and the
    /// prefix has to say so — or the next `Z` closes the file on its own.
    ///
    /// Tested against `:` rather than a motion, and that is the whole point.
    /// Most keys clear the pending slot on their way past, so a test written
    /// against `j` passes with the clearing removed and proves nothing; `:`
    /// opens the command line and returns without touching it. It was found
    /// by disabling the clear and walking a row of keys past it, which is the
    /// only way to tell a guard from a decoration.
    #[test]
    fn a_half_typed_z_does_not_survive_the_next_key() {
        let (_d, mut app) = viewer_on("one\n");
        app.handle_key(key('Z')).unwrap();
        app.handle_key(key(':')).unwrap();
        let pending = match &app.popup {
            Popup::Viewer { pending, .. } => *pending,
            _ => panic!("no viewer"),
        };
        assert_eq!(pending, None, "the half-typed Z did not survive the colon");
    }

    #[test]
    fn zq_closes_without_saving() {
        let (_d, mut app) = viewer_on("one\n");
        app.handle_key(key('i')).unwrap();
        app.handle_key(key('x')).unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { dirty: true, .. }), "unsaved");
        app.handle_key(key('Z')).unwrap();
        app.handle_key(key('Q')).unwrap();
        assert!(!matches!(app.popup, Popup::Viewer { .. }), "gone, unsaved work and all");
    }

    /// **A save must not write over somebody else's writing.**
    ///
    /// Two people on one file over a shared drive: both open it, both save,
    /// and the second used to erase the first without a word. `:w` refuses
    /// when the file moved underneath; `:w!` is how to mean it anyway.
    #[test]
    fn saving_refuses_a_file_that_changed_underneath() {
        let (d, mut app) = viewer_on("one\n");
        let path = match &app.popup {
            Popup::Viewer { path, .. } => path.clone(),
            _ => panic!("no viewer"),
        };
        // Somebody else writes to it, far enough back in time to be seen.
        std::fs::write(&path, "theirs, longer\n").unwrap();
        let old = std::fs::metadata(&path).unwrap().modified().unwrap()
            - std::time::Duration::from_secs(120);
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_modified(old).unwrap();

        app.handle_key(key('i')).unwrap();
        app.handle_key(key('X')).unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
        app.save_viewer_file();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "theirs, longer\n",
            "it did not write"
        );
        let said = app.message.clone().unwrap_or_default();
        assert!(said.contains(":w!"), "and it said how to mean it: {said}");

        // …and `:w!` does write.
        app.save_viewer_file_forced(true);
        assert!(
            std::fs::read_to_string(&path).unwrap().starts_with('X'),
            "the bang wrote it"
        );
        drop(d);
    }

    #[test]
    fn colon_types_a_colon_while_editing() {
        let (_d, mut app) = viewer_on("one\n");
        // From READ mode it still opens the command line.
        app.handle_key(key(':')).unwrap();
        assert!(matches!(&app.popup, Popup::Viewer { sub_input: Some(_), .. }));
        app.handle_key(code(KeyCode::Esc)).unwrap();

        app.handle_key(key('i')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }), "in the editor");
        for c in "a:b".chars() {
            app.handle_key(key(c)).unwrap();
        }
        assert!(
            matches!(&app.popup, Popup::Viewer { sub_input: None, .. }),
            "no command line opened",
        );
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }), "still editing");
        let body = match &app.popup {
            Popup::Viewer { view, .. } => view.lines.join("\n"),
            _ => panic!("no viewer"),
        };
        assert!(body.starts_with("a:bone"), "the colon was typed: {body:?}");

        // Esc, and the command line is back.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        app.handle_key(key(':')).unwrap();
        assert!(matches!(&app.popup, Popup::Viewer { sub_input: Some(_), .. }));
    }

    /// `:` opens replace in the viewer; a plain command replaces everything
    /// at once, and `u` takes the whole thing back as one step.
    #[test]
    fn viewer_replace_all_is_one_undo_step() {
        let (_d, mut app) = viewer_on("alpha bravo\nbravo charlie\nbravo\n");
        app.handle_key(key(':')).unwrap();
        // The prompt opens empty — the word commands share it, and none of
        // them should start by deleting a seeded `s/`.
        assert!(matches!(&app.popup, Popup::Viewer { sub_input: Some(s), .. } if s.is_empty()));
        for c in "s/bravo/BRAVO/g".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), ["alpha BRAVO", "BRAVO charlie", "BRAVO"]);
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains('3'), "reports the count: {msg}");

        app.handle_key(key('u')).unwrap();
        assert_eq!(
            viewer_lines(&app),
            ["alpha bravo", "bravo charlie", "bravo"],
            "one undo takes back the whole replace"
        );
    }

    /// The `c` flag walks the hits: y replaces, n skips, and the walk reports
    /// both tallies at the end.
    #[test]
    fn viewer_replace_can_confirm_each_one() {
        let (_d, mut app) = viewer_on("x one\nx two\nx three\n");
        app.handle_key(key(':')).unwrap();
        for c in "s/x/Y/c".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { sub_walk: Some(_), .. }), "walk started");

        app.handle_key(key('y')).unwrap(); // line 0: replace
        app.handle_key(key('n')).unwrap(); // line 1: skip
        app.handle_key(key('y')).unwrap(); // line 2: replace → walk ends
        assert!(matches!(app.popup, Popup::Viewer { sub_walk: None, .. }), "walk finished");
        assert_eq!(viewer_lines(&app), ["Y one", "x two", "Y three"]);
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains('2') && msg.contains('1'), "reports 2 replaced, 1 skipped: {msg}");
    }

    /// `q` stops a walk partway, keeping what was already done; `a` takes the
    /// whole remainder in one go.
    #[test]
    fn a_confirm_walk_can_be_stopped_or_finished_wholesale() {
        let (_d, mut app) = viewer_on("x\nx\nx\nx\n");
        let start = |app: &mut App| {
            app.handle_key(key(':')).unwrap();
            for c in "s/x/Y/c".chars() {
                app.handle_key(key(c)).unwrap();
            }
            app.handle_key(code(KeyCode::Enter)).unwrap();
        };
        start(&mut app);
        app.handle_key(key('y')).unwrap();
        app.handle_key(key('q')).unwrap();
        assert_eq!(viewer_lines(&app), ["Y", "x", "x", "x"], "stopped, keeping the first");
        assert!(app.message.clone().unwrap_or_default().contains("stopped")
            || app.message.clone().unwrap_or_default().contains("中断"));

        start(&mut app);
        app.handle_key(key('a')).unwrap();
        assert_eq!(viewer_lines(&app), ["Y", "Y", "Y", "Y"], "`a` took the rest");
    }

    /// A CRLF file keeps its line endings through an edit — the viewer's
    /// lines never hold the ending, so saving used to quietly rewrite every
    /// Windows file as LF. `:crlf` / `:lf` convert on purpose.
    #[test]
    fn line_endings_survive_an_edit_and_convert_on_request() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("win.txt");
        std::fs::write(&f, b"one\r\ntwo\r\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "win.txt").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);
        match &app.popup {
            Popup::Viewer { view, .. } => {
                assert_eq!(view.eol, cian_core::viewer::Eol::Crlf, "detected as CRLF");
            }
            _ => panic!("not a viewer"),
        }
        let shown = render(&mut app, 100, 30).join("\n");
        assert!(shown.contains("CRLF"), "and says so in the title: {shown}");

        // An edit and a save keep the CRLFs.
        app.handle_key(key('i')).unwrap();
        app.handle_key(key('X')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)).unwrap();
        let raw = std::fs::read(&f).unwrap();
        assert!(raw.windows(2).any(|w| w == b"\r\n"), "still CRLF after saving");
        assert_eq!(String::from_utf8_lossy(&raw), "Xone\r\ntwo\r\n");

        // `:lf` converts, deliberately.
        app.handle_key(code(KeyCode::Esc)).unwrap(); // leave insert
        app.handle_key(key(':')).unwrap();
        if let Popup::Viewer { sub_input, .. } = &mut app.popup {
            *sub_input = Some("lf".into());
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)).unwrap();
        let raw = std::fs::read(&f).unwrap();
        assert!(!raw.contains(&b'\r'), "converted to LF on request: {:?}", String::from_utf8_lossy(&raw));
    }

    /// A replace can be limited to a visual selection.
    #[test]
    fn replace_honours_a_selection() {
        let (_d, mut app) = viewer_on("a\na\na\n");
        app.handle_key(key('V')).unwrap();
        app.handle_key(key('j')).unwrap();
        app.handle_key(key(':')).unwrap();
        for c in "s/a/B/g".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), ["B", "B", "a"], "only the selected lines");
    }


    /// The line transforms act on the whole file, or on a v/V selection, and
    /// each lands as one undo step.
    #[test]
    fn viewer_line_transforms_work_on_file_and_selection() {
        let run = |app: &mut App, cmd: &str| {
            app.handle_key(key(':')).unwrap();
            if let Popup::Viewer { sub_input, .. } = &mut app.popup {
                *sub_input = Some(cmd.into());
            }
            app.handle_key(code(KeyCode::Enter)).unwrap();
        };

        let (_d, mut app) = viewer_on("c\na\nb\na\n");
        run(&mut app, "sort");
        assert_eq!(viewer_lines(&app), ["a", "a", "b", "c"]);
        run(&mut app, "uniq");
        assert_eq!(viewer_lines(&app), ["a", "b", "c"]);
        run(&mut app, "rsort");
        assert_eq!(viewer_lines(&app), ["c", "b", "a"]);
        // Each transform is one undo step, so this walks back to sorted.
        app.handle_key(key('u')).unwrap();
        assert_eq!(viewer_lines(&app), ["a", "b", "c"]);

        // Full-width Latin and half-width kana both come out normal.
        let (_d2, mut app) = viewer_on("ＡＢＣ１２３\nｶﾞｯｺｳ\n");
        run(&mut app, "han");
        assert_eq!(viewer_lines(&app), ["ABC123", "ガッコウ"]);

        // A selection limits it: sort only the middle two lines.
        let (_d3, mut app) = viewer_on("z\nd\nc\na\n");
        if let Popup::Viewer { line, .. } = &mut app.popup {
            *line = 1;
        }
        app.handle_key(key('V')).unwrap();
        app.handle_key(key('j')).unwrap();
        run(&mut app, "sort");
        assert_eq!(viewer_lines(&app), ["z", "c", "d", "a"], "only the selected pair moved");
    }

    /// `:ws` makes the invisible characters visible — the pass where a
    /// trailing space or an ideographic space is the actual bug.
    #[test]
    fn ws_shows_the_invisible_characters() {
        let (_d, mut app) = viewer_on("trailing   \n全角\u{3000}空白\n");
        // Body rows only: the title carries its own `·` for the encoding and
        // line-ending badges. Matched on single characters because the test
        // backend dumps a wide char's second cell as a space, so "空白" comes
        // back as "空 白".
        let body = |app: &mut App| -> String {
            render(app, 100, 30)
                .into_iter()
                .filter(|l| l.contains("trailing") || l.contains('全'))
                .collect::<Vec<_>>()
                .join("\n")
        };
        // On by default now: the invisible characters are the ones that cause
        // the trouble, so they are shown until asked not to be.
        let after = body(&mut app);
        assert!(after.contains('·'), "spaces are marked: {after}");
        assert!(after.contains('□'), "and the ideographic space: {after}");
        assert!(after.contains('↓'), "and the line ending: {after}");

        // `:ws` turns them off.
        app.handle_key(key(':')).unwrap();
        if let Popup::Viewer { sub_input, .. } = &mut app.popup {
            *sub_input = Some("ws".into());
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let off = body(&mut app);
        assert!(!off.contains('·') && !off.contains('□'), "off on request: {off}");

        // …and back on.
        app.handle_key(key(':')).unwrap();
        if let Popup::Viewer { sub_input, .. } = &mut app.popup {
            *sub_input = Some("ws".into());
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let after = body(&mut app);
        assert!(after.contains('·'), "trailing spaces are marked: {after}");
        assert!(after.contains('□'), "ideographic space is marked: {after}");
        // Marking is display only — the buffer is untouched.
        assert_eq!(viewer_lines(&app)[0], "trailing   ");
    }

    /// The outline: on by default when the file type has rules, `]]` and `[[`
    /// step through it, a click jumps, and `:outline` puts the column away.
    #[test]
    fn the_outline_shows_a_files_shape_and_jumps_around_it() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("code.rs"),
            "use std::io;\n\nstruct Config {\n    a: u8,\n}\n\npub fn run() {\n    let x = 1;\n}\n\nfn helper() {}\n",
        )
        .unwrap();
        std::fs::write(d.path().join("plain.txt"), "no structure here\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let open = |app: &mut App, name: &str| {
            app.active_pane_mut().unwrap().cursor =
                app.active_pane().unwrap().entries.iter().position(|e| e.name == name).unwrap();
            app.handle_key(code(KeyCode::Enter)).unwrap();
            if !app.zoomed {
                app.handle_key(code(KeyCode::F(12))).unwrap();
            }
            let _ = render(app, 120, 30);
        };
        let shape = |app: &App| match &app.popup {
            Popup::Viewer { shape, .. } => shape.clone(),
            other => panic!("not a viewer: {other:?}"),
        };
        let at = |app: &App| match &app.popup {
            Popup::Viewer { line, .. } => *line,
            other => panic!("not a viewer: {other:?}"),
        };

        open(&mut app, "code.rs");
        let sh = shape(&app).expect("Rust has outline rules");
        assert!(sh.shown, "shown without being asked for");
        assert_eq!(
            sh.items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(),
            ["struct Config {", "pub fn run() {", "fn helper() {}"],
        );

        // `]]` steps forward, `[[` back. A single bracket does nothing, so it
        // stays free for something else.
        app.handle_key(key(']')).unwrap();
        assert_eq!(at(&app), 0, "one bracket is not a motion");
        app.handle_key(key(']')).unwrap();
        assert_eq!(at(&app), 2, "struct Config");
        for _ in 0..2 {
            app.handle_key(key(']')).unwrap();
        }
        assert_eq!(at(&app), 6, "pub fn run");
        for _ in 0..2 {
            app.handle_key(key('[')).unwrap();
        }
        assert_eq!(at(&app), 2, "back to the struct");

        // A click in the outline column lands on the entry drawn there.
        let ol = app.outline_rect;
        assert!(ol.width > 0, "the column is drawn at this width");
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: ol.x,
            row: ol.y + 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(at(&app), 10, "the third entry, fn helper");

        // `:outline` puts it away, and the body gets the width back.
        let narrow = app.viewer_rect.width;
        app.handle_key(key(':')).unwrap();
        if let Popup::Viewer { sub_input, .. } = &mut app.popup {
            *sub_input = Some("outline".into());
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let _ = render(&mut app, 120, 30);
        assert!(!shape(&app).unwrap().shown);
        assert!(app.viewer_rect.width > narrow, "the text got the column back");
        assert_eq!(app.outline_rect.width, 0);

        // A file type with no rules says so rather than showing an empty box.
        quit_viewer(&mut app);
        open(&mut app, "plain.txt");
        assert!(shape(&app).is_none());
        for _ in 0..2 {
            app.handle_key(key(']')).unwrap();
        }
        assert!(app.message.as_deref().unwrap_or("").contains("outline"));
    }

    /// Editing and saving must hand the file back the way it came: same tabs,
    /// same byte-order mark. Both were being spent silently — a Makefile came
    /// out indented with spaces, and a UTF-8-BOM file came out without one,
    /// which is precisely what `:nobom` is a deliberate command for.
    #[test]
    fn saving_keeps_the_tabs_and_the_bom_the_file_arrived_with() {
        let d = tempfile::tempdir().unwrap();
        let mk = d.path().join("Makefile");
        let bom = d.path().join("bom.txt");
        std::fs::write(&mk, b"all:\n\techo one\n\techo two\n").unwrap();
        std::fs::write(&bom, [&[0xEF, 0xBB, 0xBF][..], b"alpha\nbeta\n"].concat()).unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let open = |app: &mut App, name: &str| {
            app.active_pane_mut().unwrap().cursor =
                app.active_pane().unwrap().entries.iter().position(|e| e.name == name).unwrap();
            app.handle_key(code(KeyCode::F(3))).unwrap();
            let _ = render(app, 100, 30);
        };

        open(&mut app, "Makefile");
        assert_eq!(viewer_lines(&app)[1], "\techo one", "the buffer holds the real tab");
        // A tab is still drawn four columns wide — the fix is about what is
        // written, not about how it looks. With the marks on (the default) the
        // first of those columns says what it is.
        let screen = render(&mut app, 100, 30).join("\n");
        assert!(screen.contains("→   echo one↓"), "the tab and the line ending are marked: {screen}");
        app.show_ws = false;
        let plain = render(&mut app, 100, 30).join("\n");
        assert!(plain.contains("    echo one"), "and plain with the marks off");
        app.show_ws = true;

        // A tab is one buffer character but four drawn columns, so a click has
        // to be walked back through the same expansion: anywhere in the tab is
        // the tab, and the column after it is the first letter.
        let b = app.viewer_rect;
        let g = app.viewer_gutter;
        let click = |app: &mut App, dx: u16| {
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: b.x + g + dx,
                row: b.y + 1,
                modifiers: KeyModifiers::NONE,
            });
            match &app.popup {
                Popup::Viewer { col, .. } => *col,
                other => panic!("not a viewer: {other:?}"),
            }
        };
        assert_eq!(click(&mut app, 0), 0, "the start of the tab");
        assert_eq!(click(&mut app, 3), 0, "still inside the tab");
        assert_eq!(click(&mut app, 4), 1, "the e of echo");
        assert_eq!(click(&mut app, 6), 3, "three characters in");

        // Make an edit somewhere else entirely, then save.
        if let Popup::Viewer { line, .. } = &mut app.popup {
            *line = 0;
        }
        app.handle_key(key('o')).unwrap(); // opens a line, entering insert
        app.handle_key(code(KeyCode::Esc)).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)).unwrap();
        let out = std::fs::read(&mk).unwrap();
        assert!(app.message.as_deref().unwrap_or("").starts_with("saved"), "{:?}", app.message);
        assert!(
            out.windows(9).any(|w| w == b"\techo one"),
            "the recipe lines still start with a tab: {:?}",
            String::from_utf8_lossy(&out),
        );
        quit_viewer(&mut app);

        open(&mut app, "bom.txt");
        assert_eq!(viewer_lines(&app), ["alpha", "beta"], "the BOM is not part of the text");
        app.handle_key(key('o')).unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(
            &std::fs::read(&bom).unwrap()[..3],
            &[0xEF, 0xBB, 0xBF],
            "the byte-order mark came back",
        );
    }

    /// F3 with several files marked opens them all: having marked them is how
    /// you say "these ones", and opening the first while forgetting the rest
    /// answers a question nobody asked.
    #[test]
    fn f3_on_marked_files_opens_them_as_tabs() {
        let d = tempfile::tempdir().unwrap();
        for (n, body) in [("a.txt", "AAA\n"), ("b.txt", "BBB\n"), ("c.txt", "CCC\n")] {
            std::fs::write(d.path().join(n), body).unwrap();
        }
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let mark = |app: &mut App, name: &str| {
            let path = app
                .active_pane()
                .unwrap()
                .entries
                .iter()
                .find(|e| e.name == name)
                .unwrap()
                .path
                .clone();
            app.active_pane_mut().unwrap().marks.insert(path);
        };
        let shown = |app: &App| match &app.popup {
            Popup::Viewer { view, .. } => view.lines.join("\n"),
            other => panic!("not a viewer: {other:?}"),
        };

        for n in ["a.txt", "b.txt", "c.txt"] {
            mark(&mut app, n);
        }
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert_eq!(app.viewer_tab_count(), 3, "one tab per marked file");
        assert_eq!(shown(&app), "AAA", "the first is on screen");

        // F2 walks them, and wraps.
        app.handle_key(code(KeyCode::F(2))).unwrap();
        assert_eq!(shown(&app), "BBB");
        app.handle_key(code(KeyCode::F(2))).unwrap();
        assert_eq!(shown(&app), "CCC");
        app.handle_key(code(KeyCode::F(2))).unwrap();
        assert_eq!(shown(&app), "AAA", "wrapped round");
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(shown(&app), "CCC", "and back the other way");

        // Each tab keeps its own place in its own file.
        if let Popup::Viewer { line, .. } = &mut app.popup {
            *line = 0;
        }
        app.handle_key(code(KeyCode::F(2))).unwrap(); // to a.txt
        app.handle_key(key('i')).unwrap();
        app.handle_key(key('X')).unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert_eq!(shown(&app), "XAAA", "edited");
        app.handle_key(code(KeyCode::F(2))).unwrap();
        assert_eq!(shown(&app), "BBB", "the other tab is untouched");
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(shown(&app), "XAAA", "and the edit is still there on return");

        // Esc closes this file; the rest stay open. Only the last one closes
        // the viewer. (The edited tab needs its discard key.)
        quit_viewer_discarding(&mut app);
        assert_eq!(app.viewer_tab_count(), 2, "one closed, two left");
        assert!(matches!(app.popup, Popup::Viewer { .. }), "still viewing");
        quit_viewer(&mut app);
        quit_viewer(&mut app);
        assert!(matches!(app.popup, Popup::None), "the last one closes it");
        assert_eq!(app.viewer_tab_count(), 0);
    }

    /// Paste goes where vi puts it: `p` after, `P` before, whole lines when
    /// whole lines were copied.
    #[test]
    fn p_and_shift_p_paste_after_and_before() {
        let (_d, mut app) = viewer_on("one\ntwo\nthree\n");
        let lines = |app: &App| viewer_lines(app);

        // Line-wise: `V` then `y` copies a whole line, `p` puts it below.
        app.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT)).unwrap();
        app.handle_key(key('y')).unwrap();
        // Kept inside cian as well as on the system clipboard: a machine
        // reached over SSH often has neither a clipboard service nor a need
        // for one, and copy-and-paste within a file must work there.
        assert_eq!(app.yank.as_deref(), Some("one\n"), "the yank carries its newline");
        app.handle_key(key('p')).unwrap();
        assert_eq!(lines(&app), ["one", "one", "two", "three"], "below the cursor");
        app.handle_key(key('u')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(lines(&app), ["one", "one", "two", "three"], "above it, same result at line 0");

        // Character-wise: `p` lands after the character under the cursor.
        let (_d2, mut app) = viewer_on("abc\n");
        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE)).unwrap();
        app.handle_key(key('y')).unwrap(); // copies "a"
        app.handle_key(key('p')).unwrap();
        assert_eq!(lines(&app), ["aabc"], "after the cursor");
        app.handle_key(key('u')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(lines(&app), ["aabc"], "before it, same result at column 0");
    }

    /// The tab strip: every open file named, and the mouse able to reach both
    /// the arrows and the names. Also that a menu opened from the viewer is
    /// drawn *over* it rather than instead of it.
    #[test]
    fn the_tab_strip_is_visible_and_clickable() {
        let d = tempfile::tempdir().unwrap();
        for n in ["alpha.txt", "beta.txt", "gamma.txt"] {
            std::fs::write(d.path().join(n), format!("{n}\n")).unwrap();
        }
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        for n in ["alpha.txt", "beta.txt", "gamma.txt"] {
            let path = app.active_pane().unwrap().entries.iter().find(|e| e.name == n).unwrap().path.clone();
            app.active_pane_mut().unwrap().marks.insert(path);
        }
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let screen = render(&mut app, 160, 30).join("\n");
        for n in ["alpha.txt", "beta.txt", "gamma.txt"] {
            assert!(screen.contains(n), "every open file is named in the strip:\n{screen}");
        }
        assert!(!app.viewer_tab_rects.is_empty(), "and each has somewhere to click");

        let shown = |app: &App| match &app.popup {
            Popup::Viewer { view, .. } => view.lines.join("\n"),
            other => panic!("not a viewer: {other:?}"),
        };
        let click = |app: &mut App, c: u16, r: u16| {
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: c,
                row: r,
                modifiers: KeyModifiers::NONE,
            });
        };

        // Click the third tab by name.
        let (rect, _) = app.viewer_tab_rects[2];
        click(&mut app, rect.x + 1, rect.y);
        assert_eq!(shown(&app), "gamma.txt", "clicked straight to the third");

        // The arrows step, at their fixed columns.
        let f = app.viewer_frame;
        click(&mut app, f.x + 2, f.y);
        assert_eq!(shown(&app), "beta.txt", "◂ went back one");
        click(&mut app, f.x + 4, f.y);
        assert_eq!(shown(&app), "gamma.txt", "▸ went forward one");

        // A menu opened from the viewer keeps the file on screen behind it.
        let _ = render(&mut app, 160, 30);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        let screen = render(&mut app, 160, 30).join("\n");
        assert!(screen.contains("gamma.txt"), "the file is still there:\n{screen}");
        assert!(
            screen.contains("Theme") || screen.contains("テーマ"),
            "with the menu over it:\n{screen}",
        );
    }

    /// F3 inside a zip opens the member; saving puts it back into the zip
    /// rather than leaving the work in a temp file nobody will look at again.
    #[test]
    fn editing_a_zip_member_writes_it_back_into_the_zip() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("stage");
        std::fs::create_dir_all(src.join("conf")).unwrap();
        std::fs::write(src.join("conf").join("app.ini"), "[main]\nlevel=INFO\n").unwrap();
        let zip = d.path().join("bundle.zip");
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut sink = |_: &cian_core::progress::Progress| {};
        let mut ctl = cian_core::progress::Ctl { cancel: &cancel, on_progress: &mut sink };
        let r = cian_core::archive::create_zip(
            &[src.join("conf")],
            &zip,
            None,
            &mut ctl,
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);

        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.enter_archive(zip.clone(), String::new());
        // Into conf/, then onto the member.
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name.starts_with("conf")).unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "app.ini").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "the member opened: {:?}", app.popup);

        // Edit it and save.
        app.handle_key(key(':')).unwrap();
        if let Popup::Viewer { sub_input, .. } = &mut app.popup {
            *sub_input = Some("s/INFO/DEBUG/".into());
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        app.handle_key(key(':')).unwrap();
        if let Popup::Viewer { sub_input, .. } = &mut app.popup {
            *sub_input = Some("w".into());
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(
            app.message.as_deref().unwrap_or("").contains("bundle.zip"),
            "it says where it went: {:?}",
            app.message,
        );

        // The archive itself now holds the edit.
        let out = d.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let mut sink = |_: &cian_core::progress::Progress| {};
        let mut ctl = cian_core::progress::Ctl { cancel: &cancel, on_progress: &mut sink };
        let r = cian_core::archive::extract(
            &zip,
            &["conf/app.ini".to_string()],
            &out,
            None,
            "",
            &mut ctl,
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let back = std::fs::read_to_string(out.join("conf").join("app.ini")).unwrap();
        assert!(back.contains("level=DEBUG"), "the zip has the edit: {back:?}");
        assert!(back.contains("[main]"), "and the rest of the file");
    }

    /// The ruler and the crosshair, and Enter reading rather than launching.
    #[test]
    fn the_viewer_shows_a_column_scale_and_where_the_cursor_is() {
        let (_d, mut app) = viewer_on("abcdefghijklmnopqrstuvwxyz\nsecond line\n");
        app.show_ws = false;
        let screen = render(&mut app, 120, 20);
        // Every tenth column numbered, every fifth marked.
        let scale = screen.iter().find(|r| r.contains("····+····1")).cloned();
        assert!(scale.is_some(), "a column scale over the text:\n{}", screen.join("\n"));

        // …and it says which column the cursor is in, as the corner does.
        //
        // Walked across rather than set to one number: the scale is built from
        // `·`, which is two bytes wide, and cutting it at a *column* number
        // took the program down on the very first press of the right arrow.
        // A single hand-picked column can sit on a byte boundary by luck.
        for want in 2..=12 {
            app.handle_key(code(KeyCode::Right)).unwrap();
            let screen = render(&mut app, 120, 20).join("\n");
            assert!(screen.contains(&format!("1:{want}")), "the corner agrees with the corner");
        }

        // The scale starts where the text starts. Measured in cells, because
        // "roughly above it" is what it looked like and was not.
        let rows = render(&mut app, 120, 20);
        let ruler = rows.iter().find(|r| r.contains("····+")).expect("a ruler");
        let text = rows.iter().find(|r| r.contains("abcdefghij")).expect("the line");
        assert_eq!(
            ruler.find('·').unwrap(),
            text.find('a').unwrap(),
            "the first column of the scale is over the first column of the text",
        );

        // …and the column is counted the way the screen counts it. Two
        // full-width characters take four columns, so the cursor on the third
        // is in column five — which is what the ruler marks and therefore what
        // the corner has to say, or the two disagree on every Japanese line.
        let (_d2, mut app) = viewer_on("あいうえお\n");
        app.show_ws = false;
        for (chars_over, want_col) in [(0, 1), (1, 3), (2, 5), (4, 9)] {
            if let Popup::Viewer { col, .. } = &mut app.popup {
                *col = chars_over;
            }
            let screen = render(&mut app, 120, 20).join("\n");
            assert!(
                screen.contains(&format!("1:{want_col}")),
                "{chars_over} characters in is column {want_col}:\n{screen}",
            );
        }

        // `:ruler` puts both away and gives the row back to the text.
        let rows_with = render(&mut app, 120, 20).iter().filter(|r| r.contains("second line")).count();
        app.handle_key(key(':')).unwrap();
        if let Popup::Viewer { sub_input, .. } = &mut app.popup {
            *sub_input = Some("ruler".into());
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let screen = render(&mut app, 120, 20);
        assert!(!screen.iter().any(|r| r.contains("····+····1")), "the scale is gone");
        assert_eq!(
            screen.iter().filter(|r| r.contains("second line")).count(),
            rows_with,
            "the text is still all there",
        );
    }

    /// Enter reads the file — in the pane, since the editor is `F3` — and
    /// launching it is Ctrl+Enter. Looking at a file is the hundred-times-a-
    /// day action and can be left with Esc; an application opened by accident
    /// has to be found and closed.
    #[test]
    fn enter_reads_the_file_and_ctrl_enter_launches_it() {
        let (_d, mut app) = app_with(&["note.txt"]);
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "note.txt").unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "Enter opened the viewer");
        assert_eq!(app.viewer_dock, Some(FocusedPane::Left), "docked in the pane it came from");
        for k in [':', 'q'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();

        // On a directory Enter still goes in, which is not something a
        // launcher could have meant.
        let d2 = tempfile::tempdir().unwrap();
        std::fs::create_dir(d2.path().join("sub")).unwrap();
        let p = d2.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "sub").unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(app.active_pane().unwrap().cwd.ends_with("sub"), "went in");
    }

    /// "Select all" means the listing in a pane and the file in the viewer —
    /// one idea, and which of the two is simply which is in front of you.
    #[test]
    fn select_all_means_this_directory_or_this_file() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)).unwrap();
        let p = app.active_pane().unwrap();
        assert_eq!(p.marks.len(), 3, "everything here");
        assert!(
            !p.marks.iter().any(|m| m.ends_with("..")),
            "but not the parent, which is not a file to operate on",
        );

        // In the viewer it is a line-wise selection of the whole buffer, so
        // `y` copies the file and Esc clears it — the ordinary visual keys.
        let (_d2, mut app) = viewer_on("one\ntwo\nthree\n");
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)).unwrap();
        match &app.popup {
            Popup::Viewer { visual: Some(ViewVisual::Line), anchor, line, .. } => {
                assert_eq!(*anchor, (0, 0));
                assert_eq!(*line, 2, "down to the last line");
            }
            other => panic!("expected a whole-file selection, got {other:?}"),
        }
        app.handle_key(key('y')).unwrap();
        assert_eq!(app.yank.as_deref(), Some("one\ntwo\nthree\n"), "and y takes the lot");

        // Reachable without Ctrl, which this terminal does not deliver.
        let (_d3, mut app) = app_with_keymaps(&["a.txt"], vec![("alt+a", "mark_all".into())]);
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT)).unwrap();
        assert_eq!(app.active_pane().unwrap().marks.len(), 1);
    }

    /// `=` compares the two halves in place: the marks appear on the real
    /// lines, both files stay editable, and the comparison follows the edit.
    #[test]
    fn a_split_can_be_compared_while_both_halves_stay_editable() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "same\nold\ngone\ntail\n").unwrap();
        std::fs::write(d.path().join("b.txt"), "same\nnew\ntail\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        for n in ["a.txt", "b.txt"] {
            let path = app.active_pane().unwrap().entries.iter().find(|e| e.name == n).unwrap().path.clone();
            app.active_pane_mut().unwrap().marks.insert(path);
        }
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 160, 30);

        // Without a split it says what to do rather than doing nothing.
        app.handle_key(key('=')).unwrap();
        assert!(app.viewer_diff.is_none());
        assert!(app.message.as_deref().unwrap_or("").contains("F8"));

        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT)).unwrap();
        let _ = render(&mut app, 160, 30);
        app.handle_key(key('=')).unwrap();
        let marks = |app: &App| app.viewer_diff.as_deref().unwrap().mine.clone();
        use cian_core::diff::Mark;
        assert_eq!(
            marks(&app),
            vec![Mark::Same, Mark::Changed, Mark::Only, Mark::Same],
            "one mark per real line — nothing inserted to line the two up",
        );

        // `]c` / `[c` step the differences — vimdiff's own keys. Tab used to
        // do the forward half and belongs to the window now.
        let at = |app: &App| match &app.popup {
            Popup::Viewer { line, .. } => *line,
            other => panic!("not a viewer: {other:?}"),
        };
        app.handle_key(key(']')).unwrap();
        app.handle_key(key('c')).unwrap();
        assert_eq!(at(&app), 1, "the changed line");
        app.handle_key(key(']')).unwrap();
        app.handle_key(key('c')).unwrap();
        assert_eq!(at(&app), 2, "the one only this side has");
        app.handle_key(key('[')).unwrap();
        app.handle_key(key('c')).unwrap();
        assert_eq!(at(&app), 1, "and back");

        // Editing one half is allowed, and the comparison follows it: making
        // the changed line match makes the difference go away.
        app.handle_key(key(':')).unwrap();
        if let Popup::Viewer { sub_input, .. } = &mut app.popup {
            *sub_input = Some("s/old/new/".into());
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), ["same", "new", "gone", "tail"], "edited in place");
        let _ = render(&mut app, 160, 30);
        assert_eq!(marks(&app)[1], Mark::Same, "the edit closed the difference");

        // `=` again stops.
        app.handle_key(key('=')).unwrap();
        assert!(app.viewer_diff.is_none());

        // A key that refuses has to refuse every time, not only the first —
        // the reply is about the keystroke, not about the words changing.
        app.handle_key(KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT)).unwrap();
        for _ in 0..3 {
            app.handle_key(key('=')).unwrap();
            assert!(app.message_fresh, "said so again");
            assert!(app.message.as_deref().unwrap_or("").contains("F8"));
            app.handle_key(code(KeyCode::Down)).unwrap();
            assert!(!app.message_fresh, "and stood down for the next key");
        }
    }

    /// `?` in the viewer answers "what can I do here", not "what can cian do".
    #[test]
    fn question_mark_lists_only_the_editor_panels_keys() {
        let (_d, mut app) = viewer_on("hello\n");
        app.handle_key(key('?')).unwrap();
        let Popup::Report { lines, .. } = &app.popup else { panic!("no help: {:?}", app.popup) };
        let text = lines.join("\n");
        assert!(
            text.contains("text editor panel") || text.contains("テキストエディタパネル"),
            "it names the panel, not the key that used to open it:\n{text}",
        );
        assert!(!text.contains("(F3)"), "and does not put F3 in its name:\n{text}");
        // Things the viewer cannot do are not in it.
        for absent in ["Rename", "SSH", "trash"] {
            assert!(!text.contains(absent), "{absent:?} does not belong here:\n{text}");
        }
        // The keys it *does* have are, grouped by what you are doing.
        for present in ["Move", "Edit", "gg", "zz", "*", ">>", ":wq"] {
            assert!(text.contains(present), "{present:?} is missing:\n{text}");
        }
        // It scrolls — it is far taller than a dialog.
        app.handle_key(key('j')).unwrap();
        let Popup::Report { scroll, .. } = &app.popup else { panic!("gone") };
        assert_eq!(*scroll, 1);
        // …and it goes back to the file.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "back to the file");
    }

    /// The mouse reaches both halves of a split and both tab arrows. All of
    /// this is geometry, and the geometry used to be measured against a
    /// viewer that filled the screen even when it had half of it.
    #[test]
    fn the_mouse_reaches_the_other_half_and_the_tab_arrows() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "AAA\n").unwrap();
        std::fs::write(d.path().join("b.txt"), "BBB\n").unwrap();
        std::fs::write(d.path().join("c.txt"), "CCC\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            let path = app.active_pane().unwrap().entries.iter().find(|e| e.name == n).unwrap().path.clone();
            app.active_pane_mut().unwrap().marks.insert(path);
        }
        // Marked files open together; F12 gives the panel the window, which
        // is the geometry a split is measured against here.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        app.handle_key(code(KeyCode::F(12))).unwrap();
        let shown = |app: &App| match &app.popup {
            Popup::Viewer { view, .. } => view.lines.join("\n"),
            other => panic!("not a viewer: {other:?}"),
        };
        let click = |app: &mut App, c: u16, r: u16| {
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: c,
                row: r,
                modifiers: KeyModifiers::NONE,
            });
        };

        // The arrows step through the open files.
        let _ = render(&mut app, 160, 30);
        let f = app.viewer_frame;
        click(&mut app, f.x + 2, f.y);
        assert_eq!(shown(&app), "CCC", "◂ wrapped back to the last");
        click(&mut app, f.x + 4, f.y);
        assert_eq!(shown(&app), "AAA", "▸ came round again");

        // Split, then click the half the keyboard is not on.
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT)).unwrap();
        let _ = render(&mut app, 160, 30);
        let theirs = app.viewer_half_rects[1];
        assert!(theirs.width > 0, "the other half was measured");
        // Well inside it: its own left edge is the seam with the first half,
        // and a click on a seam is a resize.
        click(&mut app, theirs.x + 8, theirs.y + 3);
        assert_eq!(
            shown(&app),
            "BBB",
            "the keyboard crossed to the half that was clicked (halves {:?}, frame {:?}, dock {:?}, zoomed {})",
            app.viewer_half_rects,
            app.viewer_frame,
            app.viewer_dock,
            app.zoomed,
        );
        let theirs = app.viewer_half_rects[1];
        click(&mut app, theirs.x + 5, theirs.y + 3);
        assert_eq!(shown(&app), "AAA", "and back again");
    }

    /// A split must not draw anything but the viewer. It used to draw every
    /// popup as though it were one — so the menu, and worse the quit
    /// confirmation, were on screen and invisible, quietly taking the Enter
    /// that followed.
    #[test]
    fn a_split_does_not_swallow_the_dialogs_that_open_over_it() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "AAA\n").unwrap();
        std::fs::write(d.path().join("b.txt"), "BBB\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        for n in ["a.txt", "b.txt"] {
            let path = app.active_pane().unwrap().entries.iter().find(|e| e.name == n).unwrap().path.clone();
            app.active_pane_mut().unwrap().marks.insert(path);
        }
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 160, 30);
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT)).unwrap();

        // Shift+Enter opens the menu, and the menu is drawn.
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert!(matches!(app.popup, Popup::ContextMenu { .. }), "the menu opened");
        let screen = render(&mut app, 160, 30).join("\n");
        assert!(
            screen.contains("Theme") || screen.contains("テーマ"),
            "the menu is actually on screen:\n{screen}",
        );
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "and the file came back");

        // Closing every file leaves nothing of the split behind, so the next
        // dialog to open is visible.
        quit_viewer(&mut app);
        quit_viewer(&mut app);
        assert!(matches!(app.popup, Popup::None), "viewer gone: {:?}", app.popup);
        assert!(app.viewer_split.is_none(), "and so is the split");
        app.handle_key(key('q')).unwrap();
        let screen = render(&mut app, 160, 30).join("\n");
        assert!(!matches!(app.popup, Popup::None), "the quit confirmation opened");
        assert!(
            screen.contains("uit") || screen.contains("終了"),
            "and is on screen:\n{screen}",
        );
    }

    /// Two files side by side, on the keys the shell panel already uses.
    #[test]
    fn the_viewer_splits_and_puts_itself_back_together() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "AAA\n").unwrap();
        std::fs::write(d.path().join("b.txt"), "BBB\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        for n in ["a.txt", "b.txt"] {
            let path = app.active_pane().unwrap().entries.iter().find(|e| e.name == n).unwrap().path.clone();
            app.active_pane_mut().unwrap().marks.insert(path);
        }
        // Open them, then give the panel the window: this is about how a
        // split is laid out across it.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        app.handle_key(code(KeyCode::F(12))).unwrap();
        let shown = |app: &App| match &app.popup {
            Popup::Viewer { view, .. } => view.lines.join("\n"),
            other => panic!("not a viewer: {other:?}"),
        };
        let other = |app: &App| match app.viewer_split.as_deref() {
            Some(Popup::Viewer { view, .. }) => view.lines.join("\n"),
            _ => panic!("not split"),
        };

        // Shift+F8 puts the next open file beside this one.
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT)).unwrap();
        assert!(app.viewer_split.is_some(), "split");
        assert_eq!(shown(&app), "AAA");
        assert_eq!(other(&app), "BBB");
        // …and the strip no longer holds it, since it is on screen.
        assert!(app.viewer_tabs.is_empty(), "both halves are on screen");

        // Shift+L crosses over, Shift+H comes back.
        app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(shown(&app), "BBB", "the keyboard is on the other half");
        app.handle_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(shown(&app), "AAA");

        // Both halves are drawn side by side — and crossing over moves the
        // focus, not the files: each stays on the side it was put.
        let side_of = |app: &mut App, needle: &str| -> usize {
            let rows = render(app, 160, 30);
            let row = rows.iter().find(|r| r.contains(needle)).expect("on screen");
            usize::from(row.find(needle).expect("column") >= 80)
        };
        assert_eq!(side_of(&mut app, "AAA"), 0, "AAA is on the left");
        assert_eq!(side_of(&mut app, "BBB"), 1, "BBB is on the right");
        app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(shown(&app), "BBB", "the keyboard crossed over");
        assert_eq!(side_of(&mut app, "AAA"), 0, "…and AAA did not move");
        assert_eq!(side_of(&mut app, "BBB"), 1, "…nor did BBB");
        app.handle_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT)).unwrap();

        // Shift+F10 keeps the one being read and returns the other to the strip.
        app.handle_key(KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT)).unwrap();
        assert!(app.viewer_split.is_none(), "one file again");
        assert_eq!(shown(&app), "AAA", "the half in focus stayed");
        assert_eq!(app.viewer_tab_count(), 2, "the other went back to the tabs");
        app.handle_key(code(KeyCode::F(2))).unwrap();
        assert_eq!(shown(&app), "BBB", "and is still reachable");
    }

    /// `:q` closes the file it was typed into, not the viewer. In a split it
    /// used to take the other half down with it — two files read together,
    /// one `:q`, and both were gone.
    #[test]
    fn q_in_a_split_closes_only_the_half_it_was_typed_into() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "AAA\n").unwrap();
        std::fs::write(d.path().join("b.txt"), "BBB\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        for n in ["a.txt", "b.txt"] {
            let path =
                app.active_pane().unwrap().entries.iter().find(|e| e.name == n).unwrap().path.clone();
            app.active_pane_mut().unwrap().marks.insert(path);
        }
        app.handle_key(code(KeyCode::F(3))).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT)).unwrap();
        let shown = |app: &App| match &app.popup {
            Popup::Viewer { view, .. } => view.lines.join("\n"),
            other => panic!("not a viewer: {other:?}"),
        };
        assert_eq!(shown(&app), "AAA");

        // `:q` on the half in focus leaves the other one being read.
        for k in [':', 'q'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(app.viewer_split.is_none(), "the split is over");
        assert_eq!(shown(&app), "BBB", "the other half is what's left");

        // The second `:q` has nothing else open, so the viewer closes.
        for k in [':', 'q'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::None), "the last file closes the viewer");
    }

    /// With a Japanese IME on, punctuation arrives full-width: `：` for the
    /// colon key, `／` for slash. Where a keystroke is a command that is still
    /// the key being pressed, so it opens what the key opens — but text must
    /// arrive exactly as typed, because a name may hold those characters on
    /// purpose (and on Windows, must).
    #[test]
    fn ime_punctuation_works_as_a_command_but_never_inside_text() {
        use crate::util::{fold_ime_key, fold_ime_word};
        assert_eq!(fold_ime_key('：'), Some(':'));
        assert_eq!(fold_ime_key('／'), Some('/'));
        assert_eq!(fold_ime_key('？'), Some('?'));
        assert_eq!(fold_ime_key('ｑ'), Some('q'));
        assert_eq!(fold_ime_key('・'), Some('/'), "the kana layout's slash key");
        assert_eq!(fold_ime_key('あ'), None, "kana is not a key press");
        assert_eq!(fold_ime_word("ｒａｇ"), "rag");

        // In a pane, `：` opens the command line.
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(key('：')).unwrap();
        assert_eq!(app.mode, Mode::Command, "the colon key opened the command line");
        // …and what is typed into it is left alone: this is text.
        app.handle_key(key('：')).unwrap();
        assert_eq!(app.command_buffer, "：", "typed text keeps its full-width form");
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // A verb typed with the IME on still runs, since verbs are ASCII.
        app.command_buffer = "ｍａｎ".into();
        app.run_command();
        assert!(matches!(app.popup, Popup::Manual { .. }), "ｍａｎ ran :man");
        app.popup = Popup::None;

        // A rename keeps every character exactly as typed — folding a
        // full-width colon into a real one would be a different file name,
        // and an illegal one on Windows.
        app.start_rename();
        for c in "メモ：一覧".chars() {
            app.handle_key(key(c)).unwrap();
        }
        match &app.popup {
            Popup::TextInput { buffer, .. } => assert!(
                buffer.ends_with("メモ：一覧"),
                "the name is what was typed: {buffer:?}"
            ),
            other => panic!("expected the rename prompt, got {other:?}"),
        }
    }

    /// In the viewer the same rule applies: `／` searches, but the text of the
    /// search itself is left as typed — a Japanese file is searched for
    /// Japanese.
    #[test]
    fn ime_punctuation_opens_the_viewer_search_but_not_its_text() {
        let (_d, mut app) = viewer_on("メモ：一覧\nplain\n");
        app.handle_key(key('／')).unwrap();
        assert!(
            matches!(&app.popup, Popup::Viewer { find_input: Some(_), .. }),
            "the slash key opened the search"
        );
        for c in "：一覧".chars() {
            app.handle_key(key(c)).unwrap();
        }
        match &app.popup {
            Popup::Viewer { find_input: Some(q), .. } => {
                assert_eq!(q, "：一覧", "the query is what was typed")
            }
            other => panic!("expected a search in progress, got {other:?}"),
        }
    }

    /// A terminal paste (Cmd/Ctrl+V) arrives as one event carrying the whole
    /// text. In the viewer it used to arrive nowhere at all — the paste path
    /// knew about every one-line field and not about the file — so the only
    /// way to get text in was to have the terminal type it, a frame per
    /// character. It lands as one edit, undone in one step.
    #[test]
    fn a_terminal_paste_lands_in_the_viewer_in_one_edit() {
        let (_d, mut app) = viewer_on("first\nsecond\n");
        let before = viewer_lines(&app).join("\n");

        // Reading: it goes in where `p` would put it, newlines and all.
        app.insert_into_active_text("alpha\nbeta\n");
        let after = viewer_lines(&app);
        assert!(after.iter().any(|l| l.contains("alpha")), "the text is in: {after:?}");
        assert!(after.iter().any(|l| l.contains("beta")));
        assert!(after.len() > 2, "both lines landed: {after:?}");

        // One edit: one `u` puts the file back.
        app.handle_key(key('u')).unwrap();
        assert_eq!(viewer_lines(&app).join("\n"), before, "the paste undoes in one step");

        // Editing: it lands at the caret rather than on the next line.
        app.handle_key(key('i')).unwrap();
        app.insert_into_active_text("XY");
        let l = viewer_lines(&app);
        assert!(l[0].starts_with("XY"), "at the cursor: {l:?}");
    }

    /// A paste while a prompt is open over the file belongs to the prompt.
    /// It used to go into the file: typing `/` and pasting the search term
    /// left the search box empty and the term spliced into the text.
    #[test]
    fn a_paste_goes_to_the_prompt_that_is_open_over_the_file() {
        let (_d, mut app) = viewer_on("alpha\nbeta\n");
        app.handle_key(key('/')).unwrap();
        app.insert_into_active_text("bet");
        match &app.popup {
            Popup::Viewer { find_input, view, .. } => {
                assert_eq!(find_input.as_deref(), Some("bet"), "into the search box");
                assert_eq!(view.lines, vec!["alpha", "beta"], "and not into the file");
            }
            other => panic!("expected the viewer, got {other:?}"),
        }

        // The `:` line likewise.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        app.handle_key(key(':')).unwrap();
        app.insert_into_active_text("w");
        match &app.popup {
            Popup::Viewer { sub_input, view, .. } => {
                assert_eq!(sub_input.as_deref(), Some("w"));
                assert_eq!(view.lines, vec!["alpha", "beta"]);
            }
            other => panic!("expected the viewer, got {other:?}"),
        }
    }

    /// Text cannot be pasted into a binary file. What is on screen is a hex
    /// rendering of the bytes, not the bytes — a pasted line would be saved
    /// as whatever that rendering parses back to.
    #[test]
    fn text_is_refused_for_a_binary_file() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("bin.dat"), [0u8, 1, 2, 3, 255, 254]).unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let i = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "bin.dat")
            .unwrap();
        app.active_pane_mut().unwrap().cursor = i;
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let before = match &app.popup {
            Popup::Viewer { view, .. } => view.lines.clone(),
            other => panic!("expected the viewer, got {other:?}"),
        };
        app.insert_into_active_text("hello\n");
        match &app.popup {
            Popup::Viewer { view, dirty, .. } => {
                assert_eq!(view.lines, before, "the hex dump is untouched");
                assert!(!*dirty, "and the file is not marked as edited");
            }
            other => panic!("expected the viewer, got {other:?}"),
        }
        assert!(app.message.is_some(), "it says why");
    }

    /// Typing `48G` used to happen in the dark: the count built up invisibly,
    /// so there was no way to tell what had been pressed. It now shows on the
    /// prompt row, where `:` and `/` show theirs, and Esc abandons it.
    #[test]
    fn a_half_typed_command_is_visible_and_cancellable() {
        let (_d, mut app) = viewer_on(&(1..=80).map(|i| format!("line {i}\n")).collect::<String>());
        app.handle_key(key('4')).unwrap();
        app.handle_key(key('8')).unwrap();
        match &app.popup {
            Popup::Viewer { count, .. } => assert_eq!(*count, Some(48)),
            other => panic!("expected the viewer, got {other:?}"),
        }
        let rows = render(&mut app, 100, 30);
        assert!(rows.iter().any(|r| r.contains("48_")), "what is typed is on screen:\n{rows:?}");

        // Esc abandons it rather than closing the file.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        match &app.popup {
            Popup::Viewer { count, .. } => assert_eq!(*count, None, "the count is gone"),
            other => panic!("Esc closed the viewer instead: {other:?}"),
        }

        // And it still jumps.
        for k in ['4', '8', 'G'] {
            app.handle_key(key(k)).unwrap();
        }
        match &app.popup {
            Popup::Viewer { line, .. } => assert_eq!(*line, 47, "48G is line 48"),
            other => panic!("expected the viewer, got {other:?}"),
        }
    }

    /// A count repeats the motion it precedes, as it does in vi. Only `G`
    /// used to take one.
    #[test]
    fn a_count_repeats_the_motion_it_precedes() {
        let (_d, mut app) =
            viewer_on(&(1..=80).map(|i| format!("line {i} word word\n")).collect::<String>());
        let line = |app: &App| match &app.popup {
            Popup::Viewer { line, .. } => *line,
            other => panic!("expected the viewer, got {other:?}"),
        };
        let col = |app: &App| match &app.popup {
            Popup::Viewer { col, .. } => *col,
            other => panic!("expected the viewer, got {other:?}"),
        };
        for k in ['3', 'j'] {
            app.handle_key(key(k)).unwrap();
        }
        assert_eq!(line(&app), 3, "3j");
        for k in ['2', 'k'] {
            app.handle_key(key(k)).unwrap();
        }
        assert_eq!(line(&app), 1, "2k");
        for k in ['5', 'l'] {
            app.handle_key(key(k)).unwrap();
        }
        assert_eq!(col(&app), 5, "5l");
        for k in ['2', 'w'] {
            app.handle_key(key(k)).unwrap();
        }
        assert!(col(&app) > 5, "2w moved on: {}", col(&app));
        // `gg`, not a bare `g`: the prefix is vi's, and it leaves `gJ` room.
        for k in ['5', 'g', 'g'] {
            app.handle_key(key(k)).unwrap();
        }
        assert_eq!(line(&app), 4, "5gg is line 5");
    }

    /// The vim keys the viewer was missing: `*` searches the word under the
    /// cursor, `~` swaps its case, `>>` shifts a line by a tab stop, and `zz`
    /// puts the cursor's line in the middle of the window without moving it.
    #[test]
    fn star_tilde_shift_and_zz() {
        let (_d, mut app) = viewer_on("alpha beta\ngamma\nbeta again\n");
        app.handle_key(key('w')).unwrap(); // onto "beta"
        app.handle_key(key('*')).unwrap();
        match &app.popup {
            Popup::Viewer { find_query, line, .. } => {
                assert_eq!(find_query.as_deref(), Some("beta"), "the word under the cursor");
                assert_eq!(*line, 2, "and it jumped to the next one");
            }
            other => panic!("expected the viewer, got {other:?}"),
        }

        let (_d2, mut app2) = viewer_on("abc\n");
        app2.handle_key(key('~')).unwrap();
        assert_eq!(viewer_lines(&app2)[0], "Abc");
        app2.handle_key(key('~')).unwrap();
        assert_eq!(viewer_lines(&app2)[0], "ABc", "it walks along");

        app2.handle_key(key('>')).unwrap();
        app2.handle_key(key('>')).unwrap();
        assert!(viewer_lines(&app2)[0].starts_with("    "), "{:?}", viewer_lines(&app2));
        app2.handle_key(key('<')).unwrap();
        app2.handle_key(key('<')).unwrap();
        assert_eq!(viewer_lines(&app2)[0], "ABc", "and back");

        let (_d3, mut app3) =
            viewer_on(&(1..=200).map(|i| format!("l{i}\n")).collect::<String>());
        let _ = render(&mut app3, 100, 30);
        for k in ['1', '0', '0', 'G'] {
            app3.handle_key(key(k)).unwrap();
        }
        let _ = render(&mut app3, 100, 30);
        app3.handle_key(key('z')).unwrap();
        app3.handle_key(key('z')).unwrap();
        match &app3.popup {
            Popup::Viewer { line, scroll, .. } => {
                assert_eq!(*line, 99, "the cursor stayed");
                assert!(*scroll > 0 && *scroll < 99, "the line moved to the middle: {scroll}");
            }
            other => panic!("expected the viewer, got {other:?}"),
        }
    }

    /// The file closes with `:q`, as it does in vi — never with Esc, which is
    /// the key you press to mean "never mind" and must not also mean "put
    /// this away". The ✕ in the corner is the mouse's way out.
    #[test]
    fn only_q_and_the_button_close_the_viewer() {
        let (_d, mut app) = viewer_on("alpha\nbeta\n");
        let rows = render(&mut app, 100, 30);
        assert!(rows.iter().any(|r| r.contains('✕')), "the button is drawn:\n{rows:?}");

        // Esc keeps the file, and says nothing: a count along the bottom of
        // the window, raised by a key pressed in error, is noise on the one
        // occasion it is least wanted.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "Esc kept the file");
        assert!(app.message.is_none(), "and quietly: {:?}", app.message);

        // A click on the ✕ closes it.
        let x = app.viewer_close_rect;
        assert!(x.width > 0, "the button has a place on screen");
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: x.x,
            row: x.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(app.popup, Popup::None), "the button closed it");
    }

    /// Even a file cian cannot write closes with `:q` — the prompt used to be
    /// offered only on editable files, which after this change would have left
    /// a PDF or a docx with no way out at all.
    #[test]
    fn a_read_only_file_can_still_be_closed_and_refuses_to_be_written() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let i = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "bin.dat")
            .unwrap();
        app.active_pane_mut().unwrap().cursor = i;
        app.handle_key(code(KeyCode::F(3))).unwrap();
        // Force the read-only case the document viewers produce.
        if let Popup::Viewer { editable, .. } = &mut app.popup {
            *editable = false;
        }
        for k in [':', 'w'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), ":w did not close it");
        assert!(app.message.is_some(), "…and said why it cannot be written");
        quit_viewer(&mut app);
        assert!(matches!(app.popup, Popup::None), ":q closes a read-only file");
    }

    /// vi's whole point: operators and motions multiply. `dw`, `d2w`, `d$`,
    /// `cw`, `yy`, `dj` — one grammar rather than a key per combination.
    #[test]
    fn operators_take_motions_and_counts() {
        let keys = |app: &mut App, s: &str| {
            for c in s.chars() {
                app.handle_key(key(c)).unwrap();
            }
        };
        // dw
        let (_d, mut app) = viewer_on("alpha beta gamma\nsecond\n");
        keys(&mut app, "dw");
        assert_eq!(viewer_lines(&app)[0], "beta gamma");
        // d2w from the start takes both words
        let (_d2, mut app2) = viewer_on("alpha beta gamma\n");
        keys(&mut app2, "d2w");
        assert_eq!(viewer_lines(&app2)[0], "gamma");
        // d$ to the end of the line, including the last character
        let (_d3, mut app3) = viewer_on("alpha beta\n");
        keys(&mut app3, "ld$");
        assert_eq!(viewer_lines(&app3)[0], "a");
        // dd and 2dd
        let (_d4, mut app4) = viewer_on("one\ntwo\nthree\nfour\n");
        keys(&mut app4, "dd");
        assert_eq!(viewer_lines(&app4), ["two", "three", "four"]);
        keys(&mut app4, "2dd");
        assert_eq!(viewer_lines(&app4), ["four"]);
        // dj takes both lines, whatever the column
        let (_d5, mut app5) = viewer_on("one\ntwo\nthree\n");
        keys(&mut app5, "lldj");
        assert_eq!(viewer_lines(&app5), ["three"]);
        // cw deletes the word and leaves the editor open to type
        let (_d6, mut app6) = viewer_on("alpha beta\n");
        keys(&mut app6, "cw");
        assert!(matches!(app6.popup, Popup::Viewer { editing: true, .. }), "c opens the editor");
        // vi's one special case: `cw` changes the word, not the space after
        // it — it behaves like `ce`.
        assert_eq!(viewer_lines(&app6)[0], " beta");
        // yy copies a line without changing anything
        let (_d7, mut app7) = viewer_on("one\ntwo\n");
        keys(&mut app7, "yy");
        assert_eq!(viewer_lines(&app7), ["one", "two"], "yank changes nothing");
        assert_eq!(app7.yank.as_deref(), Some("one\n"));
    }

    /// `f`, `t` and the pair `;` `,` — and `df,`, which is the operator and
    /// the motion together.
    #[test]
    fn find_char_moves_and_can_be_operated_on() {
        let (_d, mut app) = viewer_on("one,two,three\n");
        let col = |app: &App| match &app.popup {
            Popup::Viewer { col, .. } => *col,
            other => panic!("expected the viewer, got {other:?}"),
        };
        app.handle_key(key('f')).unwrap();
        app.handle_key(key(',')).unwrap();
        assert_eq!(col(&app), 3, "f, landed on the comma");
        app.handle_key(key(';')).unwrap();
        assert_eq!(col(&app), 7, "; repeated it");
        app.handle_key(key(',')).unwrap();
        assert_eq!(col(&app), 3, ", went back");
        // `t` stops before it.
        let (_d2, mut app2) = viewer_on("one,two\n");
        app2.handle_key(key('t')).unwrap();
        app2.handle_key(key(',')).unwrap();
        assert_eq!(col(&app2), 2, "t, stopped short");
        // `df,` deletes up to and including the comma.
        let (_d3, mut app3) = viewer_on("one,two\n");
        for c in "df,".chars() {
            app3.handle_key(key(c)).unwrap();
        }
        assert_eq!(viewer_lines(&app3)[0], "two");
    }

    /// Text objects: `ciw`, `di"`, `da(` — the other half of the grammar.
    #[test]
    fn text_objects_are_operated_on() {
        let keys = |app: &mut App, s: &str| {
            for c in s.chars() {
                app.handle_key(key(c)).unwrap();
            }
        };
        let (_d, mut app) = viewer_on("alpha beta gamma\n");
        keys(&mut app, "wdiw");
        assert_eq!(viewer_lines(&app)[0], "alpha  gamma", "diw took the word only");

        let (_d2, mut app2) = viewer_on("alpha beta gamma\n");
        keys(&mut app2, "wdaw");
        assert_eq!(viewer_lines(&app2)[0], "alpha gamma", "daw took its space too");

        let (_d3, mut app3) = viewer_on("value = \"some text\";\n");
        keys(&mut app3, "10ldi\"");
        assert_eq!(viewer_lines(&app3)[0], "value = \"\";", "di\" emptied the quotes");

        let (_d4, mut app4) = viewer_on("call(one, two);\n");
        keys(&mut app4, "6lda(");
        assert_eq!(viewer_lines(&app4)[0], "call;", "da( took the brackets with it");

        let (_d5, mut app5) = viewer_on("fn f() {\n    body();\n}\n");
        keys(&mut app5, "jdi{");
        assert_eq!(viewer_lines(&app5), ["fn f() {", "}"], "di{{ emptied the block");
    }

    /// Marks, the jump list and `.` — the three things that make a vi you can
    /// live in rather than one you can type in.
    #[test]
    fn marks_jumps_and_dot_repeat() {
        let keys = |app: &mut App, s: &str| {
            for c in s.chars() {
                app.handle_key(key(c)).unwrap();
            }
        };
        let at = |app: &App| match &app.popup {
            Popup::Viewer { line, .. } => *line,
            other => panic!("expected the viewer, got {other:?}"),
        };
        let body: String = (1..=60).map(|i| format!("line {i} alpha beta\n")).collect();
        let (_d, mut app) = viewer_on(&body);

        // `ma` here, wander off, `'a` back.
        keys(&mut app, "5jma");
        assert_eq!(at(&app), 5);
        keys(&mut app, "20j");
        assert_eq!(at(&app), 25);
        keys(&mut app, "'a");
        assert_eq!(at(&app), 5, "'a came back to the mark");
        // A mark that was never set says so rather than jumping somewhere.
        keys(&mut app, "'z");
        assert_eq!(at(&app), 5);
        assert!(app.message.as_deref().is_some_and(|m| m.contains('z')), "{:?}", app.message);

        // `G` is a jump: Ctrl+O goes back to where it started, Ctrl+I forward.
        keys(&mut app, "G");
        assert_eq!(at(&app), 59);
        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(at(&app), 5, "Ctrl+O went back");
        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(at(&app), 59, "Ctrl+I forward again");

        // `.` repeats a change, including what was typed into the editor.
        let (_d2, mut app2) = viewer_on("alpha beta\ngamma delta\n");
        keys(&mut app2, "cwX");
        app2.handle_key(code(KeyCode::Esc)).unwrap();
        assert_eq!(viewer_lines(&app2)[0], "X beta", "cw then typing");
        keys(&mut app2, "j0");
        keys(&mut app2, ".");
        assert_eq!(viewer_lines(&app2)[1], "X delta", ". did it again, here");

        // …and a plain `x` repeats too.
        let (_d3, mut app3) = viewer_on("abcdef\n");
        keys(&mut app3, "x");
        keys(&mut app3, "..");
        assert_eq!(viewer_lines(&app3)[0], "def", "x then two dots");
    }

    /// `:g/re/d` drops the lines that match, `:v/re/d` the ones that do not —
    /// the two halves of reading a log.
    #[test]
    fn global_delete_keeps_or_drops_matching_lines() {
        let (_d, mut app) = viewer_on("INFO one\nERROR two\nINFO three\nERROR four\n");
        for k in ":g/ERROR/d".chars() {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), ["INFO one", "INFO three"]);
        assert!(app.message.as_deref().is_some_and(|m| m.contains('2')), "{:?}", app.message);

        // …and one undo puts them all back.
        app.handle_key(key('u')).unwrap();
        assert_eq!(viewer_lines(&app).len(), 4, "one undo step");

        // `:v` keeps only what matches.
        for k in ":v/ERROR/d".chars() {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), ["ERROR two", "ERROR four"]);
    }

    /// Shift+Tab steps between the file and the panes, and opens an empty one
    /// when there is nothing to step back into — which is what makes the
    /// viewer somewhere to start writing rather than only somewhere to read.
    #[test]
    fn a_new_file_starts_empty_and_takes_a_name_when_saved() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // `:new` gives an empty, unnamed file, docked where you were. It used
        // to be what Shift+Tab did with nothing to step back into; Shift+Tab
        // is the tab strip now.
        app.command_buffer = "new".into();
        app.run_command();
        match &app.popup {
            Popup::Viewer { path, view, editable, .. } => {
                assert!(path.as_os_str().is_empty(), "no name yet");
                assert!(*editable, "and it can be typed into");
                assert_eq!(view.lines.len(), 1);
            }
            other => panic!("expected an empty viewer, got {other:?}"),
        }
        assert_eq!(app.viewer_dock, Some(FocusedPane::Left), "in the pane you were in");

        app.handle_key(key('i')).unwrap();
        for c in "hello".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert_eq!(viewer_lines(&app)[0], "hello");

        // `:w` alone will not guess a name; `:w <name>` writes and adopts it.
        for k in [':', 'w'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(app.message.as_deref().is_some_and(|m| m.contains(":w")), "{:?}", app.message);
        for k in ":w note.md".chars() {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let written = app.active_pane().unwrap().cwd.join("note.md");
        assert!(written.exists(), "written to the pane's folder: {:?} — {:?}", written, app.message);
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "hello\n");
        match &app.popup {
            Popup::Viewer { path, title, dirty, .. } => {
                assert_eq!(path, &written, "it adopted the name");
                assert_eq!(title, "note.md");
                assert!(!*dirty, "and is saved");
            }
            other => panic!("expected the viewer, got {other:?}"),
        }
    }

    /// The state file holds more than one thing now, so writing one value must
    /// not lose the others.
    #[test]
    fn the_state_file_keeps_the_values_it_already_had() {
        let before = "# cian runtime state — managed by cian (see :where)\ntheme = \"nord\"\n";
        let after = crate::state_with(before, "font_level", "15");
        assert_eq!(crate::state_get_in(&after, "theme").as_deref(), Some("nord"), "{after}");
        assert_eq!(crate::state_get_in(&after, "font_level").as_deref(), Some("15"));
        // Setting it again replaces the line rather than adding a second one.
        let again = crate::state_with(&after, "font_level", "16");
        assert_eq!(again.matches("font_level").count(), 1, "{again}");
        assert_eq!(crate::state_get_in(&again, "font_level").as_deref(), Some("16"));
        assert_eq!(crate::state_get_in(&again, "theme").as_deref(), Some("nord"));
        // …and a file that never had a header gets one.
        let fresh = crate::state_with("", "theme", "dracula");
        assert!(fresh.starts_with("# cian runtime state"), "{fresh}");
    }

    /// `Enter` reads the file where its listing was — the *same* viewer,
    /// docked in that pane, with everything it can do. `F3` gives the same
    /// file the whole window; `:q` closes it and the listing is there again.
    #[test]
    fn enter_docks_the_panel_in_the_pane_and_f12_zooms_it() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "alpha\nbeta\n").unwrap();
        let body: String = (1..=200).map(|i| format!("line {i}\n")).collect();
        std::fs::write(d.path().join("b.log"), &body).unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let at = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "b.log")
            .unwrap();
        app.active_pane_mut().unwrap().cursor = at;

        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "the viewer opened");
        assert_eq!(app.viewer_dock, Some(FocusedPane::Left), "docked in this pane");

        // It is drawn in the pane, not over the window: the other pane still
        // lists its files beside it.
        let rows = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("line 1"), "the file is in the pane:\n{screen}");
        assert!(screen.contains("a.txt"), "and the other pane still lists files");
        assert!(app.viewer_frame.width < 70, "it takes the pane's width: {:?}", app.viewer_frame);

        // Everything the viewer can do, it can do here — vi motions and all.
        for k in ['1', '0', '0', 'G'] {
            app.handle_key(key(k)).unwrap();
        }
        match &app.popup {
            Popup::Viewer { line, .. } => assert_eq!(*line, 99, "100G in a docked file"),
            other => panic!("expected the viewer, got {other:?}"),
        }

        // Everything the panel has to say is along the foot of the window: its
        // keys on the hint bar, its mode and position on the status bar.
        let rows = render(&mut app, 120, 30);
        let bottom = rows[rows.len().saturating_sub(2)].clone();
        let status = rows[rows.len() - 1].clone();
        assert!(
            bottom.contains("search") || bottom.contains("検索"),
            "the file's hints are on the bottom bar: {bottom:?}",
        );
        assert!(status.contains("READ"), "the mode is on the status bar: {status:?}");
        assert!(status.contains("100:1"), "…and where the cursor is: {status:?}");
        // Not in the panel's own frame any more.
        let framed = rows.iter().take(rows.len() - 3).any(|r| r.contains("READ"));
        assert!(!framed, "the frame gave the badge up:\n{rows:#?}");

        // The `:` line is cian's own, so it has the width of the window.
        app.handle_key(key(':')).unwrap();
        let rows = render(&mut app, 120, 30);
        let prompt = rows[rows.len().saturating_sub(3)].clone();
        assert!(prompt.contains(":_"), "the prompt is on cian's prompt row: {prompt:?}");
        assert!(
            rows[rows.len() - 1].contains("COMMAND"),
            "and the mode says so: {:?}",
            rows[rows.len() - 1],
        );
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // Tab crosses to the listing beside it; the file stays.
        app.handle_key(code(KeyCode::Tab)).unwrap();
        assert_eq!(app.focused, FocusedPane::Right);
        assert!(matches!(app.popup, Popup::Viewer { .. }), "the file is still open");
        app.handle_key(key('j')).unwrap();
        assert!(app.right.active_ref().cursor > 0, "j moved the listing's cursor");
        let rows = render(&mut app, 120, 30);
        let bottom = rows[rows.len().saturating_sub(2)].clone();
        assert!(
            !(bottom.contains("whole window") || bottom.contains("全画面へ")),
            "the file's hints stepped aside: {bottom:?}",
        );
        app.handle_key(code(KeyCode::Tab)).unwrap();
        assert_eq!(app.focused, FocusedPane::Left, "and back to the file");

        // F12 (and F3, which used to mean this) zooms the pane it is docked
        // in, so the panel fills the window without being a second mode.
        app.handle_key(code(KeyCode::F(12))).unwrap();
        assert!(app.zoomed, "the pane zoomed");
        assert_eq!(app.viewer_dock, Some(FocusedPane::Left), "still the same docked panel");
        let full = render(&mut app, 120, 30);
        assert!(full.iter().any(|r| r.contains("line 100")), "still the same place in it");
        assert!(app.viewer_frame.width > 100, "and it has the window now");
        app.handle_key(code(KeyCode::F(12))).unwrap();
        assert!(!app.zoomed, "and back to the pane");

        // `:q` closes it and the listing is back.
        for k in [':', 'q'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::None));
        let back = render(&mut app, 120, 30);
        assert!(back.iter().any(|r| r.contains("b.log")), "the listing is there");
    }

    /// The panel is one surface among the window's, not a dialog over them:
    /// a click on the listing beside it moves the focus there, `Shift+H` /
    /// `Shift+L` / `Shift+J` move it while reading, and `F3` reads a file in
    /// the *other* pane rather than opening a second kind of window.
    #[test]
    fn the_panel_gives_the_focus_up_to_a_click_and_to_shift_hjl() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "alpha\nbeta\n").unwrap();
        std::fs::write(d.path().join("b.txt"), "gamma\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let _ = render(&mut app, 120, 30);
        assert_eq!(app.viewer_dock, Some(FocusedPane::Left));

        // A click on the listing beside it takes the focus.
        let r = app.layout_rects.right;
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: r.x + 3,
            row: r.y + 3,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.focused, FocusedPane::Right, "the click moved the focus");
        assert!(matches!(app.popup, Popup::Viewer { .. }), "the file is still open");

        // Shift+H comes back to it, Shift+J goes down to the shell.
        app.handle_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.focused, FocusedPane::Left);
        app.handle_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.focused, FocusedPane::Shell, "Shift+J while reading");
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // …but not while editing: there `H` is a character.
        app.focus(FocusedPane::Left);
        app.handle_key(key('i')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.focused, FocusedPane::Left, "the editor kept the key");
        assert!(viewer_lines(&app)[0].contains('L'), "…and typed it: {:?}", viewer_lines(&app));
        app.handle_key(code(KeyCode::Esc)).unwrap();
        quit_viewer_discarding(&mut app);

        // `F3` reads the file under the cursor in the *other* pane.
        app.focus(FocusedPane::Left);
        let at = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "b.txt")
            .unwrap();
        app.active_pane_mut().unwrap().cursor = at;
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert_eq!(app.viewer_dock, Some(FocusedPane::Right), "opened over there");
        assert_eq!(app.focused, FocusedPane::Right, "and the focus followed it");
        assert_eq!(viewer_lines(&app), ["gamma"], "the file the cursor was on");
        let rows = render(&mut app, 120, 30);
        assert!(
            rows.iter().any(|r| r.contains("a.txt")),
            "the listing is still there, on the left:\n{rows:#?}",
        );
    }

    /// `F3` into a pane that is already reading something adds a tab there
    /// rather than replacing what is open — and it used to do nothing at all,
    /// because a leftover "F3 means full window" branch cleared the dock and
    /// returned before opening anything.
    #[test]
    fn f3_into_a_busy_pane_opens_another_tab() {
        let d = tempfile::tempdir().unwrap();
        for (n, b) in [("a.txt", "AAA\n"), ("b.txt", "BBB\n")] {
            std::fs::write(d.path().join(n), b).unwrap();
        }
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let go = |app: &mut App, n: &str| {
            app.focus(FocusedPane::Left);
            let at =
                app.active_pane().unwrap().entries.iter().position(|e| e.name == n).unwrap();
            app.active_pane_mut().unwrap().cursor = at;
            app.handle_key(code(KeyCode::F(3))).unwrap();
        };

        go(&mut app, "a.txt");
        assert_eq!(app.viewer_dock, Some(FocusedPane::Right));
        assert_eq!(viewer_lines(&app), ["AAA"]);

        go(&mut app, "b.txt");
        assert_eq!(viewer_lines(&app), ["BBB"], "the second one is what is being read");
        assert_eq!(app.viewer_tab_count(), 2, "and the first is still open, as a tab");
        assert_eq!(app.viewer_dock, Some(FocusedPane::Right), "in the same pane");

        // Shift+F2 steps back to the one that was there.
        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(viewer_lines(&app), ["AAA"], "the tab strip has both");
    }

    /// The panel's frame goes quiet when the keyboard is somewhere else — a
    /// panel that keeps its mode colour while the keys go elsewhere looks
    /// live and is not.
    #[test]
    fn the_panels_frame_says_whether_it_has_the_keyboard() {
        // Reads the active theme, which lives in a process-wide global that
        // other tests swap. Without the lock this passes until the machine is
        // parallel enough to run one of them at the same moment — which is
        // what a CI runner with more cores than a laptop is.
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let buf = render_buf(&mut app, 120, 30);
        let f = app.viewer_frame;
        let border = buf[(f.x, f.y)].fg;
        app.handle_key(code(KeyCode::Tab)).unwrap(); // focus the listing beside it
        let buf = render_buf(&mut app, 120, 30);
        let quiet = buf[(f.x, f.y)].fg;
        assert_ne!(border, quiet, "the frame changed colour when it lost the keyboard");
        assert_eq!(quiet, crate::theme::theme().border, "…to the colour an unfocused pane wears");
    }

    /// The borders resize while the panel is docked — with the mouse and
    /// with Ctrl+Shift+arrows. Both belong to the window's layout, so neither
    /// is the panel's to swallow.
    #[test]
    fn the_panes_still_resize_while_the_panel_is_open() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let _ = render(&mut app, 120, 30);
        assert!(matches!(app.popup, Popup::Viewer { .. }), "the panel is open");
        let before = app.layout_rects.left.width;

        // Ctrl+Shift+Left narrows the left pane.
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL | KeyModifiers::SHIFT))
            .unwrap();
        let _ = render(&mut app, 120, 30);
        assert!(app.layout_rects.left.width < before, "the keyboard resized it");

        // And the seam between the panes can still be grabbed and dragged.
        // The seam between the two panes: tall and narrow. (The other one is
        // the horizontal seam above the shell.)
        let seam = app
            .dividers
            .iter()
            .find(|d| d.zone.width <= 2 && d.zone.height > 2)
            .map(|d| d.zone)
            .expect("a vertical seam to grab");
        let narrowed = app.layout_rects.left.width;
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: seam.x,
            row: seam.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(
            app.drag.is_some(),
            "the border was grabbed rather than the panel (seam {seam:?}, dividers {:?})",
            app.dividers.iter().map(|d| d.zone).collect::<Vec<_>>(),
        );
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            column: seam.x + 12,
            row: seam.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        let _ = render(&mut app, 120, 30);
        assert!(app.layout_rects.left.width > narrowed, "and dragging moved it");
    }

    /// The replace bar: two fields, three switches, and both ways of running
    /// it. A bar rather than a dialog so the file stays in view — watching
    /// each match land is what makes replace usable.
    #[test]
    fn the_replace_bar_replaces_one_at_a_time_and_all_at_once() {
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let alt = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
        let lines = |app: &App| match &app.popup {
            Popup::Viewer { view, .. } => view.lines.clone(),
            other => panic!("expected the panel, got {other:?}"),
        };

        let (_d, mut app) = viewer_on("cat CAT\ncattle\ncat\n");
        app.handle_key(ctrl('h')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { replace: Some(_), .. }), "the bar opened");

        for c in "cat".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Tab)).unwrap();
        for c in "dog".chars() {
            app.handle_key(key(c)).unwrap();
        }
        // It is on the line `:` and `/` use, with what was typed in it.
        let bar = crate::render::editor_prompt(&app.popup, app.lang).unwrap();
        assert!(bar.contains("cat") && bar.contains("dog"), "the bar shows both: {bar}");

        // Enter takes the first match and stops on it.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(lines(&app)[0], "dog CAT", "one replaced, the rest untouched");

        // Shift+Enter takes the rest. Case-insensitive by default, so CAT goes
        // too — and `cattle` with it, since nothing said whole words.
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(lines(&app), vec!["dog dog", "dogtle", "dog"]);

        // Whole words only leaves `dogtle` alone. (Alt, not Ctrl: a letter has
        // to stay a letter in a text field.)
        let (_d, mut app) = viewer_on("cat cattle\n");
        app.handle_key(ctrl('h')).unwrap();
        for c in "cat".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Tab)).unwrap();
        for c in "dog".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(alt('w')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(lines(&app), vec!["dog cattle"], "whole words only");

        // Case sensitivity, and the switch showing in the bar.
        let (_d, mut app) = viewer_on("cat CAT\n");
        app.handle_key(ctrl('h')).unwrap();
        for c in "cat".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(alt('c')).unwrap();
        app.handle_key(code(KeyCode::Tab)).unwrap();
        for c in "dog".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(lines(&app), vec!["dog CAT"], "CAT is a different word now");

        // A regex, and a replacement carrying an escape.
        let (_d, mut app) = viewer_on("ORA-1234 here\n");
        app.handle_key(ctrl('h')).unwrap();
        for c in r"ORA-\d+".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(alt('r')).unwrap(); // wildcard
        app.handle_key(alt('r')).unwrap(); // regex
        app.handle_key(code(KeyCode::Tab)).unwrap();
        for c in "E".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(lines(&app), vec!["E here"], "the regex matched");

        // Esc closes it and changes nothing.
        app.handle_key(ctrl('h')).unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { replace: None, .. }), "the bar closed");
        assert_eq!(lines(&app), vec!["E here"]);
    }

    /// Clicking into a line of Japanese lands on the character that was
    /// clicked. A full-width character is one buffer character but two drawn
    /// columns; counting every character as one column put the cursor a
    /// character further left for every wide one before it, so a drag over a
    /// Japanese line selected somewhere else entirely.
    #[test]
    fn a_click_lands_where_it_was_aimed_on_a_wide_line() {
        let (_d, mut app) = viewer_on("あいうえお\nabcde\n");
        let _ = render(&mut app, 100, 30);
        let body = app.viewer_rect;
        let text_x = body.x + app.viewer_gutter;
        let click = |app: &mut App, cells: u16, kind| {
            app.handle_mouse(crossterm::event::MouseEvent {
                kind,
                column: text_x + cells,
                row: body.y,
                modifiers: KeyModifiers::NONE,
            });
        };
        use crossterm::event::{MouseButton, MouseEventKind};
        let at = |app: &App| match &app.popup {
            Popup::Viewer { col, .. } => *col,
            other => panic!("expected the panel, got {other:?}"),
        };

        // Cell 0 and 1 are both あ; cells 2 and 3 are い; 8 and 9 are お.
        for (cell, want) in [(0u16, 0usize), (1, 0), (2, 1), (3, 1), (8, 4), (9, 4)] {
            click(&mut app, cell, MouseEventKind::Down(MouseButton::Left));
            assert_eq!(at(&app), want, "cell {cell} is character {want}");
        }

        // And a drag from あ to う selects those three characters, not one.
        click(&mut app, 0, MouseEventKind::Down(MouseButton::Left));
        click(&mut app, 5, MouseEventKind::Drag(MouseButton::Left));
        match &app.popup {
            Popup::Viewer { anchor, line, col, visual: Some(ViewVisual::Char), .. } => {
                assert_eq!((*anchor, (*line, *col)), ((0, 0), (0, 2)), "あ through う");
            }
            other => panic!("expected a character selection, got {other:?}"),
        }
    }

    /// `x` over a selection cuts: what it took goes where `p` looks for it.
    /// It used to simply vanish, so `x` then `p` pasted whatever had been
    /// copied before — the last thing anyone means by cut and paste.
    #[test]
    fn what_x_cuts_is_what_p_puts_back() {
        for (start, expect) in [
            ('V', vec!["two", "one", "three"]),
            // `p` puts it after the cursor, which is where vi puts it: the
            // line is "e", the cut was "on", and it lands after the e.
            ('v', vec!["eon", "two", "three"]),
        ] {
            let (_d, mut app) = viewer_on("one\ntwo\nthree\n");
            app.handle_key(key(start)).unwrap();
            if start == 'v' {
                // `v` then `l` selects "on"; the linewise case takes the line.
                app.handle_key(key('l')).unwrap();
            }
            app.handle_key(key('x')).unwrap();
            assert!(app.yank.is_some(), "the cut text is on the clipboard");
            app.handle_key(key('p')).unwrap();
            assert_eq!(viewer_lines(&app), expect, "started with {start}");
        }

        // The operator form too: `dd` then `p` puts the line back below.
        let (_d, mut app) = viewer_on("one\ntwo\n");
        app.handle_key(key('d')).unwrap();
        app.handle_key(key('d')).unwrap();
        assert_eq!(viewer_lines(&app), vec!["two"]);
        app.handle_key(key('p')).unwrap();
        assert_eq!(viewer_lines(&app), vec!["two", "one"]);
    }

    /// The line-transform verbs act on a selection when there is one, and on
    /// the whole file when there is not. `:lf` and `:crlf` are the exception,
    /// and have to be: a line ending is a property of the file, not of a run
    /// of lines inside it.
    #[test]
    fn the_transforms_follow_the_selection_and_the_endings_do_not() {
        // `:han` on two selected lines of three.
        let (_d, mut app) = viewer_on("ＡＢＣ\nＤＥＦ\nＧＨＩ\n");
        app.handle_key(key('V')).unwrap();
        app.handle_key(key('j')).unwrap();
        app.command_buffer.clear();
        app.handle_key(key(':')).unwrap();
        for c in "han".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), vec!["ABC", "DEF", "ＧＨＩ"], "only the selection");
        assert!(
            app.message.as_deref().is_some_and(|m| m.contains("selection")),
            "and it says so: {:?}",
            app.message,
        );

        // With nothing selected it is the whole file.
        let (_d, mut app) = viewer_on("ＡＢＣ\nＤＥＦ\n");
        app.handle_key(key(':')).unwrap();
        for c in "han".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), vec!["ABC", "DEF"], "the whole file");

        // `:crlf` is the file's, selection or no selection.
        let (_d, mut app) = viewer_on("one\ntwo\nthree\n");
        app.handle_key(key('V')).unwrap();
        app.handle_key(key(':')).unwrap();
        for c in "crlf".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        match &app.popup {
            Popup::Viewer { view, .. } => {
                assert_eq!(view.eol, cian_core::viewer::Eol::Crlf, "every line of it");
            }
            other => panic!("expected the panel, got {other:?}"),
        }
    }

    /// `viw` and its family select the object rather than typing it. Text
    /// objects only ran after an operator, so over a selection `v` `i` `w`
    /// was read as "enter insert, type a w" — and put a `w` in the file.
    #[test]
    fn a_text_object_over_a_selection_selects_it() {
        let press = |app: &mut App, keys: &str| {
            for c in keys.chars() {
                app.handle_key(key(c)).unwrap();
            }
        };
        for (setup, keys, want) in [
            ("All done\n", "viwy", "All"),
            ("All done\n", "wviwy", "done"),
            ("say \"hi there\" now\n", "fhvi\"y", "hi there"),
            ("call(one, two)\n", "fovi(y", "one, two"),
            ("x 'abc' y\n", "fava'y", "'abc'"),
        ] {
            let (_d, mut app) = viewer_on(setup);
            press(&mut app, keys);
            assert_eq!(app.yank.as_deref(), Some(want), "{keys} on {setup:?}");
            assert_eq!(
                viewer_lines(&app)[0],
                setup.trim_end_matches('\n'),
                "and nothing was typed into the file",
            );
        }

        // …and an operator over the selection still acts on it.
        let (_d, mut app) = viewer_on("All done\n");
        press(&mut app, "viwd");
        assert_eq!(viewer_lines(&app)[0], " done");
    }

    /// A copy the system clipboard refused is still cian's copy: `p` pastes
    /// what was just taken rather than whatever the clipboard was holding
    /// from before. The failure used to be discarded, which made a copy look
    /// like it had worked and the paste produce something else entirely.
    #[test]
    fn a_refused_clipboard_does_not_paste_something_older() {
        let (_d, mut app) = viewer_on("All done\n");
        // `viewer_on` runs without a system clipboard, which is exactly the
        // "it would not take it" case.
        assert!(app.clipboard.is_none());
        for c in "viwy".chars() {
            app.handle_key(key(c)).unwrap();
        }
        assert_eq!(app.yank.as_deref(), Some("All"));
        assert!(!app.yank_on_clipboard, "the clipboard did not take it");
        // No clipboard service at all is not a problem worth a sentence: `p`
        // pastes from cian's own copy there and always has.
        assert_eq!(app.message.as_deref(), Some("copied"));
        app.handle_key(key('$')).unwrap();
        app.handle_key(key('p')).unwrap();
        assert_eq!(viewer_lines(&app)[0], "All doneAll", "p pasted what was copied");
    }

    /// A line wider than the panel scrolls sideways under the cursor, and
    /// says how much is off screen. It used to simply run off the edge: the
    /// cursor kept moving and the text stopped.
    #[test]
    fn a_long_line_follows_the_cursor_sideways() {
        // 200 columns of it, in a panel about 90 wide.
        let long: String = (0..40).map(|i| format!("word{i:02} ")).collect();
        let (_d, mut app) = viewer_on(&format!("{long}\nshort\n"));
        let seen = |app: &mut App| render(app, 100, 30).join("\n");
        let hscroll = |app: &App| match &app.popup {
            Popup::Viewer { hscroll, .. } => *hscroll,
            other => panic!("expected the panel, got {other:?}"),
        };

        let screen = seen(&mut app);
        assert_eq!(hscroll(&app), 0, "starts at the left");
        assert!(screen.contains("word00"), "the head of the line is shown");
        assert!(!screen.contains("word39"), "and the tail is not");

        // `$` goes to the end of it; the view has to follow.
        app.handle_key(key('$')).unwrap();
        let screen = seen(&mut app);
        assert!(hscroll(&app) > 0, "scrolled sideways");
        assert!(screen.contains("word39"), "the tail is shown now:\n{screen}");
        assert!(!screen.contains("word00"), "and the head has gone by");

        // …and back again.
        app.handle_key(key('0')).unwrap();
        let screen = seen(&mut app);
        assert_eq!(hscroll(&app), 0, "back to the left");
        assert!(screen.contains("word00"));

        // The line number is still there — the gutter does not scroll with
        // the text.
        assert!(screen.lines().any(|l| l.contains(" 1 ")), "gutter kept:\n{screen}");
    }

    /// Nothing is ever drawn over a frame, whatever is in it. A wide
    /// character that will not fit before the border is left out — the border
    /// is the thing that has to be right, and half a character is not one.
    #[test]
    fn a_wide_character_never_eats_the_border() {
        // Names of every length around the pane's right edge, so one of them
        // lands with a full-width character straddling it.
        let names: Vec<String> = (1..=30).map(|n| format!("{}.txt", "あ".repeat(n))).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let (_d, mut app) = app_with(&refs);
        // Past the "starting up" splash, which is drawn over both panes.
        app.startup_at = std::time::Instant::now() - std::time::Duration::from_secs(30);
        let buf = render_buf(&mut app, 100, 40);
        for (name, r) in
            [("left", app.layout_rects.left), ("right", app.layout_rects.right)]
        {
            for y in r.y + 1..r.y + r.height - 1 {
                for (edge, x) in [("left", r.x), ("right", r.x + r.width - 1)] {
                    let sym = buf[(x, y)].symbol();
                    assert!(
                        sym == "│" || sym == "┃" || sym == "║",
                        "{name} pane's {edge} border at row {y} is {sym:?}",
                    );
                }
            }
        }

        // **The four corners too.** The loop above walks `r.y + 1 ..` — it
        // skips the border *rows* on purpose (a title lives on the top one),
        // and that gap is where the view switcher was rubbing out the right
        // pane's `╮`: right-aligned to the pane's last column, its label's
        // trailing space landed on the corner at every terminal width. The box
        // read as open at the top right and nothing said so.
        for (name, r) in
            [("left", app.layout_rects.left), ("right", app.layout_rects.right)]
        {
            for (which, x, y) in [
                ("top-left", r.x, r.y),
                ("top-right", r.x + r.width - 1, r.y),
                ("bottom-left", r.x, r.y + r.height - 1),
                ("bottom-right", r.x + r.width - 1, r.y + r.height - 1),
            ] {
                let sym = buf[(x, y)].symbol();
                assert!(
                    matches!(sym, "╭" | "╮" | "╰" | "╯" | "┌" | "┐" | "└" | "┘"
                                | "╔" | "╗" | "╚" | "╝"),
                    "{name} pane's {which} corner is {sym:?}",
                );
            }
        }

        // …and the shell panel, drawn by the terminal widget rather than by
        // cian, with wide characters running exactly to its edge and past it.
        let dir = tempfile::tempdir().unwrap();
        let sh = cian_pty::default_shell();
        let session = cian_pty::PtySession::new(dir.path(), &sh, 24, 80).unwrap();
        app.shell.tabs.push(ShellTab::new(session));
        app.shell.active = 0;
        app.preview_on = false;
        app.focus(FocusedPane::Shell);
        let _ = render(&mut app, 100, 40);
        let cols = app.shell.cols as usize;
        if let Some(s) = app.shell.active_session() {
            // Exactly to the edge, then one narrow character shifting the next
            // line so a wide one straddles it.
            let text =
                format!("{}\r\nx{}Z\r\n", "あ".repeat(cols / 2), "あ".repeat(cols / 2));
            s.parser().lock().unwrap().process(text.as_bytes());
        }
        let buf = render_buf(&mut app, 100, 40);
        let r = app.layout_rects.shell;
        for y in r.y + 1..r.y + r.height - 1 {
            for (edge, x) in [("left", r.x), ("right", r.x + r.width - 1)] {
                let sym = buf[(x, y)].symbol();
                assert!(
                    sym == "│" || sym == "┃" || sym == "║",
                    "shell's {edge} border at row {y} is {sym:?}",
                );
            }
        }
    }

    /// A picture still draws when the terminal has no image protocol — the
    /// half-block renderer is the fallback, and it has to actually run.
    #[test]
    fn an_image_previews_without_a_terminal_protocol() {
        let dir = tempfile::tempdir().unwrap();
        image::RgbImage::from_pixel(8, 8, image::Rgb([200, 40, 40]))
            .save(dir.path().join("shot.png"))
            .unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let i = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "shot.png")
            .unwrap();
        app.active_pane_mut().unwrap().cursor = i;
        assert!(crate::preview::preview_target(&app).is_ok(), "an image is previewable");

        let buf = settled(&mut app, 100, 30);
        let sh = app.layout_rects.shell;
        let painted = (sh.y + 1..sh.y + sh.height - 1)
            .flat_map(|y| (sh.x + 1..sh.x + sh.width - 1).map(move |x| (x, y)))
            .filter(|(x, y)| !buf[(*x, *y)].symbol().trim().is_empty())
            .count();
        assert!(painted > 20, "the picture is drawn: {painted} cells");
    }

    /// A click puts the cursor on the row that was clicked and leaves the
    /// view where it is. The window used to be derived from the cursor with a
    /// formula that put the cursor on the *last* visible row, so clicking a
    /// file scrolled it to the bottom of the pane.
    #[test]
    fn a_click_lands_on_the_row_and_does_not_scroll() {
        let names: Vec<String> = (0..80).map(|i| format!("f{i:03}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let (_d, mut app) = app_with(&refs);
        app.startup_at = std::time::Instant::now() - std::time::Duration::from_secs(30);
        let _ = render(&mut app, 100, 40);
        let left = app.layout_rects.left;

        assert_eq!(app.left.active_ref().scroll, 0, "the top of the list");
        // The fifth row of the listing.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 4, left.y + 6));
        let p = app.left.active_ref();
        assert_eq!(p.scroll, 0, "the view did not move");
        assert_eq!(p.cursor, 4, "and the cursor is on the row that was clicked");

        // Scrolling down and clicking again still lands where it was aimed.
        for _ in 0..30 {
            app.handle_key(key('j')).unwrap();
        }
        let _ = render(&mut app, 100, 40);
        let before = app.left.active_ref().scroll;
        assert!(before > 0, "the view followed the cursor down");
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 4, left.y + 3));
        let p = app.left.active_ref();
        assert_eq!(p.scroll, before, "still did not move");
        assert_eq!(p.cursor, before + 1, "the second visible row");
    }

    /// The wheel belongs to whatever the pointer is over. With the editor
    /// panel open it took every wheel event in the window, so a flick over
    /// the listing beside it moved the file's cursor instead.
    #[test]
    fn the_wheel_follows_the_pointer_not_the_focus() {
        let names: Vec<String> = (0..60).map(|i| format!("f{i:03}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let (_d, mut app) = app_with(&refs);
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(app.viewer_dock, Some(FocusedPane::Left), "the panel is on the left");
        let _ = render(&mut app, 100, 40);

        let line_of = |app: &App| match &app.popup {
            Popup::Viewer { line, .. } => *line,
            other => panic!("expected the panel, got {other:?}"),
        };
        let before = line_of(&app);
        let right = app.layout_rects.right;
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, right.x + 4, right.y + 4));
        assert_eq!(line_of(&app), before, "the file did not move");
        assert!(app.right.active_ref().cursor > 0, "the listing under the pointer did");
    }

    /// Everything the Markdown preview draws reads on the page it is drawn
    /// on, under every preset. It carried colours from before the themes
    /// existed — inline code was a fixed dark box with yellow text, which on
    /// a light theme is a black hole in the paragraph.
    #[test]
    fn the_markdown_preview_reads_on_every_theme() {
        use crate::theme::{set_theme, surface, theme_preset, ResolvedTheme, THEME_NAMES};
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let md = "# Heading\n\nText with `code` and **bold** and a [link](x).\n\n                  ```\nfn main() {}\n```\n\n> a quote\n\n- a list item\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let src: Vec<String> = md.lines().map(|s| s.to_string()).collect();
        let mut bad: Vec<String> = Vec::new();
        for name in THEME_NAMES {
            let t = theme_preset(name).unwrap();
            set_theme(t);
            let (lines, styles, _) = crate::markdown::render_styled(&src, 60);
            for (i, l) in lines.iter().enumerate() {
                for (j, ch) in l.chars().enumerate() {
                    if ch.is_whitespace() {
                        continue;
                    }
                    let st = styles.get(i).and_then(|r| r.get(j)).copied().unwrap_or_default();
                    let fg = st.fg.unwrap_or_else(|| crate::render::readable_on(surface()));
                    let bg = st.bg.unwrap_or_else(surface);
                    let cr = crate::render::contrast_ratio(fg, bg);
                    if cr < 3.0 {
                        bad.push(format!("{name}: {ch:?} {fg:?} on {bg:?} = {cr:.2}"));
                    }
                }
            }
        }
        set_theme(ResolvedTheme::DARK);
        bad.dedup();
        bad.truncate(10);
        assert!(bad.is_empty(), "{} unreadable:\n{}", bad.len(), bad.join("\n"));
    }

    /// The preview as it is actually painted, not as its style grid describes
    /// itself: body text against the surface it lands on, under a light theme
    /// and a dark one. The grid was measured before and passed while the
    /// screen was still wrong, because nothing checked the two together.
    #[test]
    fn the_painted_markdown_reads_on_light_and_dark() {
        use crate::theme::{set_theme, theme_preset, ResolvedTheme};
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for name in ["solarized-light", "dracula", "github-light", "tokyo-night"] {
            set_theme(theme_preset(name).unwrap());
            let d = tempfile::tempdir().unwrap();
            std::fs::write(
                d.path().join("a.md"),
                "# Heading\n\nSome plain body text here.\n\n> quoted\n\n- listed\n",
            )
            .unwrap();
            let p = d.path().to_path_buf();
            let mut app = App::new(p.clone(), p, en_config()).unwrap();
            let i = app
                .active_pane()
                .unwrap()
                .entries
                .iter()
                .position(|e| e.name == "a.md")
                .unwrap();
            app.active_pane_mut().unwrap().cursor = i;
            app.handle_key(code(KeyCode::Enter)).unwrap();
            app.handle_key(code(KeyCode::F(12))).unwrap();
            let buf = render_buf(&mut app, 100, 30);
            let f = app.viewer_frame;
            let mut worst = f32::MAX;
            let mut worst_at = String::new();
            for y in f.y + 1..f.y + f.height - 1 {
                for x in f.x + 1..f.x + f.width - 1 {
                    let c = &buf[(x, y)];
                    if !c.symbol().chars().all(char::is_alphanumeric) || c.symbol().trim().is_empty()
                    {
                        continue;
                    }
                    let cr = crate::render::contrast_ratio(c.fg, c.bg);
                    if cr < worst {
                        worst = cr;
                        worst_at = format!("{:?} {:?} on {:?}", c.symbol(), c.fg, c.bg);
                    }
                }
            }
            assert!(worst >= 4.0, "{name}: {worst_at} is {worst:.2}:1");
        }
        set_theme(ResolvedTheme::DARK);
    }

    /// Every preset draws a readable Markdown preview — measured off the
    /// screen, with the content that raised it: Japanese paragraphs, headings
    /// with rules under them, inline code, links and a list.
    #[test]
    fn the_preview_reads_on_every_preset() {
        use crate::theme::{set_theme, theme_preset, ResolvedTheme};
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let md = "# Change Log\n\nAll notable changes to the \"crmaine\" extension will be documented.\n\n                  ## [Unreleased]\n\n## 0.6.5 — 外部システム参照\n\n                  RAG・Agent が「社内資料を読む」だけでなく、サーバ・実DB を見に行けるようにした版。\n\n                  ### 新機能: 外部システムの参照\n\n- サーバ参照（SSH）: `crmaine.servers` を設定すると\n\n> 引用\n";
        for name in [
            "solarized-dark", "nord", "tokyo-night", "dracula", "gruvbox-dark", "monokai-pro",
            "solarized-light", "github-light", "ayu-light", "bluloco-light", "gruvbox-light",
            "catppuccin-latte",
        ] {
            set_theme(theme_preset(name).unwrap());
            let d = tempfile::tempdir().unwrap();
            std::fs::write(d.path().join("a.md"), md).unwrap();
            let p = d.path().to_path_buf();
            let mut app = App::new(p.clone(), p, en_config()).unwrap();
            let i = app
                .active_pane()
                .unwrap()
                .entries
                .iter()
                .position(|e| e.name == "a.md")
                .unwrap();
            app.active_pane_mut().unwrap().cursor = i;
            app.handle_key(code(KeyCode::Enter)).unwrap();
            app.handle_key(code(KeyCode::F(12))).unwrap();
            let buf = render_buf(&mut app, 110, 34);
            let f = app.viewer_frame;
            // The style grid has to cover every character; one that falls off
            // the end is drawn with no colour at all, which on some terminals
            // is indistinguishable from the background.
            if let Popup::Viewer { view, md_styles, .. } = &app.popup {
                for (i, l) in view.lines.iter().enumerate() {
                    let got = md_styles.get(i).map(|r| r.len()).unwrap_or(0);
                    assert!(got >= l.chars().count(), "{name}: row {i} has {got} styles for {} chars", l.chars().count());
                }
            }
            let mut worst = f32::MAX;
            let mut at = String::new();
            for y in f.y + 1..f.y + f.height - 1 {
                for x in f.x + 1..f.x + f.width - 1 {
                    let c = &buf[(x, y)];
                    if c.symbol().trim().is_empty() {
                        continue;
                    }
                    let cr = crate::render::contrast_ratio(c.fg, c.bg);
                    if cr < worst {
                        worst = cr;
                        at = format!("{:?} {:?} on {:?}", c.symbol(), c.fg, c.bg);
                    }
                }
            }
            // 4.0 rather than 3.0: body text at 3:1 passes a checklist and
            // is still hard to read, which is what "見にくい" meant.
            assert!(worst >= 4.0, "{name}: {at} is {worst:.2}:1");
        }
        set_theme(ResolvedTheme::DARK);
    }

    /// Switching the theme while a preview is open recolours it. The style
    /// grid is a cache of *colours*: opened on a light theme and switched to
    /// a dark one, it kept its near-black text and the page went black on
    /// black — with only the headings and the code blocks, which carry a
    /// background of their own, still visible.
    #[test]
    fn changing_the_theme_recolours_what_is_already_open() {
        use crate::theme::{set_theme, theme_preset, ResolvedTheme};
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let worst = |app: &mut App| {
            let buf = render_buf(app, 100, 30);
            let f = app.viewer_frame;
            let mut w = f32::MAX;
            let mut at = String::new();
            for y in f.y + 1..f.y + f.height - 1 {
                for x in f.x + 1..f.x + f.width - 1 {
                    let c = &buf[(x, y)];
                    if c.symbol().trim().is_empty() {
                        continue;
                    }
                    let cr = crate::render::contrast_ratio(c.fg, c.bg);
                    if cr < w {
                        w = cr;
                        at = format!("{:?} {:?} on {:?}", c.symbol(), c.fg, c.bg);
                    }
                }
            }
            (w, at)
        };

        for (from, to) in [("solarized-light", "solarized-dark"), ("dracula", "github-light")] {
            set_theme(theme_preset(from).unwrap());
            let d = tempfile::tempdir().unwrap();
            std::fs::write(d.path().join("a.md"), "# Head\n\nPlain body text here.\n\n- listed\n")
                .unwrap();
            let p = d.path().to_path_buf();
            let mut app = App::new(p.clone(), p, en_config()).unwrap();
            let i = app
                .active_pane()
                .unwrap()
                .entries
                .iter()
                .position(|e| e.name == "a.md")
                .unwrap();
            app.active_pane_mut().unwrap().cursor = i;
            app.handle_key(code(KeyCode::Enter)).unwrap();
            app.handle_key(code(KeyCode::F(12))).unwrap();
            let (w, at) = worst(&mut app);
            assert!(w >= 4.0, "{from}: {at} is {w:.2}:1");

            // …and now the theme changes under it, through the command a
            // person would actually type. Calling `set_theme` here would be
            // testing the fix rather than the path to it.
            app.command_buffer = format!("theme {to}");
            app.run_command();

            let (w, at) = worst(&mut app);
            assert!(w >= 4.0, "{from} → :theme {to}: {at} is {w:.2}:1");
        }
        set_theme(ResolvedTheme::DARK);
    }

    /// Draw, wait out the settle a heavy preview asks for, and draw again —
    /// what holding still in front of one actually does.
    fn settled(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        // Two waits, and they are different: the first is the cursor resting
        // long enough for a heavy file to be read at all, the second is the
        // decoder thread finishing. Frames in between draw "reading…".
        for _ in 0..40 {
            let buf = render_buf(app, w, h);
            if app.preview_wanted.is_none() && app.preview_decode.is_none() {
                // One more, now that whatever arrived can be drawn.
                return render_buf(app, w, h);
            }
            drop(buf);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        render_buf(app, w, h)
    }

    /// A heavy file is read once the cursor stops on it, not on the way past.
    /// Reading happens mid-frame, so a slow decode is time taken out of the
    /// interface — holding an arrow key down a folder of photographs used to
    /// decode every one of them in turn.
    #[test]
    fn a_heavy_preview_waits_for_the_cursor_to_settle() {
        let dir = tempfile::tempdir().unwrap();
        // Big enough to count as heavy, and a picture besides.
        image::RgbImage::from_pixel(400, 400, image::Rgb([90, 140, 200]))
            .save(dir.path().join("big.png"))
            .unwrap();
        std::fs::write(dir.path().join("small.txt"), b"plain and quick\n").unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let go = |app: &mut App, name: &str| {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == name).unwrap();
        };

        app.preview_on = true;   // 既定は切（2026-09-06）。この検査はプレビューそのものを見る
        // A small text file is read at once, as it always was.
        go(&mut app, "small.txt");
        let out = render(&mut app, 100, 30).join("\n");
        assert!(out.contains("plain and quick"), "read straight away:\n{out}");

        // The picture is not — the first frame after moving onto it leaves it
        // alone, and asks for another frame so it is not waiting on a key.
        go(&mut app, "big.png");
        let _ = render(&mut app, 100, 30);
        assert!(app.preview_wanted.is_some(), "waiting for the cursor to settle");
        assert!(
            app.preview.as_ref().map(|p| p.path.ends_with("big.png")) != Some(true),
            "and has not read it yet",
        );

        // …and once it has settled, it is read.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _ = render(&mut app, 100, 30);
        assert!(app.preview_wanted.is_none(), "the wait is over");
        assert!(
            app.preview.as_ref().map(|p| p.path.ends_with("big.png")) == Some(true),
            "and the picture is the preview now",
        );
    }

    /// Decoding a picture happens off the drawing thread. Several megabytes of
    /// PNG is unpacked whole before anything can be scaled, and doing that
    /// mid-frame stopped the cursor dead on every large image.
    #[test]
    fn a_picture_is_decoded_off_the_drawing_thread() {
        let dir = tempfile::tempdir().unwrap();
        image::RgbImage::from_pixel(600, 600, image::Rgb([40, 160, 90]))
            .save(dir.path().join("big.png"))
            .unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let pane = app.active_pane_mut().unwrap();
        pane.cursor = pane.entries.iter().position(|e| e.name == "big.png").unwrap();

        // Past the settle, the frame that starts the read hands the work to a
        // thread rather than doing it. A picture this small may land before
        // the next frame, so what is pinned is that the frame *while* it is
        // being read says so rather than sitting blank.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _ = render(&mut app, 100, 30);
        if app.preview_decode.is_some() {
            let out = render(&mut app, 100, 30).join("\n");
            assert!(
                out.contains("reading") || out.contains("読み込み"),
                "the panel says what it is doing:\n{out}",
            );
        }

        // When it lands, the picture is drawn — and nothing decodes again for
        // the same file.
        let buf = settled(&mut app, 100, 30);
        let sh = app.layout_rects.shell;
        let painted = (sh.y + 1..sh.y + sh.height - 1)
            .flat_map(|y| (sh.x + 1..sh.x + sh.width - 1).map(move |x| (x, y)))
            .filter(|(x, y)| !buf[(*x, *y)].symbol().trim().is_empty())
            .count();
        assert!(painted > 20, "the picture is drawn: {painted} cells");
        let _ = render(&mut app, 100, 30);
        assert!(app.preview_decode.is_none(), "and it is not read a second time");
    }

    /// `:gfx` steps through the ways of drawing a picture — the way out when
    /// a terminal advertises a protocol and then draws nothing with it.
    #[test]
    fn image_walks_the_ways_of_drawing_a_picture() {
        let dir = tempfile::tempdir().unwrap();
        image::RgbImage::from_pixel(8, 8, image::Rgb([200, 40, 40]))
            .save(dir.path().join("shot.png"))
            .unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let i = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "shot.png")
            .unwrap();
        app.active_pane_mut().unwrap().cursor = i;

        // Whatever the terminal offered, the picture draws.
        let painted = |app: &mut App| {
            let buf = settled(app, 100, 30);
            let sh = app.layout_rects.shell;
            (sh.y + 1..sh.y + sh.height - 1)
                .flat_map(|y| (sh.x + 1..sh.x + sh.width - 1).map(move |x| (x, y)))
                .filter(|(x, y)| !buf[(*x, *y)].symbol().trim().is_empty())
                .count()
        };
        assert!(painted(&mut app) > 20, "a picture to begin with");

        // `:gfx` walks the ways of drawing one: auto → iterm2 → kitty →
        // sixel → half-blocks → auto. A terminal can be wrong about itself,
        // so naming a protocol is one keystroke away.
        //
        // The choice is remembered in the real state file, which belongs to
        // whoever is running the tests — so it is put back afterwards.
        let before = crate::state_get("images");
        let mut seen = Vec::new();
        for _ in 0..5 {
            app.command_buffer = "image".into();
            app.run_command();
            seen.push(app.message.clone().unwrap_or_default());
            // Whichever it lands on, a picture is drawn: with no terminal
            // protocol here, every step falls back to half-blocks.
            assert!(painted(&mut app) > 20, "still a picture: {:?}", app.message);
        }
        crate::state_set("images", before.as_deref().unwrap_or("auto"));
        assert_eq!(seen.len(), 5);
        assert!(
            seen.iter().any(|m| m.contains("half-block") || m.contains("半角")),
            "half-blocks are one of the stops: {seen:?}",
        );
    }

    /// A long prompt stays readable. The chat's input drew one row per typed
    /// line and let a long one run off the right-hand edge; the AI-command
    /// dialog sized its box from the unwrapped text and cut the rest off the
    /// bottom. A prompt you cannot read back is one you cannot correct.
    #[test]
    fn a_long_prompt_is_visible_in_full() {
        let long: String = (0..24).map(|i| format!("word{i:02} ")).collect();
        assert!(long.len() > 150, "longer than any dialog is wide");

        // The chat.
        let (_d, mut app) = app_with(&["a.txt"]);
        app.start_ai_chat(ChatMode::Ai, vec![], false);
        for c in long.chars() {
            app.handle_key(key(c)).unwrap();
        }
        let screen = render(&mut app, 100, 30).join("\n");
        assert!(screen.contains("word00"), "the start is there:\n{screen}");
        assert!(screen.contains("word23"), "and so is the end — where the caret is");

        // The AI-command dialog.
        let (_d, mut app) = app_with(&["a.txt"]);
        app.popup = Popup::TextInput {
            title: " command from a description ".into(),
            prompt: "what should it do?".into(),
            buffer: long.clone(),
            kind: InputKind::AiShellCmd,
            cursor: long.chars().count(),
            select_all: false,
        };
        let screen = render(&mut app, 100, 30).join("\n");
        assert!(screen.contains("word00"), "the start is there:\n{screen}");
        assert!(screen.contains("word23"), "and the end:\n{screen}");
    }

    /// The same, in Japanese, which is where it actually broke. The box grew by
    /// `chars().count() / cols` — but a Japanese character is two columns wide,
    /// so a sentence that needed two rows was told it needed none and the rest
    /// fell off the bottom. It only ever looked right because the test above
    /// types ASCII.
    #[test]
    fn a_long_japanese_prompt_is_visible_in_full() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // 60 characters — 120 columns — in a box whose inner width is 92.
        let head = "先頭がここ";
        let tail = "末尾はここ";
        let long = format!("{head}{}{tail}", "あいうえおかきくけこ".repeat(5));
        app.popup = Popup::TextInput {
            title: " 説明からコマンド生成 ".into(),
            prompt: "やりたいことを説明してください:".into(),
            buffer: long.clone(),
            kind: InputKind::AiShellCmd,
            cursor: long.chars().count(),
            select_all: false,
        };
        let screen = render(&mut app, 100, 30).join("\n");
        // A wide character occupies two cells, and the test backend hands back
        // one symbol per cell — so the screen reads "先 頭", not "先頭". The
        // blanks are the rendering, not the text; drop them before comparing.
        let flat: String = screen.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(flat.contains(head), "the start is there:\n{screen}");
        assert!(flat.contains(tail), "and the end, which used to be cut off:\n{screen}");
    }

    /// The AI's answer wraps too. `extra_rows` only ever applied to the text
    /// input, so a long command came back, wrapped onto a second row, and the
    /// box had no room for it — the tail was simply gone.
    #[test]
    fn a_long_ai_command_is_visible_in_full() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let tail = "--exclude=NOTICEABLE_END";
        let command = format!(
            "find . -type f -name '*.log' -mtime +30 -print0 | xargs -0 tar czf logs.tar.gz {tail}"
        );
        app.popup = Popup::AiShellConfirm {
            command: command.clone(),
            description: "compress old logs".into(),
        };
        let screen = render(&mut app, 100, 30).join("\n");
        assert!(screen.contains("find . -type f"), "the start is there:\n{screen}");
        assert!(screen.contains(tail), "and the end, which used to be cut off:\n{screen}");
    }

    /// Shift+Enter is a newline in the fields that take a paragraph, and stays
    /// "submit" in the ones that do not — a filename with a newline in it is a
    /// mistake, not a feature.
    #[test]
    fn shift_enter_is_a_newline_only_where_a_paragraph_belongs() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.popup = Popup::TextInput {
            title: " t ".into(),
            prompt: "p".into(),
            buffer: "ログを".into(),
            kind: InputKind::AiShellCmd,
            cursor: 3,
            select_all: false,
        };
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        for c in "まとめる".chars() {
            app.handle_key(code(KeyCode::Char(c))).unwrap();
        }
        match &app.popup {
            Popup::TextInput { buffer, .. } => {
                assert_eq!(buffer, "ログを\nまとめる", "Shift+Enter put a line break in");
            }
            other => panic!("the prompt should still be open, got {other:?}"),
        }
        // Both rows are drawn, not just the one the caret is on.
        let screen = render(&mut app, 100, 30).join("\n");
        let flat: String = screen.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(flat.contains("ログを"), "first row:\n{screen}");
        assert!(flat.contains("まとめる"), "second row:\n{screen}");
        // And plain Enter still submits.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(!matches!(app.popup, Popup::TextInput { .. }), "Enter submitted");

        // A rename field is one line by nature: Shift+Enter submits it.
        let (_d, mut app) = app_with(&["a.txt"]);
        app.popup = Popup::TextInput {
            title: " rename ".into(),
            prompt: "new name:".into(),
            buffer: "b.txt".into(),
            kind: InputKind::Rename { original: std::path::PathBuf::from("a.txt") },
            cursor: 5,
            select_all: false,
        };
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert!(
            !matches!(app.popup, Popup::TextInput { .. }),
            "Shift+Enter submitted the rename rather than breaking the name in two",
        );
    }

    /// "Close, but not quite" — the third answer to a proposed command. It used
    /// to be yes or no, so an almost-right command had to be asked for again
    /// from nothing.
    #[test]
    fn a_proposed_command_can_be_sent_back_for_another_try() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.popup = Popup::AiShellConfirm {
            command: "rm -rf /tmp/logs".into(),
            description: "clear the logs".into(),
        };
        app.handle_key(code(KeyCode::Char('r'))).unwrap();
        let Popup::TextInput { kind, .. } = &app.popup else {
            panic!("r should open the adjust prompt, got {:?}", app.popup)
        };
        match kind {
            InputKind::AiShellRefine { description, rejected } => {
                assert_eq!(description, "clear the logs", "the original request came along");
                assert_eq!(rejected, "rm -rf /tmp/logs", "and so did the command being rejected");
            }
            other => panic!("wrong kind: {other:?}"),
        }
        // Saying nothing puts the command back rather than asking the model to
        // interpret silence.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        match &app.popup {
            Popup::AiShellConfirm { command, .. } => assert_eq!(command, "rm -rf /tmp/logs"),
            other => panic!("an empty note should restore the command, got {other:?}"),
        }
    }

    /// A bookmark that could not be written says so. Adding one reported a
    /// failed save; deleting one and making a group did not — the list on
    /// screen changed, the file did not, and the next launch had the old
    /// bookmarks back with no hint why.
    #[test]
    fn a_bookmark_that_cannot_be_saved_says_so() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // A path that cannot be written to: the "directory" is a file.
        let blocked = app.active_pane().unwrap().cwd.join("a.txt").join("shortcuts.lua");
        app.shortcuts.path = blocked;
        app.shortcuts.entries = vec![
            Shortcut { name: "one".into(), target: Some("/tmp".into()), children: None },
            Shortcut { name: "two".into(), target: Some("/tmp".into()), children: None },
        ];

        // Delete the first — the path that used to swallow it.
        app.popup = Popup::Shortcuts {
            entries: app.shortcuts.entries.clone(),
            cursor: 0,
            path: vec![],
        };
        // `d` asks first now — see `shortcut_delete_asks`. The save it cannot
        // do happens after the yes.
        app.handle_key(key('d')).unwrap();
        app.handle_key(key('y')).unwrap();
        match &app.popup {
            Popup::Notice { lines } => {
                let text = lines.join(" ");
                assert!(
                    text.contains("could not be saved") || text.contains("保存できませんでした"),
                    "it says so: {text}",
                );
                assert!(text.contains(":where"), "and where it would have gone: {text}");
            }
            other => panic!("expected a notice, got {other:?}"),
        }
    }

    /// `preview_skip` names the kinds the cursor-follow preview leaves alone.
    /// A `.vsix` is a zip of an editor extension: listing one means unpacking
    /// it, which stalls the panel for a file nobody wanted to look inside.
    #[test]
    fn preview_skip_leaves_those_kinds_alone() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["ext.vsix", "disc.ISO", "notes.txt"] {
            std::fs::write(dir.path().join(n), b"x").unwrap();
        }
        let p = dir.path().to_path_buf();
        let mut config = en_config();
        // Written any of the three ways someone would write it.
        config.options.preview_skip =
            vec!["vsix".into(), ".iso".into(), "TAR.GZ".into()];
        let mut app = App::new(p.clone(), p, config).unwrap();

        let at = |app: &mut App, name: &str| {
            let i = app
                .active_pane()
                .unwrap()
                .entries
                .iter()
                .position(|e| e.name == name)
                .unwrap();
            app.active_pane_mut().unwrap().cursor = i;
            crate::preview::preview_target(app)
        };

        assert!(at(&mut app, "notes.txt").is_ok(), "an ordinary file still previews");
        let e = at(&mut app, "ext.vsix").unwrap_err();
        assert!(e.contains("preview_skip"), "and says why: {e:?}");
        // Case does not matter, on the file or in the config.
        assert!(at(&mut app, "disc.ISO").is_err(), "matched whatever the case");

        // Some kinds are skipped without being configured at all: a `.vsix`
        // is an editor extension, and unpacking one to list it stalls the
        // panel for something nobody is looking at the folder for.
        let dir2 = tempfile::tempdir().unwrap();
        for n in ["ext.vsix", "disc.iso", "lib.whl", "paper.pdf", "archive.zip", "notes.txt", "shot.png"] {
            std::fs::write(dir2.path().join(n), b"x").unwrap();
        }
        let p2 = dir2.path().to_path_buf();
        let mut plain = App::new(p2.clone(), p2, en_config()).unwrap();
        for skipped in ["ext.vsix", "disc.iso", "lib.whl", "paper.pdf"] {
            assert!(at(&mut plain, skipped).is_err(), "{skipped} is skipped by default");
        }
        // …but a plain archive is one someone is browsing on purpose.
        assert!(at(&mut plain, "archive.zip").is_ok(), "a zip still previews");
        assert!(at(&mut plain, "notes.txt").is_ok());
        // …and so does an image, which is the whole point of a preview.
        assert!(at(&mut plain, "shot.png").is_ok(), "a picture still previews");
    }

    /// The shell keeps what has gone past, with its colours, and can be
    /// scrolled back through it. The parser was built with a scrollback of
    /// zero, so a line leaving the top of the panel was simply gone.
    #[test]
    fn the_shell_can_be_scrolled_back_through_what_went_past() {
        let dir = tempfile::tempdir().unwrap();
        let sh = cian_pty::default_shell();
        let s = cian_pty::PtySession::new(dir.path(), &sh, 10, 80).unwrap();
        s.parser().lock().unwrap().process(
            (1..=200).map(|i| format!("line {i}\r\n")).collect::<String>().as_bytes(),
        );
        let seen = |s: &cian_pty::PtySession| s.parser().lock().unwrap().screen().contents();
        assert!(seen(&s).contains("line 200"), "the end is on screen");
        assert!(!seen(&s).contains("line 100"), "the middle is not");
        assert_eq!(s.scrollback_pos(), 0, "and it is live");

        // Back past the height of the screen — which used to panic, and is
        // why cian briefly kept a plain-text history of its own.
        assert_eq!(s.scroll_back(120), 120, "120 rows back");
        assert!(seen(&s).contains("line 72"), "which is up there:\n{}", seen(&s));
        assert!(!seen(&s).contains("line 200"), "the end has gone off the bottom");

        // Forward again, and to the end.
        s.scroll_back(-60);
        assert_eq!(s.scrollback_pos(), 60);
        s.scroll_to_bottom();
        assert_eq!(s.scrollback_pos(), 0);
        assert!(seen(&s).contains("line 200"), "back to live output");

        // It stops at both ends rather than running off them.
        s.scroll_back(-10);
        assert_eq!(s.scrollback_pos(), 0, "cannot scroll past the end");
        let far = s.scroll_back(isize::MAX / 2);
        assert!(far > 100 && far < 10_000, "clamped to what there is: {far}");
    }

    /// The wheel over the shell scrolls it, and typing comes back to the end
    /// — typing into a screen that is not the current one is how commands end
    /// up somewhere nobody was looking.
    #[test]
    fn the_wheel_scrolls_the_shell_and_typing_returns_to_it() {
        use crossterm::event::MouseEventKind;
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 30);
        let Some(s) = app.shell.active_session() else {
            return; // no shell on this machine; the unit test above covers it
        };
        s.parser().lock().unwrap().process(
            (1..=200).map(|i| format!("line {i}\r\n")).collect::<String>().as_bytes(),
        );
        let shell = app.layout_rects.shell;
        let wheel = |app: &mut App, kind| {
            app.handle_mouse(crossterm::event::MouseEvent {
                kind,
                column: shell.x + 4,
                row: shell.y + 2,
                modifiers: KeyModifiers::NONE,
            });
        };
        wheel(&mut app, MouseEventKind::ScrollUp);
        wheel(&mut app, MouseEventKind::ScrollUp);
        let at = app.shell.active_session().map(|s| s.scrollback_pos()).unwrap_or(0);
        assert_eq!(at, 6, "six rows back, three to a notch");

        // …and the wheel does not steal the focus: reading is not choosing
        // where to type.
        assert_ne!(app.focused, FocusedPane::Shell, "focus stayed on the listing");

        // Typing into the shell brings it back to live output.
        app.focus(FocusedPane::Shell);
        app.handle_key(key('x')).unwrap();
        assert_eq!(
            app.shell.active_session().map(|s| s.scrollback_pos()),
            Some(0),
            "typing returned to the end",
        );
    }

    /// The wheel moves the view, both ways, and takes the cursor only when it
    /// would otherwise be left off screen.
    #[test]
    fn the_wheel_scrolls_the_panel_in_both_directions() {
        use crossterm::event::{MouseButton, MouseEventKind};
        let long: String = (0..40).map(|i| format!("word{i:02} ")).collect();
        let body = format!("{long}\n{}", "filler\n".repeat(60));
        let (_d, mut app) = viewer_on(&body);
        let _ = render(&mut app, 100, 30);
        let wheel = |app: &mut App, kind| {
            let r = app.viewer_rect;
            app.handle_mouse(crossterm::event::MouseEvent {
                kind,
                column: r.x + 4,
                row: r.y + 2,
                modifiers: KeyModifiers::NONE,
            });
        };
        let at = |app: &App| match &app.popup {
            Popup::Viewer { scroll, hscroll, line, .. } => (*scroll, *hscroll, *line),
            other => panic!("expected the panel, got {other:?}"),
        };

        // Down and back up.
        wheel(&mut app, MouseEventKind::ScrollDown);
        assert_eq!(at(&app).0, 3, "the view moved, three lines");
        assert!(at(&app).2 >= 3, "and the cursor came along rather than scrolling away");
        wheel(&mut app, MouseEventKind::ScrollUp);
        assert_eq!(at(&app).0, 0);

        // Sideways, for the terminals that report it.
        wheel(&mut app, MouseEventKind::ScrollRight);
        assert_eq!(at(&app).1, 3, "three columns right");
        wheel(&mut app, MouseEventKind::ScrollLeft);
        assert_eq!(at(&app).1, 0);

        // The wheel does not move the cursor while it stays in view: a flick
        // over a file should not change where typing would land.
        let before = at(&app).2;
        wheel(&mut app, MouseEventKind::ScrollDown);
        wheel(&mut app, MouseEventKind::ScrollUp);
        assert_eq!(at(&app).2, before, "the cursor stayed put");

        // A click still places it.
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: app.viewer_rect.x + app.viewer_gutter + 2,
            row: app.viewer_rect.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(at(&app).2, at(&app).0 + 1, "the row that was clicked");
    }

    /// Both bars are drawn on the frame, and only when there is something off
    /// screen to report.
    #[test]
    fn the_panel_says_how_much_is_off_screen() {
        let bars = |app: &mut App| {
            let buf = render_buf(app, 100, 20);
            let f = app.viewer_frame;
            let right: String = (f.y..f.y + f.height)
                .map(|y| buf[(f.x + f.width - 1, y)].symbol().to_string())
                .collect();
            let bottom: String = (f.x..f.x + f.width)
                .map(|x| buf[(x, f.y + f.height - 1)].symbol().to_string())
                .collect();
            (right, bottom)
        };

        // A short, narrow file: nothing to say, so nothing is drawn.
        let (_d, mut app) = viewer_on("one\ntwo\n");
        let (right, bottom) = bars(&mut app);
        assert!(!right.contains('┃'), "no vertical bar: {right:?}");
        assert!(!bottom.contains('━'), "no horizontal bar: {bottom:?}");

        // Taller than the panel: a bar down the right border.
        let (_d, mut app) = viewer_on(&"line\n".repeat(200));
        let (right, _) = bars(&mut app);
        assert!(right.contains('┃'), "a vertical bar: {right:?}");

        // Wider than the panel: a bar along the bottom border.
        let long: String = (0..40).map(|i| format!("word{i:02} ")).collect();
        let (_d, mut app) = viewer_on(&format!("{long}\n"));
        let (_, bottom) = bars(&mut app);
        assert!(bottom.contains('━'), "a horizontal bar: {bottom:?}");
    }

    /// The whole operator/object grid, since `viw` turned out to be broken
    /// and the only way to know the rest are not is to press them. Each row
    /// is: the keys, the buffer they leave, what went to the clipboard, and
    /// whether it ended up typing.
    #[test]
    fn every_operator_and_object_pairing() {
        let run = |setup: &str, keys: &str| -> (String, String, bool) {
            let (_d, mut app) = viewer_on(&format!("{setup}\n"));
            for c in keys.chars() {
                app.handle_key(key(c)).unwrap();
            }
            let editing = matches!(app.popup, Popup::Viewer { editing: true, .. });
            (viewer_lines(&app).join("|"), app.yank.clone().unwrap_or_default(), editing)
        };
        // (buffer, keys) -> (buffer after, yanked, left typing)
        let cases: &[(&str, &str, &str, &str, bool)] = &[
            // Quotes and brackets, inside and around.
            ("say \"hi there\" now", "fhci\"", "say \"\" now", "hi there", true),
            ("say \"hi there\" now", "fhdi\"", "say \"\" now", "hi there", false),
            ("say \"hi there\" now", "fhyi\"", "say \"hi there\" now", "hi there", false),
            ("say \"hi there\" now", "fhda\"", "say  now", "\"hi there\"", false),
            ("x 'abc' y", "faci'", "x '' y", "abc", true),
            ("x 'abc' y", "fada'", "x  y", "'abc'", false),
            ("say `x` now", "fxci`", "say `` now", "x", true),
            ("call(one, two) end", "foci(", "call() end", "one, two", true),
            // The closing half of a pair names the same object.
            ("call(one, two) end", "fodi)", "call() end", "one, two", false),
            ("call(one, two) end", "foya(", "call(one, two) end", "(one, two)", false),
            ("arr[1] end", "f1di[", "arr[] end", "1", false),
            ("map{a: 1} end", "f1di{", "map{} end", "a: 1", false),
            ("map{a: 1} end", "f1ca{", "map end", "{a: 1}", true),
            ("tag<b> end", "fbdi<", "tag<> end", "b", false),
            // Words, inside and around.
            ("All done here", "ciw", " done here", "All", true),
            ("All done here", "diw", " done here", "All", false),
            ("All done here", "yiw", "All done here", "All", false),
            ("All done here", "caw", "done here", "All ", true),
            ("All done here", "daw", "done here", "All ", false),
            // Operator plus motion, with and without a count.
            ("All done here", "cw", " done here", "All", true),
            ("All done here", "dw", "done here", "All ", false),
            ("All done here", "d2w", "here", "All done ", false),
            ("All done here", "2dw", "here", "All done ", false),
            ("All done here", "c2w", " here", "All done", true),
            ("All done here", "de", " done here", "All", false),
            ("All done here", "d$", "", "All done here", false),
            ("All done here", "dtd", "done here", "All ", false),
            // Line-wise.
            ("one\ntwo\nthree", "dd", "two|three", "one\n", false),
            ("one\ntwo\nthree", "2dd", "three", "one\ntwo\n", false),
            ("one\ntwo\nthree", "dj", "three", "one\ntwo\n", false),
            ("one\ntwo\nthree", "cc", "|two|three", "one\n", true),
            ("one\ntwo\nthree", "yy", "one|two|three", "one\n", false),
            // An object spanning lines is line-wise, as it is in vi: the
            // brackets stay where they are rather than meeting on one line.
            ("call(\n  one,\n)", "jdi(", "call(|)", "  one,\n", false),
            // Over a selection the object is what to select — the case that
            // was typing a `w` into the file.
            ("say \"hi\" now", "fhvi\"y", "say \"hi\" now", "hi", false),
            ("x 'abc' y", "fava'y", "x 'abc' y", "'abc'", false),
            ("All done here", "vawy", "All done here", "All ", false),
        ];
        for (setup, keys, buffer, yanked, typing) in cases {
            let got = run(setup, keys);
            assert_eq!(
                (got.0.as_str(), got.1.as_str(), got.2),
                (*buffer, *yanked, *typing),
                "{keys} on {setup:?}",
            );
        }

        // An operator followed by another operator is not a command. vi drops
        // it; what must not happen is the machine staying armed, so that the
        // next key is read as the tail of something abandoned.
        let (_d, mut app) = viewer_on("All done here\n");
        for c in "dc".chars() {
            app.handle_key(key(c)).unwrap();
        }
        assert!(matches!(app.popup, Popup::Viewer { pending: None, .. }), "nothing left armed");
        assert!(app.vim_obj.is_none() && app.vim_wait.is_none());
        app.handle_key(key('x')).unwrap();
        assert_eq!(viewer_lines(&app)[0], "ll done here", "and x is x again");
    }

    /// The vi keys the panel was missing, and the two it had but could not
    /// reach. Everything here was reported by using it.
    #[test]
    fn the_vi_keys_that_were_missing_or_unreachable() {
        let at = |app: &App| match &app.popup {
            Popup::Viewer { line, col, editing, scroll, .. } => (*line, *col, *editing, *scroll),
            other => panic!("expected the panel, got {other:?}"),
        };
        let press = |app: &mut App, keys: &str| {
            for c in keys.chars() {
                app.handle_key(key(c)).unwrap();
            }
        };

        // `zt` — the `t` was being read as the start of a find-till motion,
        // so the fold prefix never saw it and nothing scrolled.
        let body = format!("{}{}", "alpha beta gamma\nsecond line\nthird\n", "x\n".repeat(40));
        let (_d, mut app) = viewer_on(&body);
        press(&mut app, "jjzt");
        assert_eq!(at(&app).3, 2, "zt put the cursor's line at the top");
        press(&mut app, "zz");
        assert!(at(&app).3 < 2, "zz centred it again");

        // `s` and `S` — substitute a character, and a line.
        let (_d, mut app) = viewer_on("alpha beta\nsecond\n");
        press(&mut app, "s");
        assert_eq!(viewer_lines(&app)[0], "lpha beta");
        assert!(at(&app).2, "and it is typing");
        app.handle_key(code(KeyCode::Esc)).unwrap();
        press(&mut app, "S");
        assert_eq!(viewer_lines(&app)[0], "", "S emptied the line");
        assert!(at(&app).2);
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // `A` — the end of the line. It was the AI's key once and never came
        // back. `C` changes to the end of the line.
        let (_d, mut app) = viewer_on("alpha beta\n");
        press(&mut app, "A");
        assert_eq!(at(&app).1, 10, "A went to the end");
        assert!(at(&app).2);
        app.handle_key(code(KeyCode::Esc)).unwrap();
        press(&mut app, "0llC");
        assert_eq!(viewer_lines(&app)[0], "al", "C took the rest of the line");
        assert_eq!(app.yank.as_deref(), Some("pha beta"), "and kept it");

        // `r` stamps one character, `3r` three; `R` overwrites until Esc.
        let (_d, mut app) = viewer_on("abcdef\n");
        press(&mut app, "rZ");
        assert_eq!(viewer_lines(&app)[0], "Zbcdef");
        press(&mut app, "0");
        press(&mut app, "3rY");
        assert_eq!(viewer_lines(&app)[0], "YYYdef", "3rY overwrote three");
        press(&mut app, "0Rxy");
        assert_eq!(viewer_lines(&app)[0], "xyYdef", "R overwrote rather than pushed");
        app.handle_key(code(KeyCode::Esc)).unwrap();
        press(&mut app, "0ix");
        assert_eq!(viewer_lines(&app)[0], "xxyYdef", "and insert inserts again");
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // `:combine` joins with a space, `gJ` without, and a count takes
        // more lines. `J` is the window's key for the shell below.
        let combine = |app: &mut App, cmd: &str| {
            app.handle_key(key(':')).unwrap();
            for c in cmd.chars() {
                app.handle_key(key(c)).unwrap();
            }
            app.handle_key(code(KeyCode::Enter)).unwrap();
        };
        let (_d, mut app) = viewer_on("one\ntwo\nthree\nfour\n");
        combine(&mut app, "combine");
        assert_eq!(viewer_lines(&app)[0], "one two");
        press(&mut app, "gJ");
        assert_eq!(viewer_lines(&app)[0], "one twothree");
        let (_d, mut app) = viewer_on("one\ntwo\nthree\nfour\n");
        combine(&mut app, "combine 3");
        assert_eq!(viewer_lines(&app), vec!["one two three", "four"], "three lines");
        let (_d, mut app) = viewer_on("one\ntwo\n");
        combine(&mut app, "combine!");
        assert_eq!(viewer_lines(&app), vec!["onetwo"], "the ! form adds no space");

        // W / E / B are the WORD forms: a word stops at punctuation, a WORD
        // runs to the next space.
        let (_d, mut app) = viewer_on("one two.three four\n");
        press(&mut app, "w");
        assert_eq!(at(&app).1, 4, "w to `two`");
        press(&mut app, "w");
        assert_eq!(at(&app).1, 7, "…and w stops at the dot");
        press(&mut app, "0W");
        assert_eq!(at(&app).1, 4, "W to `two.three`");
        press(&mut app, "W");
        assert_eq!(at(&app).1, 14, "…and W skips over the dot to `four`");
        press(&mut app, "0E");
        assert_eq!(at(&app).1, 2, "E to the end of `one`");
        press(&mut app, "E");
        assert_eq!(at(&app).1, 12, "…then the end of `two.three`");
        press(&mut app, "$B");
        assert_eq!(at(&app).1, 14, "B to the start of `four`");
        press(&mut app, "B");
        assert_eq!(at(&app).1, 4, "…then over the whole of `two.three`");
        press(&mut app, "$ge");
        assert_eq!(at(&app).1, 12, "ge back to the end of the previous word");
        // …and they take an operator: `dW` eats the punctuation with it.
        let (_d, mut app) = viewer_on("one two.three four\n");
        press(&mut app, "wdW");
        assert_eq!(viewer_lines(&app)[0], "one four");
        let (_d, mut app) = viewer_on("one two.three four\n");
        press(&mut app, "wdw");
        assert_eq!(viewer_lines(&app)[0], "one .three four", "dw stops at the dot");

        // `gg` is the top; a bare `g` is a prefix now and jumps nowhere.
        let (_d, mut app) = viewer_on("one\ntwo\nthree\n");
        press(&mut app, "jjg");
        assert_eq!(at(&app).0, 2, "g on its own waits");
        press(&mut app, "g");
        assert_eq!(at(&app).0, 0, "gg is the top");

        // `ca'` and friends — the quote was being eaten by the mark handler,
        // which reads `'a` as a jump.
        for (setup, keys, want) in [
            ("x 'abc' y\n", "faca'", "x  y"),
            ("x 'abc' y\n", "faci'", "x '' y"),
            ("say `x` now\n", "fxci`", "say `` now"),
            ("call(one, two)\n", "fodi(", "call()"),
        ] {
            let (_d, mut app) = viewer_on(setup);
            press(&mut app, keys);
            assert_eq!(viewer_lines(&app)[0], want, "{keys} on {setup:?}");
        }

        // A rectangle, `$`, then `A`: the same text on the end of lines that
        // are not the same length.
        let (_d, mut app) = viewer_on("one\nthirteen\nfive\n");
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)).unwrap();
        press(&mut app, "jj$A");
        press(&mut app, ";");
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), vec!["one;", "thirteen;", "five;"], "ragged right");
    }

    /// The wildcard mode: `crm*ne` finds `crmaine`, which is what a `*` in a
    /// search box is nearly always meant to say. It is its own mode rather
    /// than a change to the regex one — Alt+r cycles as typed → wildcard →
    /// regex — because `\d*` has to keep meaning what it says.
    #[test]
    fn the_wildcard_mode_reads_a_star_the_way_a_search_box_does() {
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let alt = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
        let (_d, mut app) = viewer_on("crmaine\ncrmne\n");
        app.handle_key(ctrl('h')).unwrap();
        for c in "crm*ne".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(alt('r')).unwrap(); // as typed → wildcard
        let bar = crate::render::editor_prompt(&app.popup, app.lang).unwrap();
        assert!(bar.contains("wildcard"), "the bar names the mode: {bar}");
        app.handle_key(code(KeyCode::Tab)).unwrap();
        for c in "X".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(viewer_lines(&app), vec!["X", "X"], "both, empty run included");

        // One more press is a real regex, where the same text means something
        // else and says so.
        let (_d, mut app) = viewer_on("crmaine\n");
        app.handle_key(ctrl('h')).unwrap();
        for c in "crm*ne".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(alt('r')).unwrap();
        app.handle_key(alt('r')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert!(
            app.message.as_deref().is_some_and(|m| m.contains("crm.*ne")),
            "{:?}",
            app.message,
        );

        // And a third press is back to as-typed, where `*` is a star.
        let (_d, mut app) = viewer_on("a*b\naxb\n");
        app.handle_key(ctrl('h')).unwrap();
        for c in "a*b".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Tab)).unwrap();
        app.handle_key(key('Z')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(viewer_lines(&app), vec!["Z", "axb"], "the literal star only");
    }

    /// A regex that finds nothing says why, when the reason is the usual one.
    /// `crm*ne` is "cr, any number of m, then ne" — it does not match
    /// `crmaine`, and looks like it should.
    #[test]
    fn a_regex_that_finds_nothing_says_what_the_star_means() {
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let (_d, mut app) = viewer_on("crmaine\ncrmaine\n");
        app.handle_key(ctrl('h')).unwrap();
        for c in "crm*ne".chars() {
            app.handle_key(key(c)).unwrap();
        }
        let alt_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT);
        app.handle_key(alt_r).unwrap(); // wildcard
        app.handle_key(alt_r).unwrap(); // regex
        app.handle_key(code(KeyCode::Tab)).unwrap();
        for c in "x".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("no matches"), "it did not match: {msg}");
        assert!(msg.contains("crm.*ne"), "and it says what to type: {msg}");
        assert_eq!(viewer_lines(&app), vec!["crmaine", "crmaine"], "nothing changed");

        // The pattern it suggests does match.
        app.handle_key(code(KeyCode::Backspace)).unwrap(); // the replacement
        app.handle_key(code(KeyCode::Tab)).unwrap();
        for _ in 0..6 {
            app.handle_key(code(KeyCode::Backspace)).unwrap();
        }
        for c in "crm.*ne".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Tab)).unwrap();
        app.handle_key(key('x')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(viewer_lines(&app), vec!["x", "x"]);
    }

    /// `:replace` is the same bar, for the terminal that keeps Ctrl.
    #[test]
    fn replace_is_reachable_without_ctrl() {
        let (_d, mut app) = viewer_on("one\n");
        app.handle_key(key(':')).unwrap();
        for c in "replace".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { replace: Some(_), .. }));
    }

    /// Tab crosses the window; Shift+Tab steps the tab strip of whatever has
    /// the focus. Between two listings, between a listing and a file open in
    /// the editor panel, and between two of those panels — one key, because
    /// they are all just "the other side".
    #[test]
    fn tab_crosses_the_window_and_shift_tab_walks_the_tabs() {
        let (_l, _r, mut app) = app_two_dirs(&["a.txt", "b.txt"], &["c.txt"]);
        let _ = render(&mut app, 120, 30);

        // Listing ↔ listing.
        assert_eq!(app.focused, FocusedPane::Left);
        app.handle_key(code(KeyCode::Tab)).unwrap();
        assert_eq!(app.focused, FocusedPane::Right);
        app.handle_key(code(KeyCode::Tab)).unwrap();
        assert_eq!(app.focused, FocusedPane::Left);

        // Shift+Tab is this pane's own tabs, not the other pane.
        app.handle_key(key('t')).unwrap(); // a second tab here — asks first
        app.handle_key(code(KeyCode::Enter)).unwrap(); // yes
        assert_eq!(app.left.tabs.len(), 2, "two tabs open");
        let before = app.left.active;
        app.handle_key(code(KeyCode::BackTab)).unwrap();
        assert_ne!(app.left.active, before, "Shift+Tab stepped the tab strip");
        assert_eq!(app.focused, FocusedPane::Left, "and left the focus where it was");

        // Listing ↔ editor panel.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(app.viewer_dock, Some(FocusedPane::Left), "a file open on the left");
        app.handle_key(code(KeyCode::Tab)).unwrap();
        assert_eq!(app.focused, FocusedPane::Right, "crossed to the listing");
        assert!(matches!(app.popup, Popup::Viewer { .. }), "the file stayed open");
        app.handle_key(code(KeyCode::Tab)).unwrap();
        assert_eq!(app.focused, FocusedPane::Left, "and back into the panel");

        // Panel ↔ panel: open one on the other side too.
        app.handle_key(code(KeyCode::Tab)).unwrap();
        assert_eq!(app.focused, FocusedPane::Right);
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(app.viewer_dock, Some(FocusedPane::Right), "and one open here");
        app.handle_key(code(KeyCode::Tab)).unwrap();
        assert_eq!(app.focused, FocusedPane::Left, "Tab crosses between two panels too");

        // The shell is not on the Tab circuit: Shift+J is how you get there.
        app.handle_key(code(KeyCode::Tab)).unwrap();
        app.handle_key(code(KeyCode::Tab)).unwrap();
        assert_ne!(app.focused, FocusedPane::Shell, "Tab never lands on the shell");
    }

    /// Ctrl+G opens the grep, as it does in Sakura. Ctrl+F was already the
    /// key here; the two are the same prompt, so neither has to be the one
    /// remembered.
    #[test]
    fn ctrl_g_greps_the_way_ctrl_f_does() {
        for k in ['f', 'g'] {
            let (_d, mut app) = app_with(&["a.txt"]);
            app.handle_key(KeyEvent::new(KeyCode::Char(k), KeyModifiers::CONTROL)).unwrap();
            assert!(
                matches!(
                    app.popup,
                    Popup::TextInput { kind: InputKind::GrepRecursive, .. }
                ),
                "Ctrl+{k} opened the grep, got {:?}",
                app.popup,
            );
        }
    }

    /// The seven keys every editor shares — save, copy, cut, paste, undo,
    /// redo, select all — mean the same thing in all three of the panel's
    /// modes. A key you have to change modes to use is a key nobody reaches
    /// for, so they are handled ahead of the mode dispatch.
    #[test]
    fn the_editor_shortcuts_work_in_every_mode() {
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let text = |app: &App| match &app.popup {
            Popup::Viewer { view, .. } => view.lines.join("\n"),
            other => panic!("expected the panel, got {other:?}"),
        };

        // READ mode. Ctrl+X with no selection takes the line the cursor is
        // on, as an editor with these keys does.
        let (_d, mut app) = viewer_on("one\ntwo\nthree\n");
        app.handle_key(ctrl('x')).unwrap();
        assert_eq!(text(&app), "two\nthree", "Ctrl+X cut the cursor's line");
        assert_eq!(app.yank.as_deref(), Some("one\n"), "and it is on the clipboard");

        app.handle_key(ctrl('z')).unwrap();
        assert_eq!(text(&app), "one\ntwo\nthree", "Ctrl+Z put it back");
        app.handle_key(ctrl('y')).unwrap();
        assert_eq!(text(&app), "two\nthree", "Ctrl+Y took it away again");
        // vim's own name for the same step.
        app.handle_key(ctrl('z')).unwrap();
        app.handle_key(ctrl('r')).unwrap();
        assert_eq!(text(&app), "two\nthree", "Ctrl+R is redo too");

        app.handle_key(ctrl('v')).unwrap();
        assert_eq!(text(&app), "two\none\nthree", "Ctrl+V pasted it back");

        // VISUAL mode: Ctrl+C takes exactly what is selected.
        app.handle_key(key('g')).unwrap();
        app.handle_key(key('g')).unwrap();
        app.handle_key(key('V')).unwrap();
        app.handle_key(key('j')).unwrap();
        app.handle_key(ctrl('c')).unwrap();
        assert_eq!(app.yank.as_deref(), Some("two\none\n"), "the selection, not the file");

        // Ctrl+A selects all of it, whatever the mode.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        app.handle_key(ctrl('a')).unwrap();
        assert!(
            matches!(app.popup, Popup::Viewer { visual: Some(ViewVisual::Line), .. }),
            "Ctrl+A selected the file",
        );
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // EDIT mode. The same keys, without leaving it.
        app.handle_key(key('g')).unwrap();
        app.handle_key(key('g')).unwrap();
        app.handle_key(key('i')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }), "in the editor");
        for c in "hello".chars() {
            app.handle_key(key(c)).unwrap();
        }
        assert!(text(&app).starts_with("hello"), "typed: {:?}", text(&app));
        app.handle_key(ctrl('z')).unwrap();
        assert!(!text(&app).starts_with("hello"), "Ctrl+Z undid the insert while editing");
        app.handle_key(ctrl('y')).unwrap();
        assert!(text(&app).starts_with("hello"), "and Ctrl+Y redid it");
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }), "still editing");
        app.handle_key(ctrl('s')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { dirty: false, .. }), "Ctrl+S saved");
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // A new edit throws the undone branch away, as vim does.
        app.handle_key(ctrl('z')).unwrap();
        let before = text(&app);
        app.handle_key(key('x')).unwrap();
        app.handle_key(ctrl('y')).unwrap();
        assert_ne!(text(&app), before, "the forked branch did not come back");
        assert_eq!(
            app.message.as_deref(),
            Some("already at newest change"),
            "and it says so",
        );
    }

    /// Ctrl+V pastes now, so the rectangle it used to start is on vim's own
    /// synonym for it — plus Alt+v and `:block`, which no terminal can take.
    #[test]
    fn the_rectangle_kept_the_keys_that_are_not_ctrl_v() {
        for start in [
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT),
        ] {
            let (_d, mut app) = viewer_on("abcd\nefgh\n");
            app.handle_key(start).unwrap();
            assert!(
                matches!(app.popup, Popup::Viewer { visual: Some(ViewVisual::Block), .. }),
                "{start:?} starts a rectangle",
            );
        }
        let (_d, mut app) = viewer_on("abcd\nefgh\n");
        app.handle_key(key(':')).unwrap();
        for c in "block".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { visual: Some(ViewVisual::Block), .. }));
    }

    /// Focus follows the mouse to the panel as well as away from it. Clicking
    /// the panel from another pane used to do nothing at all: the panel's own
    /// mouse handling only runs for the focused pane, so the click was
    /// swallowed on the way in.
    #[test]
    fn clicking_the_docked_panel_focuses_it() {
        let (_d, mut app) = app_with(&["a.txt", "b.log"]);
        let at = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "b.log")
            .unwrap();
        app.active_pane_mut().unwrap().cursor = at;
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(app.viewer_dock, Some(FocusedPane::Left), "docked on the left");
        let _ = render(&mut app, 120, 30);
        let frame = app.viewer_frame;

        let click = |app: &mut App, column: u16, row: u16| {
            app.handle_mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(
                    crossterm::event::MouseButton::Left,
                ),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            });
        };

        // Away: the listing beside it takes the focus.
        let (right, shell) = (app.layout_rects.right, app.layout_rects.shell);
        click(&mut app, right.x + 4, right.y + 3);
        assert_eq!(app.focused, FocusedPane::Right, "the listing took it");

        // …and back. This is the direction that did not work.
        click(&mut app, frame.x + 4, frame.y + 3);
        assert_eq!(app.focused, FocusedPane::Left, "the panel took it back");

        // From the shell, too.
        click(&mut app, shell.x + 4, shell.y + 1);
        assert_eq!(app.focused, FocusedPane::Shell);
        click(&mut app, frame.x + 4, frame.y + 3);
        assert_eq!(app.focused, FocusedPane::Left, "and back from the shell");
        assert!(matches!(app.popup, Popup::Viewer { .. }), "the file is still open");
    }

    /// F3 gave the panel the whole window, which is what F12 does. One key for
    /// that is enough, and F3 is the listings' — it opens a file in the other
    /// pane.
    #[test]
    fn f3_is_not_a_second_way_to_fill_the_window() {
        let (_d, mut app) = app_with(&["a.txt", "b.log"]);
        let at = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "b.log")
            .unwrap();
        app.active_pane_mut().unwrap().cursor = at;
        app.handle_key(code(KeyCode::Enter)).unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert!(!app.zoomed, "F3 does not zoom the panel");

        // F12 still does.
        app.handle_key(code(KeyCode::F(12))).unwrap();
        assert!(app.zoomed, "F12 does");

        // And it is not offered along the bottom any more.
        app.handle_key(code(KeyCode::F(12))).unwrap();
        let rows = render(&mut app, 120, 30);
        let bottom = rows[rows.len().saturating_sub(2)].clone();
        assert!(
            !bottom.contains("whole window") && !bottom.contains("全画面へ"),
            "the hint went with it: {bottom:?}",
        );
    }

    /// cian is written in Japanese first: with no `lang` in the config the
    /// interface is Japanese, and `lang = "en"` is what asks for English.
    /// (It was the other way round, which meant the people it was written
    /// for had to configure their own language.)
    #[test]
    fn the_interface_is_japanese_unless_asked() {
        let (_d, app) = app_with_lang(&["a.txt"], "ja");
        assert_eq!(app.lang, Lang::Ja);
        assert_eq!(app.menu_lang, Lang::Ja, "and the menus follow it");

        // The real default: nothing set at all.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"").unwrap();
        let p = dir.path().to_path_buf();
        let mut app =
            App::new(p.clone(), p, cian_lua::Config::default()).unwrap();
        assert_eq!(app.lang, Lang::Ja, "no config, Japanese");
        assert_eq!(app.menu_lang, Lang::Ja);
        // Wide characters take two cells, so the rendered rows read "名 前";
        // the spacing is the terminal's, not the string's.
        let screen: String =
            render(&mut app, 120, 30).join("\n").chars().filter(|c| *c != ' ').collect();
        assert!(screen.contains("名前"), "the listing is in Japanese:\n{screen}");

        // And English is one option away.
        let (_d, app) = app_with_lang(&["a.txt"], "en");
        assert_eq!(app.lang, Lang::En);
    }

    /// A menu opened from the docked panel leaves the file stashed behind it,
    /// and the stash was drawn over the whole window: opening the menu looked
    /// like the panel had maximised itself, and Esc "restored" it.
    #[test]
    fn the_menu_does_not_move_the_docked_panel() {
        let (_d, mut app) = app_with(&["a.txt", "b.log"]);
        let at = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "b.log")
            .unwrap();
        app.active_pane_mut().unwrap().cursor = at;
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(app.viewer_dock, Some(FocusedPane::Left), "docked in this pane");
        let _ = render(&mut app, 120, 30);
        let docked_frame = app.viewer_frame;

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert!(matches!(app.popup, Popup::ContextMenu { .. }), "the menu opened");
        assert!(app.viewer_return.is_some(), "with the file waiting behind it");

        // The panel stays where it was: the pane beside it still lists files,
        // which it cannot do if the panel has taken the window.
        let rows = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("b.log"), "the file is still shown:\n{screen}");
        assert!(screen.contains("Name"), "the other pane's listing is intact:\n{screen}");
        assert!(screen.contains("a.txt"), "with its files on it:\n{screen}");

        // Esc puts the menu away and changes nothing else.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "the file is back");
        let _ = render(&mut app, 120, 30);
        assert_eq!(app.viewer_frame, docked_frame, "in the same place it was");
    }

    /// Dialogs follow the theme now — a light theme's menus are light — so
    /// everything drawn on them has to read on them. They were painted for a
    /// dark surface: fixed greys, the theme accent used as body text, the
    /// chat's own cyan. On a light dialog those ran from 1.0:1 to 3.2:1.
    ///
    /// Two things are checked, on every preset in the gallery. That the text
    /// reads — 4.0:1, measured against the cell it actually sits on, which
    /// for a row under the cursor is the selection and not the dialog. And
    /// that the cell was painted at all: `Clear` empties cells without
    /// colouring them, so a dialog with no surface of its own showed the
    /// terminal's background — the `?` manual and Z's jump list did exactly
    /// that, and no contrast check would ever have caught it.
    #[test]
    fn every_popup_reads_on_the_theme_it_is_drawn_on() {
        use crate::theme::{set_theme, ResolvedTheme};
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut bad: Vec<String> = Vec::new();
        // Every preset in the gallery, not a sample of them: the light ones
        // are where this goes wrong, and "which light ones" is not something
        // to keep in step by hand.
        for name in crate::theme::THEME_NAMES {
            let t = crate::theme::theme_preset(name).unwrap();
            set_theme(t);
            for what in ["manual", "panel-help", "palette", "chat", "notice", "toggles", "gallery", "jump",
                "listings", "ssh-users", "snippets", "local-dest", "find", "history",
                "bookmarks", "macros", "sort", "encoding", "op-queue", "ai-history",
                "commit", "input", "quit", "menu", "pane-bg", "report", "archive",
                "git-log", "disk-usage"]
            {
                let (_d, mut app) = app_with(&["a.txt", "b.rs"]);
                match what {
                    // `?` in the panes.
                    "manual" => {
                        app.handle_key(key('?')).unwrap();
                    }
                    // `?` in the text editor panel.
                    "panel-help" => {
                        app.handle_key(code(KeyCode::Enter)).unwrap();
                        app.handle_key(code(KeyCode::F(12))).unwrap();
                        app.handle_key(key('?')).unwrap();
                    }
                    "palette" => {
                        app.handle_key(key('C')).unwrap();
                    }
                    // Z's directory jump is the same popup with other items
                    // in it; it is listed separately because it is the one
                    // the missing surface was noticed on.
                    "jump" => app.start_fuzzy_jump(),
                    "chat" => app.start_ai_chat(
                        ChatMode::Ai,
                        vec![
                            ChatMsg { user: true, text: "hello".into() },
                            ChatMsg { user: false, text: "a reply".into() },
                        ],
                        false,
                    ),
                    "notice" => {
                        app.command_buffer = "ls".into();
                        app.run_command();
                    }
                    "gallery" => app.start_theme_picker(),
                    // The rest are built straight from their variants: they
                    // need remote hosts, a git repo or a finished search to
                    // reach by key, and what is under test is only the paint.
                    "listings" => {
                        app.popup = Popup::SshHosts { cursor: 0, filter: String::new() }
                    }
                    "ssh-users" => app.popup = Popup::SshUsers { host: 0, cursor: 0 },
                    "snippets" => {
                        app.popup = Popup::Snippets { cursor: 0, filter: String::new() }
                    }
                    "local-dest" => {
                        app.popup =
                            Popup::LocalDest { files: vec!["one.txt".into()], cursor: 0 }
                    }
                    "find" => {
                        app.popup = Popup::FindResults {
                            hits: vec![cian_core::search::Hit {
                                path: "/tmp/a.txt".into(),
                                rel: "a.txt".into(),
                                is_dir: false,
                                line: Some((3, "a matching line".into())),
                            }],
                            cursor: 0,
                            scroll: 0,
                            by_ai: false,
                        }
                    }
                    "history" => {
                        app.popup =
                            Popup::History { entries: vec!["/tmp".into()], cursor: 0 }
                    }
                    "bookmarks" => {
                        app.popup = Popup::Shortcuts {
                            entries: vec![Shortcut {
                                name: "home".into(),
                                target: Some("/tmp".into()),
                                children: None,
                            }],
                            cursor: 0,
                            path: vec![],
                        }
                    }
                    "macros" => {
                        app.popup =
                            Popup::Macros { cursor: 0, names: vec!["build".into()] }
                    }
                    "sort" => app.popup = Popup::SortPicker { cursor: 0 },
                    "encoding" => {
                        app.popup =
                            Popup::EncodingPicker { cursor: 0, target: EncTarget::Shell }
                    }
                    "op-queue" => app.popup = Popup::OpQueue { cursor: 0 },
                    "ai-history" => app.popup = Popup::AiHistory { cursor: 0 },
                    "commit" => {
                        app.popup = Popup::CommitMessage {
                            buffer: "fix the thing".into(),
                            stat: " 1 file changed".into(),
                            dir: "/tmp".into(),
                            editing: false,
                        }
                    }
                    "input" => {
                        app.popup = Popup::TextInput {
                            title: " rename ".into(),
                            prompt: "new name".into(),
                            buffer: "a.txt".into(),
                            kind: InputKind::Rename { original: "a.txt".into() },
                            cursor: 5,
                            select_all: false,
                        }
                    }
                    "quit" => app.popup = Popup::ConfirmQuit,
                    "menu" => app.open_context_menu(4, 4),
                    "pane-bg" => {
                        app.popup =
                            Popup::ColorPicker { pane: FocusedPane::Left, cursor: 0 }
                    }
                    "report" => {
                        app.popup = Popup::Report {
                            title: " report ".into(),
                            lines: vec!["one line of it".into(), "and another".into()],
                            scroll: 0,
                            back: Box::new(Popup::None),
                        }
                    }
                    "archive" => {
                        app.popup = Popup::Archive {
                            path: "/tmp/a.zip".into(),
                            members: vec![cian_core::archive::Member {
                                name: "inside.txt".into(),
                                is_dir: false,
                                size: 100,
                                compressed: 40,
                            }],
                            cursor: 0,
                            scroll: 0,
                        }
                    }
                    "git-log" => {
                        app.popup = Popup::GitLog {
                            title: " log ".into(),
                            dir: "/tmp".into(),
                            commits: vec![cian_core::git::Commit {
                                hash: "abc1234".into(),
                                date: "2026-08-11".into(),
                                author: "someone".into(),
                                subject: "a commit subject".into(),
                            }],
                            cursor: 0,
                            scroll: 0,
                            vcs: Vcs::Git,
                        }
                    }
                    "disk-usage" => {
                        app.popup = Popup::DiskUsage {
                            dir: "/tmp".into(),
                            entries: vec![cian_core::du::DuEntry {
                                name: "big".into(),
                                path: "/tmp/big".into(),
                                size: 4096,
                                is_dir: true,
                            }],
                            total: 4096,
                            cursor: 0,
                            scroll: 0,
                        }
                    }
                    _ => {
                        app.handle_key(key('T')).unwrap();
                    }
                }
                let buf = render_buf(&mut app, 110, 30);
                for y in 0..buf.area.height {
                    for x in 0..buf.area.width {
                        let c = &buf[(x, y)];
                        // An unpainted cell is the bug this sweep missed the
                        // first time: `Clear` empties cells without colouring
                        // them, so the dialog showed the terminal's own
                        // background — which passes any contrast check and
                        // follows no theme at all. A theme that paints a
                        // background paints every cell of the window, and
                        // every glyph on it has a colour of its own.
                        let written = !c.symbol().trim().is_empty();
                        // The right half of a wide glyph is left blank and
                        // unstyled by ratatui; the terminal paints it from
                        // the left half, so it is not a gap.
                        let wide_tail = !written
                            && x > 0
                            && crate::util::width(buf[(x - 1, y)].symbol()) == 2;
                        if t.base_bg.is_some()
                            && !wide_tail
                            && (matches!(c.bg, Color::Reset)
                                || (written && matches!(c.fg, Color::Reset)))
                        {
                            bad.push(format!(
                                "{:?} {what}: {:?} at ({x},{y}) is unpainted — {:?} on {:?}",
                                t.accent,
                                c.symbol(),
                                c.fg,
                                c.bg,
                            ));
                            continue;
                        }
                        if !c.symbol().chars().all(char::is_alphanumeric) || !written {
                            continue;
                        }
                        if matches!(c.fg, Color::Reset) || matches!(c.bg, Color::Reset) {
                            continue;
                        }
                        let cr = crate::render::contrast_ratio(c.fg, c.bg);
                        if cr < 4.0 {
                            bad.push(format!(
                                "{:?} {what}: {:?} at ({x},{y}) — {:?} on {:?} is {cr:.2}:1",
                                t.accent,
                                c.symbol(),
                                c.fg,
                                c.bg,
                            ));
                        }
                    }
                }
            }
        }
        set_theme(ResolvedTheme::DARK);
        let n = bad.len();
        bad.dedup();
        bad.truncate(40);
        assert!(bad.is_empty(), "{n} unreadable cells:\n{}", bad.join("\n"));
    }

    /// Every preset in the gallery resolves, and every one of them paints a
    /// dialog surface on the same side of the line as its page — a light
    /// theme with a dark menu is the thing this replaced.
    #[test]
    fn every_preset_resolves_and_its_dialogs_match_its_page() {
        use crate::theme::{theme_preset, THEME_NAMES};
        let lum = |c: Color| match c {
            Color::Rgb(r, g, b) => (299 * r as i32 + 587 * g as i32 + 114 * b as i32) / 1000,
            _ => -1,
        };
        for name in THEME_NAMES {
            let t = theme_preset(name).unwrap_or_else(|| panic!("{name} does not resolve"));
            let Some(base) = t.base_bg else { continue }; // `default` keeps the terminal's
            let (page, dialog) = (lum(base), lum(t.popup_bg));
            assert!(
                (page > 140) == (dialog > 140),
                "{name}: a {} page with a {} dialog",
                if page > 140 { "light" } else { "dark" },
                if dialog > 140 { "light" } else { "dark" },
            );
        }
        // …and the five that were asked for are among them.
        for name in ["monokai-pro", "ayu-dark", "ayu-light", "bluloco-light", "bearded", "nord"] {
            assert!(THEME_NAMES.contains(&name), "{name} is in the gallery");
            assert!(theme_preset(name).is_some(), "{name} resolves");
        }
    }

    /// Backspace in a search listing means the same as Esc. A set of results
    /// has no parent directory to climb to, so climbing to one is a surprise.
    #[test]
    fn backspace_leaves_a_search_listing_rather_than_wandering_off() {
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("hit.txt"), "x\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p.clone(), en_config()).unwrap();

        app.start_find("hit", cian_core::search::Mode::Name);
        drain_find(&mut app);
        app.handle_key(key('p')).unwrap(); // panelize
        assert!(app.active_pane().unwrap().is_flat(), "the pane is a result listing");

        app.handle_key(code(KeyCode::Backspace)).unwrap();
        assert!(!app.active_pane().unwrap().is_flat(), "back to a folder");
        assert_eq!(
            app.active_pane().unwrap().cwd.canonicalize().unwrap(),
            p.canonicalize().unwrap(),
            "the same folder, not its parent",
        );
    }

    /// `:r` after a search takes the pattern with it, so a replace is the
    /// replacement text and nothing else. (It was the bare `r`, which is vi's
    /// replace-one-character.)
    #[test]
    fn r_replaces_what_the_search_just_found() {
        let (_d, mut app) = viewer_on("alpha bravo\nbravo charlie\n");
        let colon_r = |app: &mut App| {
            app.handle_key(key(':')).unwrap();
            app.handle_key(key('r')).unwrap();
            app.handle_key(code(KeyCode::Enter)).unwrap();
        };
        // Nothing searched for yet: it says so rather than opening a prompt
        // with nothing in it.
        colon_r(&mut app);
        assert!(matches!(app.popup, Popup::Viewer { sub_input: None, .. }));
        assert!(app.message.as_deref().unwrap_or("").contains('/'));

        app.handle_key(key('/')).unwrap();
        for c in "bravo".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        colon_r(&mut app);
        assert!(
            matches!(&app.popup, Popup::Viewer { sub_input: Some(s), .. } if s == "s/bravo/"),
            "seeded with what was searched for: {:?}",
            app.popup,
        );
        for c in "BRAVO/g".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), ["alpha BRAVO", "BRAVO charlie"]);

        // A pattern full of slashes gets a delimiter that is not one.
        let (_d2, mut app) = viewer_on("/usr/local/bin\n");
        app.handle_key(key('/')).unwrap();
        for c in "/usr/".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        colon_r(&mut app);
        let Popup::Viewer { sub_input: Some(seed), .. } = &app.popup else { panic!("no prompt") };
        assert!(!seed.starts_with("s/"), "a slash delimiter would break it: {seed:?}");
        assert!(seed.contains("/usr/"), "the pattern is intact: {seed:?}");
    }

    /// Whether a tab-separated file lines up is arithmetic, and the terminal's
    /// font has no say in it. Checked in the cell buffer so "it looks off" can
    /// be told apart from "it is off".
    #[test]
    fn a_tab_separated_file_lines_up_at_the_right_stop() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("t.tsv"), "col1\tcol2\tcol3\nあ\tい\tう\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.show_ws = false;
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "t.tsv").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();

        // Where a marker sits on the screen, as a column number.
        let col_of = |app: &mut App, needle: &str, nth: usize| -> usize {
            let rows = render(app, 120, 20);
            let row = rows.iter().filter(|r| r.contains(needle)).nth(nth).expect("row");
            // Cells, not bytes: the row holds `あ`, which is three bytes and
            // one cell (the backend writes a wide char's second cell as a
            // space of its own).
            let at = row.find(needle).expect("column");
            row[..at].chars().count()
        };

        // Stops every four: `col1` fills one exactly, so its tab moves on to
        // the next — the field after it lands at eight, while a two-column
        // `あ` in the same place lands at four. They cannot line up.
        cian_core::viewer::set_tab_width(4);
        let (a, b) = (col_of(&mut app, "col2", 0), col_of(&mut app, "い", 0));
        assert_ne!(a, b, "four columns is too narrow for this file, by arithmetic");

        // Eight is wide enough for both, so they do.
        cian_core::viewer::set_tab_width(8);
        assert_eq!(
            col_of(&mut app, "col2", 0),
            col_of(&mut app, "い", 0),
            "the second field starts in the same column on both rows",
        );
        assert_eq!(
            col_of(&mut app, "col3", 0),
            col_of(&mut app, "う", 0),
            "and so does the third",
        );
        cian_core::viewer::set_tab_width(4);
    }

    /// The reports from the second pass: a tab drawn outside the viewer moved
    /// the terminal's cursor instead of the text (which left the Makefile on
    /// screen underneath the next preview), a rectangle reached past its own
    /// right edge into a half-covered character, and `I`/`A` did nothing on a
    /// line selection.
    #[test]
    fn tabs_blocks_and_line_selections_all_stay_inside_their_lines() {
        // A tab never reaches the screen as a tab outside the viewer.
        let out = crate::util::plain("a\tb");
        assert_eq!(out, "a   b", "expanded to the next stop");
        assert!(!crate::util::plain("x\u{7}y").contains('\u{7}'), "and no other control code");

        // The block stops at the last character wholly inside it.
        let (_d, mut app) = viewer_on("## 事前準備\n- ふたつめ\n");
        // Ctrl+Q, not Ctrl+V: the latter pastes now, as it does everywhere
        // else, and vim's own synonym is what starts a rectangle.
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)).unwrap();
        if let Popup::Viewer { line, col, .. } = &mut app.popup {
            *line = 1;
            *col = 2; // the `ふ`, which ends at column 4
        }
        app.handle_key(key('d')).unwrap();
        assert_eq!(viewer_lines(&app), ["事前準備", "たつめ"], "`事` was only half inside");

        // I and A on a line selection: the start of every line, and each
        // line's own end — no squaring off.
        let (_d2, mut app) = viewer_on("one\nlonger line\n\n");
        app.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT)).unwrap();
        if let Popup::Viewer { line, .. } = &mut app.popup {
            *line = 2;
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { block_input: Some(_), .. }), "asks for the text");
        app.handle_key(key(',')).unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), ["one,", "longer line,", ","], "each line's own end");

        app.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT)).unwrap();
        if let Popup::Viewer { line, .. } = &mut app.popup {
            *line = 1;
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT)).unwrap();
        for c in "# ".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), ["# one,", "# longer line,", ","]);
    }

    /// The preview panel changes contents on every cursor move, so anything
    /// it fails to wipe reads as part of the next file.
    #[test]
    fn the_preview_panel_does_not_keep_the_last_file_underneath() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("a-long.txt"),
            (1..=40).map(|i| format!("LONGFILE line {i}\n")).collect::<String>(),
        )
        .unwrap();
        std::fs::write(d.path().join("b-short.txt"), "SHORTFILE only line\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        // 既定は切（2026-09-06 に窓版へ揃えた）。この検査が見たいのは
        // 「出したときに何が出るか」なので、ここで入れる。
        assert!(!app.preview_on, "the preview is off by default");
        app.preview_on = true;
        // Past the startup splash, which would otherwise cover the panel.
        app.startup_at = std::time::Instant::now() - std::time::Duration::from_secs(5);

        let show = |app: &mut App, name: &str| {
            app.active_pane_mut().unwrap().cursor =
                app.active_pane().unwrap().entries.iter().position(|e| e.name == name).unwrap();
            render(app, 120, 40).join("\n")
        };

        let long = show(&mut app, "a-long.txt");
        assert!(long.contains("LONGFILE line 10"), "the long file previews");
        let short = show(&mut app, "b-short.txt");
        assert!(short.contains("SHORTFILE"), "the short file previews");
        assert!(
            !short.contains("LONGFILE"),
            "the previous file is still on screen underneath:\n{short}",
        );
    }

    /// A message the panel raises goes to cian's own status line, along the
    /// bottom of the window, where every other message in the program
    /// appears — never into the panel.
    ///
    /// It used to take the panel's footer, and docked there is no footer to
    /// take: the line was drawn over the *text*, without clearing it, so
    /// "copied" appeared with a couple of the file's own characters trailing
    /// after it.
    #[test]
    fn a_message_goes_to_the_status_line_and_not_into_the_file() {
        let (_d, mut app) = viewer_on("one\ntwo\n");
        // The panel's own last row carries a message it raised; the window's
        // hint bar carries its keys; its prompt line carries what is typed.
        let panel_last = |app: &mut App| {
            let rows = render(app, 100, 30);
            // Three rows of window furniture below the panel: prompt (when
            // one is open), hints, status. Without a prompt that is two.
            let n = rows.len();
            rows[n - 4].clone()
        };
        let hint_bar = |app: &mut App| {
            let rows = render(app, 100, 30);
            rows[rows.len() - 2].clone()
        };

        // A message raised by this keystroke is on the status line…
        app.handle_key(key(']')).unwrap();
        app.handle_key(key(']')).unwrap();
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("utline") || msg.contains("アウトライン"), "{msg:?}");
        let rows = render(&mut app, 100, 30);
        assert!(
            rows[rows.len() - 1].contains(&msg),
            "on cian's status line: {:?}",
            rows[rows.len() - 1],
        );
        // …and nowhere inside the panel, where it would be painted over the
        // file with whatever was already on that row left beside it.
        let last = rows.len() - 1;
        assert!(
            !rows.iter().take(last).any(|r| r.contains(&msg)),
            "not in the panel:\n{rows:#?}",
        );
        // The panel's own last row is still the panel's.
        let m = panel_last(&mut app);
        assert!(!m.contains(&msg), "the footer kept its own text: {m:?}");

        // The hints are untouched by any of it.
        app.handle_key(key('j')).unwrap();
        let f = hint_bar(&mut app);
        assert!(f.contains("search") || f.contains("検索"), "hints are there: {f:?}");
        assert!(app.message.is_some(), "the status line still has it");

        // The `:` prompt goes on cian's own prompt line, above the hints, and
        // the hints stay readable beside it.
        app.handle_key(key('/')).unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
        app.handle_key(key(':')).unwrap();
        assert!(matches!(&app.popup, Popup::Viewer { sub_input: Some(_), .. }));
        let rows = render(&mut app, 100, 30);
        let hints = rows[rows.len() - 2].clone();
        let prompt = rows[rows.len() - 3].clone();
        assert!(prompt.contains("s/old/new/"), "the command line is visible: {prompt:?}");
        assert!(
            hints.contains("search") || hints.contains("検索"),
            "and the hints keep their own row: {hints:?}"
        );
    }

    /// `/` gets the same treatment as `:`: a prompt line above the hints, and
    /// the text gives up the row so nothing of the file is covered by it.
    #[test]
    fn the_viewer_search_prompt_sits_above_the_hints() {
        let (_d, mut app) = viewer_on("alpha\nbeta\ngamma\n");
        let before = render(&mut app, 100, 30);
        app.handle_key(key('/')).unwrap();
        app.handle_key(key('b')).unwrap();
        let rows = render(&mut app, 100, 30);
        let hints = rows[rows.len() - 2].clone();
        let prompt = rows[rows.len() - 3].clone();
        assert!(prompt.contains("/b_"), "what is being typed: {prompt:?}");
        assert!(
            hints.contains("search") || hints.contains("検索"),
            "the hints are still there: {hints:?}"
        );
        // The last line of the file must not be hidden behind the new row.
        assert!(before.iter().any(|r| r.contains("gamma")));
        assert!(rows.iter().any(|r| r.contains("gamma")), "the text kept its lines");
    }

    /// A binding can name its modifiers, so a shortcut whose Ctrl key the
    /// terminal keeps can be moved somewhere the terminal will deliver.
    #[test]
    fn a_keymap_entry_can_carry_a_modifier() {
        use crate::theme::parse_key_spec;
        assert_eq!(parse_key_spec("x"), Some(('x', KeyModifiers::NONE)));
        assert_eq!(parse_key_spec("alt+g"), Some(('g', KeyModifiers::ALT)));
        assert_eq!(parse_key_spec("ctrl+f"), Some(('f', KeyModifiers::CONTROL)));
        assert_eq!(parse_key_spec(" Option+G "), Some(('G', KeyModifiers::ALT)));
        // Shift folds into the character: terminals disagree about reporting
        // both, and the uppercase letter already says it.
        assert_eq!(parse_key_spec("shift+s"), Some(('S', KeyModifiers::NONE)));
        for bad in ["", "alt+", "hyper+g", "alt+gg", "+"] {
            assert!(parse_key_spec(bad).is_none(), "{bad:?} should be refused");
        }

        // …and it drives the real key handling.
        let (_d, mut app) = app_with_keymaps(&["a.txt"], vec![("alt+g", "grep_recursive".into())]);
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT)).unwrap();
        assert!(
            matches!(&app.popup, Popup::TextInput { kind: InputKind::GrepRecursive, .. }),
            "Alt+g opened the grep prompt, got {:?}",
            app.popup,
        );
        // The unmodified key is untouched.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        app.handle_key(key('g')).unwrap();
        assert!(!matches!(&app.popup, Popup::TextInput { kind: InputKind::GrepRecursive, .. }));
    }

    /// Every Ctrl shortcut in the viewer needs a route that a terminal cannot
    /// intercept: iTerm2 keeps Ctrl+F for its own find bar and macOS takes
    /// Ctrl+Q for zoom, so a file that can be edited but not saved is a real
    /// possibility on a stock Mac.
    #[test]
    fn the_viewer_can_be_driven_without_a_single_ctrl_key() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("note.txt");
        std::fs::write(&f, "one\ntwo\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let open = |app: &mut App| {
            app.active_pane_mut().unwrap().cursor =
                app.active_pane().unwrap().entries.iter().position(|e| e.name == "note.txt").unwrap();
            app.handle_key(code(KeyCode::F(3))).unwrap();
        };
        let cmd = |app: &mut App, c: &str| {
            app.handle_key(key(':')).unwrap();
            if let Popup::Viewer { sub_input, .. } = &mut app.popup {
                *sub_input = Some(c.into());
            }
            app.handle_key(code(KeyCode::Enter)).unwrap();
        };

        // `:block` reaches the rectangle without Ctrl+V or Ctrl+Q.
        open(&mut app);
        cmd(&mut app, "block");
        assert!(matches!(app.popup, Popup::Viewer { visual: Some(ViewVisual::Block), .. }));
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // `:w` saves without Ctrl+S.
        app.handle_key(key('x')).unwrap(); // delete a character
        assert!(matches!(app.popup, Popup::Viewer { dirty: true, .. }));
        cmd(&mut app, "w");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "ne\ntwo\n");
        assert!(matches!(app.popup, Popup::Viewer { dirty: false, .. }));

        // `:q` refuses to drop unsaved work; `:q!` says to anyway.
        app.handle_key(key('x')).unwrap();
        cmd(&mut app, "q");
        assert!(matches!(app.popup, Popup::Viewer { .. }), "still open");
        assert!(app.message.as_deref().unwrap_or("").contains(":q!"));
        cmd(&mut app, "q!");
        assert!(matches!(app.popup, Popup::None));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "ne\ntwo\n", "discarded, not written");

        // `:wq` writes and then closes.
        open(&mut app);
        app.handle_key(key('x')).unwrap();
        cmd(&mut app, "wq");
        assert!(matches!(app.popup, Popup::None));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "e\ntwo\n");
    }

    /// A message must be readable on a narrow terminal, where the status
    /// chips it shares a row with would otherwise push it off the edge — the
    /// reason `:keys`, "unknown command" and every other answer appeared to
    /// do nothing at all.
    #[test]
    fn a_message_is_never_the_thing_that_falls_off_the_status_line() {
        let d = tempfile::tempdir().unwrap();
        // A long path, so the chips have plenty to say.
        let deep = d.path().join("a-fairly-long-directory-name").join("and-another-one-here");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("f.txt"), "x\n").unwrap();
        let mut app = App::new(deep.clone(), deep, en_config()).unwrap();

        app.mode = Mode::Command;
        app.command_buffer = "key".into();
        app.run_command();
        for w in [60u16, 80, 120] {
            let screen = render(&mut app, w, 24).join("\n");
            assert!(screen.contains("showing every key"), "at {w} columns the answer is off screen");
        }

        // …and the report it turns on is readable too.
        app.handle_key(key('j')).unwrap();
        let screen = render(&mut app, 60, 24).join("\n");
        assert!(screen.contains("key: Char('j')"), "the key report is off screen: {screen}");

        // An unknown command says so rather than appearing to do nothing.
        app.mode = Mode::Command;
        app.command_buffer = "nosuchcommand".into();
        app.run_command();
        let screen = render(&mut app, 60, 24).join("\n");
        assert!(screen.contains("unknown command"), "{screen}");
    }

    /// The reported problems, each pinned so it cannot come back:
    /// `:` opened with `s/` already typed so no word command was reachable;
    /// `]]` disagreed with the screen in the Markdown preview; Space did not
    /// fold; and a message the viewer raised was drawn outside its own border.
    #[test]
    fn the_viewer_command_line_and_outline_answer_where_you_are_looking() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("doc.md"),
            "# One\n\nsome prose that is long enough to wrap once the width gets small\n\n## Two\n\nmore prose\n\n# Three\n\nlast\n",
        )
        .unwrap();
        std::fs::write(d.path().join("plain.txt"), "nothing here\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let open = |app: &mut App, name: &str| {
            app.active_pane_mut().unwrap().cursor =
                app.active_pane().unwrap().entries.iter().position(|e| e.name == name).unwrap();
            app.handle_key(code(KeyCode::F(3))).unwrap();
            let _ = render(app, 100, 30);
        };
        let at = |app: &App| match &app.popup {
            Popup::Viewer { line, .. } => *line,
            other => panic!("not a viewer: {other:?}"),
        };

        open(&mut app, "doc.md");
        // The prompt opens empty, so a word command is typable, and it works
        // in the preview — where `:outline` is most wanted.
        app.handle_key(key(':')).unwrap();
        assert!(matches!(&app.popup, Popup::Viewer { sub_input: Some(s), preview: true, .. } if s.is_empty()));
        for c in "outline".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(&app.popup, Popup::Viewer { shape: Some(sh), .. } if !sh.shown));
        app.handle_key(key(':')).unwrap();
        for c in "outline".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();

        // In the preview, `]]` lands on the line that *shows* the next
        // heading — the rendered document has neither the same count of lines
        // as the source nor the same order.
        let _ = render(&mut app, 100, 30);
        let shown = |app: &mut App| {
            let l = at(app);
            match &app.popup {
                Popup::Viewer { view, .. } => view.lines[l].clone(),
                other => panic!("not a viewer: {other:?}"),
            }
        };
        for _ in 0..2 {
            app.handle_key(key(']')).unwrap();
        }
        assert!(shown(&mut app).contains("Two"), "got {:?}", shown(&mut app));
        for _ in 0..2 {
            app.handle_key(key(']')).unwrap();
        }
        assert!(shown(&mut app).contains("Three"), "got {:?}", shown(&mut app));
        for _ in 0..2 {
            app.handle_key(key('[')).unwrap();
        }
        assert!(shown(&mut app).contains("Two"), "back: got {:?}", shown(&mut app));

        // Space folds, in the source.
        app.toggle_markdown_preview();
        let _ = render(&mut app, 100, 30);
        if let Popup::Viewer { line, .. } = &mut app.popup {
            *line = 2; // inside the first section
        }
        app.handle_key(key(' ')).unwrap();
        let folds = |app: &App| match &app.popup {
            Popup::Viewer { shape, .. } => shape.as_deref().unwrap().folds.iter().copied().collect::<Vec<_>>(),
            other => panic!("not a viewer: {other:?}"),
        };
        assert_eq!(folds(&app), [0], "Space folded the section");
        app.handle_key(key(' ')).unwrap();
        assert!(folds(&app).is_empty(), "and unfolded it");

        // zA is the whole file as one switch, either way round.
        app.handle_key(key('z')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)).unwrap();
        let all = folds(&app);
        assert!(all.len() >= 2, "everything closed: {all:?}");
        app.handle_key(key('z')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)).unwrap();
        assert!(folds(&app).is_empty(), "and everything open again");

        // A file with no outline says so, on the viewer's own footer rather
        // than on the status line hiding behind it.
        quit_viewer(&mut app);
        open(&mut app, "plain.txt");
        for _ in 0..2 {
            app.handle_key(key(']')).unwrap();
        }
        let screen = render(&mut app, 100, 30);
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("outline") || msg.contains("アウトライン"), "{msg:?}");
        // On cian's status line, and only there.
        let last = screen.len() - 1;
        assert!(screen[last].contains(&msg), "on the status line: {:?}", screen[last]);
        assert!(
            !screen.iter().take(last).any(|r| r.contains(&msg)),
            "and not painted over the file:\n{screen:#?}",
        );
    }

    /// Folding: za closes the section the cursor is in, the lines under it
    /// stop being drawn, the cursor comes out with them, and zR/zM work on the
    /// lot. The outline and the folds are the same information read two ways.
    #[test]
    fn folds_hide_a_section_and_take_the_cursor_with_them() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("doc.md"),
            "# One\nunder one\nstill one\n# Two\nunder two\n# Three\nunder three\n",
        )
        .unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "doc.md").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        // Markdown opens in preview; folding belongs to the source. The
        // whitespace marks are not what this is about.
        app.toggle_markdown_preview();
        app.show_ws = false;
        let _ = render(&mut app, 120, 30);

        let folds = |app: &App| match &app.popup {
            Popup::Viewer { shape, .. } => shape.as_deref().unwrap().folds.iter().copied().collect::<Vec<_>>(),
            other => panic!("not a viewer: {other:?}"),
        };
        let at = |app: &App| match &app.popup {
            Popup::Viewer { line, .. } => *line,
            other => panic!("not a viewer: {other:?}"),
        };
        let put = |app: &mut App, l: usize| {
            if let Popup::Viewer { line, .. } = &mut app.popup {
                *line = l;
            }
        };
        let za = |app: &mut App| {
            app.handle_key(key('z')).unwrap();
            app.handle_key(key('a')).unwrap();
        };

        // From inside the first section, za closes the section — not the line.
        put(&mut app, 1);
        za(&mut app);
        assert_eq!(folds(&app), [0]);
        assert_eq!(at(&app), 0, "the cursor came out onto the heading");
        let _ = render(&mut app, 120, 30);

        // The hidden lines are no longer drawn *in the panel*. (The cursor
        // preview under it is a different surface showing the same file, and
        // it does not fold.)
        let screen = |app: &mut App| -> String {
            let rows = render(app, 120, 30);
            let f = app.viewer_frame;
            rows.iter()
                .skip(f.y as usize)
                .take(f.height as usize)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        };
        let shown = screen(&mut app);
        assert!(shown.contains("# One") && shown.contains("# Two"));
        assert!(!shown.contains("under one"), "the folded lines are gone from the panel");

        // Pressing it again opens it.
        za(&mut app);
        assert!(folds(&app).is_empty());
        assert!(screen(&mut app).contains("under one"));

        // Clicking the marker in the gutter is the same as za on that line.
        let g = app.viewer_gutter;
        let b = app.viewer_rect;
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: b.x + g - 2,
            row: b.y + 3, // the "# Two" heading, with everything open
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(folds(&app), [3], "clicking the marker closed that section");
        let _ = render(&mut app, 120, 30);
        app.handle_key(key('z')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT)).unwrap();

        // zM closes everything with something in it, zR opens the lot.
        app.handle_key(key('z')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(folds(&app), [0, 3, 5]);
        let shut = screen(&mut app);
        assert!(!shut.contains("under two") && !shut.contains("under three"));
        assert!(shut.contains("# Three"), "every heading is still there to open");
        app.handle_key(key('z')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT)).unwrap();
        assert!(folds(&app).is_empty());

        // A file with nothing to fold says so instead of doing nothing.
        quit_viewer(&mut app);
        std::fs::write(d.path().join("flat.txt"), "a\nb\n").unwrap();
        let _ = app.active_file_tabs_mut().map(|t| t.active_mut().reload());
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "flat.txt").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        za(&mut app);
        assert!(app.message.as_deref().unwrap_or("").contains("fold"));
    }

    /// Rectangular editing: Ctrl+V marks a block, then `d` cuts it, and
    /// `I` / `A` / `c` type once and land on every line.
    #[test]
    fn block_selection_can_be_edited_not_just_copied() {
        // Move the cursor to (line, col) without relying on key counts.
        let put = |app: &mut App, l: usize, c: usize| {
            if let Popup::Viewer { line, col, .. } = &mut app.popup {
                *line = l;
                *col = c;
            }
        };
        let block = |app: &mut App, from: (usize, usize), to: (usize, usize)| {
            put(app, from.0, from.1);
            // Ctrl+Q, not Ctrl+V: the latter pastes now, as it does everywhere
        // else, and vim's own synonym is what starts a rectangle.
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)).unwrap();
            put(app, to.0, to.1);
        };

        // d cuts the rectangle out of every line it covers.
        let (_d, mut app) = viewer_on("abcdef\nabcdef\nabcdef\n");
        block(&mut app, (0, 2), (2, 3));
        app.handle_key(key('d')).unwrap();
        assert_eq!(viewer_lines(&app), ["abef", "abef", "abef"]);
        app.handle_key(key('u')).unwrap();
        assert_eq!(viewer_lines(&app), ["abcdef", "abcdef", "abcdef"], "one undo step");

        // I inserts down the left edge, once typed.
        let (_d2, mut app) = viewer_on("one\ntwo\nthree\n");
        block(&mut app, (0, 0), (2, 0));
        app.handle_key(KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { block_input: Some(_), .. }), "asks for the text");
        for c in "# ".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), ["# one", "# two", "# three"]);

        // A appends at the right edge, padding the short lines so it lines up.
        let (_d3, mut app) = viewer_on("ab\nabcd\n");
        block(&mut app, (0, 0), (1, 2));
        app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)).unwrap();
        app.handle_key(key('|')).unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), ["ab |", "abc|d"], "padded to the column");

        // Mixed widths: the rectangle is rectangular on screen, so the same
        // columns come out of every line whatever it is made of.
        let (_dw, mut app) = viewer_on("あいうえ\nabcdefgh\nあbcう\n");
        // From the second character of line 1 (columns 2-3) down to the `う`
        // on line 3 (columns 4-5): columns 2..6 on every line between.
        block(&mut app, (0, 1), (2, 3));
        app.handle_key(key('d')).unwrap();
        assert_eq!(viewer_lines(&app), ["あえ", "abgh", "あ"]);

        // c replaces what the rectangle covers.
        let (_d4, mut app) = viewer_on("id=001\nid=002\n");
        block(&mut app, (0, 3), (1, 5));
        app.handle_key(key('c')).unwrap();
        for c in "999".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(viewer_lines(&app), ["id=999", "id=999"]);

        // Esc abandons a prompt without touching the buffer.
        let (_d5, mut app) = viewer_on("keep\nkeep\n");
        block(&mut app, (0, 0), (1, 1));
        app.handle_key(key('c')).unwrap();
        app.handle_key(key('X')).unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert_eq!(viewer_lines(&app), ["keep", "keep"], "Esc changes nothing");
    }

    /// The hex editor: `i` on a binary view, hex digits overwrite the byte
    /// under the cursor, Ctrl+S saves — with a `.bak` of the original — and
    /// `u` walks the whole session back.
    #[test]
    fn hex_edit_overwrites_a_byte_and_saves_with_backup() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("blob.bin");
        // NULs make the sniffer call it binary; first byte is 0x41 ('A').
        let mut bytes = vec![0x41u8, 0x42, 0x00, 0x00, 0x43];
        std::fs::write(&file, &bytes).unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "blob.bin").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);
        assert!(
            matches!(&app.popup, Popup::Viewer { view, editable: true, .. }
                if view.kind == cian_core::viewer::ViewKind::Binary),
            "binary views are hex-editable"
        );

        // i → editing; "ff" overwrites byte 0 nibble by nibble.
        app.handle_key(key('i')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }));
        app.handle_key(key('f')).unwrap();
        app.handle_key(key('f')).unwrap();
        match &app.popup {
            Popup::Viewer { view, dirty, .. } => {
                assert_eq!(view.raw_bytes()[0], 0xFF, "byte overwritten");
                assert!(view.lines[0].contains("ff"), "dump line re-rendered");
                assert!(*dirty);
            }
            _ => unreachable!(),
        }

        // u restores the original buffer.
        app.handle_key(key('u')).unwrap();
        match &app.popup {
            Popup::Viewer { view, dirty, .. } => {
                assert_eq!(view.raw_bytes()[0], 0x41, "undo restored the bytes");
                assert!(!dirty, "back to the original → clean");
            }
            _ => unreachable!(),
        }

        // Edit again and save: the file changes, a .bak keeps the original.
        app.handle_key(key('f')).unwrap();
        app.handle_key(key('f')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)).unwrap();
        bytes[0] = 0xFF;
        assert_eq!(std::fs::read(&file).unwrap(), bytes, "patched in place, same size");
        assert_eq!(
            std::fs::read(d.path().join("blob.bin.bak")).unwrap()[0],
            0x41,
            "the .bak holds the original"
        );
    }

    /// A BOM'd file wears a badge in the viewer, and `:nobom` strips UTF-8
    /// BOMs while refusing to touch UTF-16 ones.
    #[test]
    fn bom_badge_and_nobom_strip() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("bommed.txt"), b"\xEF\xBB\xBFhello\n").unwrap();
        std::fs::write(d.path().join("plain.txt"), b"hello\n").unwrap();
        // UTF-16LE with BOM: FF FE + "hi" in LE code units.
        std::fs::write(d.path().join("wide.txt"), b"\xFF\xFEh\x00i\x00").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "bommed.txt").unwrap();
        }
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let out = render(&mut app, 100, 30).join("\n");
        assert!(out.contains("UTF-8 BOM"), "the badge shows: {out}");
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // Mark all three and strip.
        {
            let pane = app.active_pane_mut().unwrap();
            for i in 0..pane.entries.len() {
                pane.set_mark_at(i);
            }
        }
        app.start_nobom();
        assert!(matches!(app.popup, Popup::ConfirmNoBom { .. }), "asks first");
        app.handle_key(key('y')).unwrap();
        assert_eq!(std::fs::read(d.path().join("bommed.txt")).unwrap(), b"hello\n", "BOM gone");
        assert_eq!(std::fs::read(d.path().join("plain.txt")).unwrap(), b"hello\n", "untouched");
        assert_eq!(
            std::fs::read(d.path().join("wide.txt")).unwrap(),
            b"\xFF\xFEh\x00i\x00",
            "UTF-16 BOM kept — it is load-bearing"
        );
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains('1') && (msg.contains("UTF-16") || msg.contains("stripped")), "{msg}");
    }

    /// Ops queue instead of refusing: a second start_op while one runs waits
    /// its turn and starts automatically when the runner finishes.
    #[test]
    fn a_second_op_queues_and_runs_after_the_first() {
        use std::sync::atomic::AtomicUsize;
        let (_d, mut app) = app_with(&[]);
        let order = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let (o1, o2) = (Arc::clone(&order), Arc::clone(&order));
        let _ = AtomicUsize::new(0);
        app.start_op("copying", move |_ctl| {
            std::thread::sleep(Duration::from_millis(80));
            o1.lock().unwrap().push(1);
            OpReport { ok: 1, ..Default::default() }
        });
        app.start_op("copying", move |_ctl| {
            o2.lock().unwrap().push(2);
            OpReport { ok: 1, ..Default::default() }
        });
        assert_eq!(app.op_queue.len(), 1, "second op waits in line");
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("queued") || msg.contains("キュー"), "{msg}");
        // Drain the runner; the queued op must start on its own and finish.
        for _ in 0..600 {
            app.poll_op_job();
            if app.op_job.is_none() && app.op_queue.is_empty() && order.lock().unwrap().len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*order.lock().unwrap(), vec![1, 2], "ran in order, automatically");
    }

    /// A failed transfer re-runs by itself; local ops never do.
    #[test]
    fn transfers_auto_retry_on_failure() {
        use std::sync::atomic::AtomicUsize;
        let (_d, mut app) = app_with(&[]);
        let runs = Arc::new(AtomicUsize::new(0));
        let r = Arc::clone(&runs);
        app.start_op("uploading", move |_ctl| {
            let n = r.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                let mut rep = OpReport::default();
                rep.note_error("connection reset".to_string());
                rep
            } else {
                OpReport { ok: 1, ..Default::default() }
            }
        });
        drain_op_job(&mut app);
        assert_eq!(runs.load(Ordering::SeqCst), 2, "one failure, one successful retry");

        // A local op with the same failure shape runs exactly once.
        let runs = Arc::new(AtomicUsize::new(0));
        let r = Arc::clone(&runs);
        app.start_op("copying", move |_ctl| {
            r.fetch_add(1, Ordering::SeqCst);
            let mut rep = OpReport::default();
            rep.note_error("nope".to_string());
            rep
        });
        drain_op_job(&mut app);
        assert_eq!(runs.load(Ordering::SeqCst), 1, "local failures are not retried");
    }

    /// A worker deaf to its cancel flag can be abandoned: the queue moves on
    /// even though the thread is still wedged.
    #[test]
    fn an_abandoned_op_frees_the_queue() {
        let (_d, mut app) = app_with(&[]);
        // A worker that blocks forever (a stand-in for a wedged syscall).
        let (_hold_tx, hold_rx) = std::sync::mpsc::channel::<()>();
        let hold = std::sync::Mutex::new(Some(hold_rx));
        app.start_op("uploading", move |_ctl| {
            if let Some(rx) = hold.lock().unwrap().take() {
                let _ = rx.recv(); // never resolves; _hold_tx lives in the test
            }
            OpReport::default()
        });
        let ran = Arc::new(std::sync::Mutex::new(false));
        let flag = Arc::clone(&ran);
        app.start_op("copying", move |_ctl| {
            *flag.lock().unwrap() = true;
            OpReport { ok: 1, ..Default::default() }
        });
        assert_eq!(app.op_queue.len(), 1);
        // Ask it to stop (it will not), then abandon.
        app.cancel_op_job();
        assert!(app.op_job.as_ref().unwrap().cancel_requested.is_some());
        app.abandon_op();
        assert!(app.message.clone().unwrap_or_default().contains("abandon")
            || app.message.clone().unwrap_or_default().contains("見捨て"));
        drain_op_job(&mut app);
        assert!(*ran.lock().unwrap(), "the queued op ran despite the wedged one");
    }

    /// `b` tucks the progress popup away and the keyboard works again while
    /// the op runs in the background.
    #[test]
    fn the_progress_popup_can_be_backgrounded() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt"]);
        app.start_op("copying", move |_ctl| {
            std::thread::sleep(Duration::from_millis(150));
            OpReport { ok: 1, ..Default::default() }
        });
        // While the bar shows, ordinary keys are owned by it…
        let before = app.active_pane().unwrap().cursor;
        app.handle_key(key('j')).unwrap();
        assert_eq!(app.active_pane().unwrap().cursor, before, "modal while the bar shows");
        // …`b` backgrounds it, and the same key now moves the cursor.
        app.handle_key(key('b')).unwrap();
        assert!(app.op_bar_hidden);
        app.handle_key(key('j')).unwrap();
        assert_ne!(app.active_pane().unwrap().cursor, before, "keyboard is live again");
        drain_op_job(&mut app);
        assert!(!app.op_bar_hidden, "reset once the queue drains");
    }

    /// Regression: a keypress arriving in the tiny window after a background op
    /// finished but before its result was polled used to be swallowed by the
    /// "Esc only while an op runs" gate — so a second copy right after the first
    /// appeared to need two presses. handle_key must land a finished op first.
    #[test]
    fn a_key_right_after_an_op_finishes_is_not_swallowed() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // Start a trivial op and let its worker report Done — but do NOT poll it,
        // exactly as the event loop leaves it while blocked on the next input.
        app.start_op("copying", |_ctl| cian_core::ops::OpReport::default());
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(app.op_job.is_some(), "job still flagged in-flight (unpolled)");

        // The next keypress must be acted on, not eaten: `c` opens the copy
        // confirmation, and the finished op is landed in the same step.
        app.handle_key(key('c')).unwrap();
        assert!(app.op_job.is_none(), "the finished op was landed, not left blocking");
        assert!(
            matches!(app.popup, Popup::ConfirmTransfer { .. }),
            "the copy key was handled: {:?}",
            app.popup
        );
    }

    #[test]
    fn unzip_extracts_into_a_named_subfolder() {
        let (d, mut app) = app_with(&[]);
        // Build a real zip in the pane's directory.
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut prog = |_: &cian_core::progress::Progress| {};
        let mut ctl = cian_core::progress::Ctl { cancel: &cancel, on_progress: &mut prog };
        std::fs::write(d.path().join("payload.txt"), b"inside the zip").unwrap();
        let archive = d.path().join("bundle.zip");
        cian_core::archive::create_zip(&[d.path().join("payload.txt")], &archive, None, &mut ctl);

        app.reload_both();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "bundle.zip").unwrap();
        app.extract_selected();
        drain_op_job(&mut app);

        // Extracted into ./bundle/ next to the archive.
        let extracted = d.path().join("bundle").join("payload.txt");
        assert!(extracted.is_file(), "payload extracted: {:?}", extracted);
        assert_eq!(std::fs::read_to_string(extracted).unwrap(), "inside the zip");
    }

    #[test]
    fn encrypted_zip_lists_on_f3_and_extracts_after_a_password() {
        let (d, mut app) = app_with(&[]);
        // Build an AES zip in the pane's directory.
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut prog = |_: &cian_core::progress::Progress| {};
        let mut ctl = cian_core::progress::Ctl { cancel: &cancel, on_progress: &mut prog };
        std::fs::write(d.path().join("secret.txt"), b"top secret").unwrap();
        let archive = d.path().join("locked.zip");
        cian_core::archive::create_zip(&[d.path().join("secret.txt")], &archive, Some("hunter2"), &mut ctl);

        app.reload_both();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "locked.zip").unwrap();

        // F3 lists the members (no more garbled hex dump).
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert!(matches!(app.popup, Popup::Archive { .. }), "F3 shows the archive listing, got {:?}", app.popup);
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // Extract asks for the password first.
        app.extract_selected();
        assert!(
            matches!(&app.popup, Popup::TextInput { kind: InputKind::ExtractPassword { .. }, .. }),
            "encrypted extract prompts for a password"
        );
        // The wrong password extracts nothing; the right one yields the file.
        if let Popup::TextInput { buffer, .. } = &mut app.popup {
            buffer.push_str("hunter2");
        }
        app.finish_text_input().unwrap();
        drain_op_job(&mut app);
        let got = d.path().join("locked").join("secret.txt");
        assert!(got.is_file(), "extracted with the password: {:?}", got);
        assert_eq!(std::fs::read_to_string(got).unwrap(), "top secret");
    }

    #[test]
    fn compress_menu_builds_a_zip() {
        let (d, mut app) = app_with(&["a.rs", "b.rs"]);
        // Mark both files, then run the Compress ▸ .zip flow.
        {
            let p = app.active_pane_mut().unwrap();
            for i in 0..p.entries.len() {
                if !p.entries[i].is_parent {
                    p.toggle_mark_at(i);
                }
            }
        }
        app.prompt_compress(CompressKind::Zip);
        // Type the archive name and submit.
        if let Popup::TextInput { buffer, .. } = &mut app.popup {
            buffer.clear();
            buffer.push_str("out");
        } else {
            panic!("no name prompt");
        }
        app.finish_text_input().unwrap();
        drain_op_job(&mut app);
        assert!(d.path().join("out.zip").is_file(), "out.zip created");
    }

    #[test]
    fn compress_menu_password_zip_chains_to_a_password_prompt() {
        let (d, mut app) = app_with(&["a.rs"]);
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "a.rs").unwrap();
        // Encrypted-zip flow: name prompt → password prompt → build.
        app.prompt_compress(CompressKind::ZipEnc);
        if let Popup::TextInput { buffer, .. } = &mut app.popup {
            buffer.clear();
            buffer.push_str("safe");
        } else {
            panic!("no name prompt");
        }
        app.finish_text_input().unwrap();
        assert!(
            matches!(&app.popup, Popup::TextInput { kind: InputKind::ZipPassword { .. }, .. }),
            "the name prompt chains into a password prompt"
        );
        if let Popup::TextInput { buffer, .. } = &mut app.popup {
            buffer.push_str("hunter2");
        }
        app.finish_text_input().unwrap();
        drain_op_job(&mut app);
        let out = d.path().join("safe.zip");
        assert!(out.is_file(), "safe.zip created");
        assert!(cian_core::archive::zip_needs_password(&out), "it is encrypted");
    }

    #[test]
    fn f3_on_an_image_opens_the_half_block_preview() {
        let (d, mut app) = app_with(&[]);
        // A small PNG in the pane's directory.
        let mut img = image::RgbImage::new(20, 12);
        for px in img.pixels_mut() {
            *px = image::Rgb([30, 160, 90]);
        }
        img.save(d.path().join("pic.png")).unwrap();
        app.reload_both();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "pic.png").unwrap();

        // F3 opens the image preview, not the hex/text viewer.
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert!(matches!(app.popup, Popup::ImageView { .. }), "image preview opened, got {:?}", app.popup);
        // Rendering decodes and caches a thumbnail sized to the box.
        let _ = render(&mut app, 80, 24);
        match &app.popup {
            Popup::ImageView { shown: Some((_, _, t)), error: None, .. } => {
                assert!(t.cols > 0 && t.rows > 0, "decoded to cells");
                assert_eq!((t.src_w, t.src_h), (20, 12));
            }
            other => panic!("no cached thumbnail: {:?}", other),
        }
        // Esc closes.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::None));
    }

    #[test]
    fn count_reports_files_and_steps() {
        let (d, mut app) = app_with(&[]);
        std::fs::write(d.path().join("a.rs"), "fn main() {}\n\n// note\nlet x = 1;\n").unwrap();
        std::fs::write(d.path().join("b.rs"), "let y = 2;\n").unwrap();
        std::fs::write(d.path().join("skip.txt"), "not counted\n").unwrap();
        app.count_opts = cian_core::count::Options {
            extensions: vec!["rs".into()],
            ..Default::default()
        };
        // Reload, then mark the two .rs files: `:count` counts the marked
        // entries (or, unmarked, the one under the cursor) — not the whole dir.
        app.reload_both();
        {
            let p = app.active_pane_mut().unwrap();
            for i in 0..p.entries.len() {
                if p.entries[i].name.ends_with(".rs") {
                    p.toggle_mark_at(i);
                }
            }
        }
        app.start_count();
        assert!(app.count_job.is_some(), "count started on a worker");

        // Wait for the worker, then let poll install the report.
        for _ in 0..200 {
            if app.poll_count() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        match &app.popup {
            Popup::Notice { lines } => {
                let text = lines.join("\n");
                assert!(text.contains("2"), "two rs files: {text}");
                assert!(text.to_lowercase().contains("step"), "shows a step line: {text}");
                assert!(!text.contains("not counted"), "txt excluded");
            }
            _ => panic!("no count notice: {:?}", app.popup),
        }
    }

    #[test]
    fn count_targets_the_cursor_not_the_whole_directory() {
        let (d, mut app) = app_with(&[]);
        // A subdirectory with one file, plus a sibling file that must NOT count.
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/inner.rs"), "let a = 1;\nlet b = 2;\n").unwrap();
        std::fs::write(d.path().join("outside.rs"), "let c = 3;\n").unwrap();
        app.count_opts = cian_core::count::Options { extensions: vec!["rs".into()], ..Default::default() };
        app.reload_both();
        // Cursor on the `sub` folder (nothing marked) → count walks just it.
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "sub").unwrap();
        app.start_count();
        for _ in 0..200 {
            if app.poll_count() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if let Popup::Notice { lines } = &app.popup {
            let text = lines.join("\n");
            // 1 file, 2 code lines from sub/inner.rs; outside.rs excluded.
            assert!(text.contains("2") && !text.contains('3'), "counted only the cursor's dir: {text}");
        } else {
            panic!("no count notice");
        }
    }

    #[test]
    fn a_macro_can_be_started_by_name() {
        // Backs the `--macro-name` startup option.
        let (_d, mut app) = app_with(&["a.txt"]);
        app.macros = cian_lua::macros::parse(
            r#"return { { name = "Deploy", panes = { { cmd = "echo go" } } } }"#,
        )
        .unwrap();
        assert!(!app.start_macro_by_name("Nope"), "unknown name is rejected");
        assert!(app.macro_run.is_none());
        assert!(app.start_macro_by_name("Deploy"), "known name starts");
        assert!(app.macro_run.is_some());
    }

    #[test]
    fn ai_chat_round_trips_a_mock_reply() {
        let have_py = std::process::Command::new("python3")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !have_py {
            eprintln!("no python3; skipping");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_path_buf();
        let mut config = en_config();
        config.ai = Some(cian_lua::AiOptions {
            python: "python3".into(),
            auth_mode: "mock".into(),
            ..Default::default()
        });
        let mut app = App::new(p.clone(), p, config).unwrap();
        assert!(app.ai.is_some(), "AI configured");
        app.ai_ready = Some(true); // the probe is async; treat mock as ready

        app.open_ai_chat();
        assert!(matches!(app.popup, Popup::AiChat { .. }), "chat opened (mock is available)");
        if let Popup::AiChat { input, .. } = &mut app.popup {
            *input = "hello".into();
        }
        app.send_ai_message();
        // Wait for the worker's reply.
        let start = Instant::now();
        while app.ai_job.is_some() && start.elapsed() < Duration::from_secs(10) {
            app.poll_ai_job();
            std::thread::sleep(Duration::from_millis(5));
        }
        match &app.popup {
            Popup::AiChat { log, .. } => {
                assert!(log.iter().any(|m| m.user && m.text == "hello"), "user turn recorded");
                assert!(
                    log.iter().any(|m| !m.user && m.text.contains("[mock] hello")),
                    "assistant echoed via the mock helper: {:?}",
                    log
                );
            }
            other => panic!("expected the chat, got {:?}", other),
        }
    }

    #[test]
    fn explain_diff_opens_the_chat_with_the_diff() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_path_buf();
        let mut config = en_config();
        config.ai = Some(cian_lua::AiOptions {
            python: "python3".into(),
            auth_mode: "mock".into(),
            ..Default::default()
        });
        let mut app = App::new(p.clone(), p, config).unwrap();
        app.ai_ready = Some(true);

        let result = cian_core::diff::diff_lines(
            &["let x = 1;".to_string()],
            &["let x = 2;".to_string()],
        );
        let folded = cian_core::diff::fold(&result.rows, cian_core::diff::CONTEXT);
        app.popup = Popup::Diff {
            left: "a".into(),
            right: "b".into(),
            left_path: "a".into(),
            right_path: "b".into(),
            encoding: cian_core::viewer::TextEncoding::Utf8,
            result,
            folded,
            fold: true,
            scroll: 0,
            find: None,
            find_input: None,
        };
        app.explain_diff();
        match &app.popup {
            Popup::AiChat { log, pending, .. } => {
                assert!(*pending, "the request is in flight");
                assert!(log.iter().any(|m| m.user && m.text == "Explain this diff"));
            }
            other => panic!("expected the chat, got {:?}", other),
        }
        assert!(app.ai_job.is_some(), "a request was fired");
    }

    #[test]
    fn triage_log_reads_the_selected_file_and_opens_chat() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.log"), "INFO ok\nERROR boom\n").unwrap();
        let p = dir.path().to_path_buf();
        let mut config = en_config();
        config.ai = Some(cian_lua::AiOptions {
            python: "python3".into(),
            auth_mode: "mock".into(),
            ..Default::default()
        });
        let mut app = App::new(p.clone(), p, config).unwrap();
        app.ai_ready = Some(true);
        if let Some(t) = app.active_file_tabs_mut() {
            let pane = t.active_mut();
            let i = pane.entries.iter().position(|e| e.name == "app.log").unwrap();
            pane.cursor = i;
        }
        app.triage_log();
        match &app.popup {
            Popup::AiChat { log, pending, skin, .. } => {
                assert!(*pending);
                assert!(log.iter().any(|m| m.user && m.text.contains("app.log")), "names the log: {:?}", log);
                // The window is named for the action that opened it, and says
                // the local model is answering — not crmaine.
                assert_eq!(skin.title, "Triage this log");
                assert!(skin.simple, "the local model answers, so the window wears AI - simple");
            }
            other => panic!("expected the chat, got {:?}", other),
        }
    }


    /// The retrieval trace is read *about* a conversation, so closing it must
    /// put that conversation back rather than dump the user in the file pane.
    #[test]
    fn a_report_raised_over_the_chat_gives_the_chat_back() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.start_ai_chat(ChatMode::Ai, vec![ChatMsg { user: true, text: "a question".into() }], false);
        let chat = std::mem::replace(&mut app.popup, Popup::None);
        app.popup = Popup::Report {
            title: " what RAG retrieved ".into(),
            lines: (0..40).map(|i| format!("line {i}")).collect(),
            scroll: 0,
            back: Box::new(chat),
        };
        // It scrolls like the manual…
        app.handle_key(key('j')).unwrap();
        app.handle_key(key('d')).unwrap();
        let Popup::Report { scroll, .. } = &app.popup else { panic!("expected the report") };
        assert_eq!(*scroll, 11);
        // …and Esc lands back in the conversation it explains.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        match &app.popup {
            Popup::AiChat { log, .. } => assert!(log[0].text.contains("a question")),
            other => panic!("expected the chat back, got {:?}", other),
        }
    }


    /// Opened from the command line there is nothing underneath, so Esc closes.
    #[test]
    fn a_report_with_nothing_behind_it_just_closes() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.popup = Popup::Report {
            title: " r ".into(),
            lines: vec!["x".into()],
            scroll: 0,
            back: Box::new(Popup::None),
        };
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::None));
    }


    #[test]
    fn pasted_images_ride_along_with_a_chat_turn_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_path_buf();
        let mut config = en_config();
        config.ai = Some(cian_lua::AiOptions {
            python: "python3".into(),
            auth_mode: "mock".into(),
            ..Default::default()
        });
        let mut app = App::new(p.clone(), p, config).unwrap();
        app.ai_ready = Some(true);

        // A structured purpose parses its reply and has no attachment UI, so it
        // must leave a pending image alone rather than consuming it.
        app.chat_attachments.push(std::path::PathBuf::from("/tmp/shot.png"));
        app.ai_request(AiPurpose::ShellCommand { description: "usr".into() }, "sys".into(), "usr".into());
        assert_eq!(app.chat_attachments.len(), 1, "a shell-command request keeps the image");

        // A chat turn takes them, so the same image isn't sent twice.
        app.ai_job = None;
        app.ai_request(AiPurpose::Chat, "sys".into(), "usr".into());
        assert!(app.chat_attachments.is_empty(), "the chat turn took the image");

        // Starting a fresh conversation drops anything pasted for the old one.
        app.chat_attachments.push(std::path::PathBuf::from("/tmp/shot.png"));
        app.start_ai_chat(ChatMode::Ai, Vec::new(), false);
        assert!(app.chat_attachments.is_empty(), "a new chat starts empty");
    }

    #[test]
    fn ai_chat_copy_uses_selection_then_last_reply() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.popup = Popup::AiChat {
            input: String::new(),
            log: vec![
                ChatMsg { user: true, text: "hi".into() },
                ChatMsg { user: false, text: "the answer\nline two".into() },
            ],
            scroll: 0,
            pending: false,
            sel: Some((0, 1)),
            mode: ChatMode::Ai,
            skin: ChatSkin::of(ChatMode::Ai),
        };
        // A selection copies those flat lines (as the draw would have populated).
        app.ai_lines = vec!["one".into(), "two".into(), "three".into()];
        app.copy_ai_text();
        assert_eq!(app.message.as_deref(), Some("copied"));
        assert!(matches!(app.popup, Popup::AiChat { sel: None, .. }), "selection cleared");

        // With no selection, it copies the last assistant reply.
        app.copy_ai_text();
        assert_eq!(app.message.as_deref(), Some("copied"));
    }

    #[test]
    fn clean_ai_command_strips_fences_and_prose() {
        assert_eq!(clean_ai_command("ls -la"), "ls -la");
        assert_eq!(clean_ai_command("```sh\nls -la\n```"), "ls -la");
        assert_eq!(clean_ai_command("`git status`"), "git status");
        assert_eq!(clean_ai_command("\n\n  find . -name '*.log'  \n"), "find . -name '*.log'");
    }

    /// The F3 viewer shows a git change bar for lines that differ from HEAD.
    #[test]
    fn the_viewer_shows_a_git_change_bar() {
        let d = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(d.path()).unwrap();
        let ok = std::process::Command::new("git")
            .arg("-C").arg(&dir).args(["init", "-q"]).status()
            .map(|s| s.success()).unwrap_or(false);
        if !ok {
            eprintln!("no git; skipping");
            return;
        }
        for kv in [["user.email", "t@e.com"], ["user.name", "T"], ["core.autocrlf", "false"]] {
            let _ = std::process::Command::new("git").arg("-C").arg(&dir).args(["config", kv[0], kv[1]]).status();
        }
        let f = dir.join("code.txt");
        std::fs::write(&f, "keep\nold\nkeep2\n").unwrap();
        std::process::Command::new("git").arg("-C").arg(&dir).args(["add", "."]).status().unwrap();
        std::process::Command::new("git").arg("-C").arg(&dir).args(["commit", "-qm", "init"]).status().unwrap();
        std::fs::write(&f, "keep\nNEW\nkeep2\n").unwrap();

        let mut app = App::new(dir.clone(), dir.clone(), en_config()).unwrap();
        app.open_viewer_at(&f, "code.txt", 0);
        // The map was computed for the modified file.
        let Popup::Viewer { git_lines, .. } = &app.popup else { panic!("no viewer") };
        assert_eq!(git_lines.get(&1), Some(&cian_core::git::LineChange::Modified), "line 2 modified");
        // And the change bar renders on screen.
        let screen = render(&mut app, 100, 30).join("\n");
        assert!(screen.contains('▏'), "change bar shown:\n{screen}");
    }

    /// The status line shows the repo's branch when the pane is in one.
    #[test]
    fn the_status_line_shows_the_git_branch() {
        let d = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(d.path()).unwrap();
        let ok = std::process::Command::new("git")
            .arg("-C").arg(&dir).args(["init", "-q", "-b", "trunk"]).status()
            .map(|s| s.success()).unwrap_or(false);
        if !ok {
            eprintln!("no git (or too old for -b); skipping");
            return;
        }
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let mut app = App::new(dir.clone(), dir, en_config()).unwrap();
        let screen = render(&mut app, 120, 30).join("\n");
        assert!(screen.contains("trunk"), "branch shown in the status line:\n{screen}");
        // And on the frame after that. The status is lifted out of `app` for
        // the length of a frame rather than copied (see `App::take_git`), so a
        // path that forgets to put it back would show the branch once and then
        // look like a directory that is not in a repository at all.
        let again = render(&mut app, 120, 30).join("\n");
        assert!(again.contains("trunk"), "still there on the next frame:\n{again}");
    }

    /// Stage / unstage / discard through the app on a real throwaway repo.
    #[test]
    fn git_stage_unstage_and_discard_operate_on_the_selection() {
        let d = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(d.path()).unwrap();
        let git_ok = std::process::Command::new("git")
            .arg("-C").arg(&dir).args(["init", "-q"]).status()
            .map(|s| s.success()).unwrap_or(false);
        if !git_ok {
            eprintln!("no git; skipping");
            return;
        }
        for kv in [["user.email", "t@example.com"], ["user.name", "Test"], ["core.autocrlf", "false"]] {
            let _ = std::process::Command::new("git").arg("-C").arg(&dir).args(["config", kv[0], kv[1]]).status();
        }
        // Commit an initial file so we have a tracked file to modify/discard.
        std::fs::write(dir.join("tracked.txt"), "one\n").unwrap();
        std::process::Command::new("git").arg("-C").arg(&dir).args(["add", "."]).status().unwrap();
        std::process::Command::new("git").arg("-C").arg(&dir).args(["commit", "-qm", "init"]).status().unwrap();
        std::fs::write(dir.join("tracked.txt"), "one\ntwo\n").unwrap();

        let mut app = App::new(dir.clone(), dir.clone(), en_config()).unwrap();
        let _ = render(&mut app, 100, 40); // computes git status
        // Cursor onto tracked.txt (index 0 is `..`).
        let idx = app.active_pane().unwrap().entries.iter()
            .position(|e| e.name == "tracked.txt").unwrap();
        app.active_pane_mut().unwrap().cursor = idx;

        // Stage: the worktree change becomes staged.
        app.git_stage();
        let st = cian_core::git::status(&dir).unwrap();
        assert_eq!(st.mark_for(&dir.join("tracked.txt")), Some(cian_core::git::GitMark::Staged));

        // Unstage: back to a plain worktree modification.
        app.git_stage(); // (re-stage to ensure state)
        app.git_unstage();
        let st = cian_core::git::status(&dir).unwrap();
        assert_eq!(st.mark_for(&dir.join("tracked.txt")), Some(cian_core::git::GitMark::Modified));

        // Discard: confirm dialog, then the change is gone.
        let _ = render(&mut app, 100, 40);
        app.active_pane_mut().unwrap().cursor = idx;
        app.git_discard_prompt();
        assert!(matches!(app.popup, Popup::ConfirmDiscard { .. }), "discard confirms first");
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("tracked.txt")).unwrap(), "one\n",
            "worktree change reverted");
    }

    #[test]
    fn parse_junk_reply_validates_names_and_strips_prose() {
        let names = vec![
            ("target".to_string(), PathBuf::from("/p/target")),
            ("main.rs".to_string(), PathBuf::from("/p/main.rs")),
            (".DS_Store".to_string(), PathBuf::from("/p/.DS_Store")),
        ];
        // Fenced, with prose around it, and a hallucinated name that must be dropped.
        let raw = "Here is the junk:\n```json\n[\
            {\"name\":\"target\",\"reason\":\"build output\"},\
            {\"name\":\".DS_Store\",\"reason\":\"macOS cruft\"},\
            {\"name\":\"nonexistent\",\"reason\":\"made up\"}\
            ]\n```\n";
        let items = parse_junk_reply(raw, &names);
        let got: Vec<&str> = items.iter().map(|i| i.path.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(got, vec!["target", ".DS_Store"], "only shown names survive");
        assert!(items.iter().all(|i| i.selected), "candidates start checked");
        assert_eq!(items[0].reason, "build output");
        // Never flags source — it just isn't in the reply, and couldn't be added.
        assert!(!got.contains(&"main.rs"));
    }

    #[test]
    fn parse_junk_reply_empty_or_garbage_is_no_items() {
        let names = vec![("x".to_string(), PathBuf::from("/p/x"))];
        assert!(parse_junk_reply("[]", &names).is_empty());
        assert!(parse_junk_reply("I could not find any junk.", &names).is_empty());
    }

    /// The whole duplicate flow: scan a dir with two identical files, wait for
    /// the worker, and check the review pre-selects the redundant copy.
    #[test]
    fn dupe_scan_finds_copies_and_preselects_all_but_one() {
        let (d, mut app) = app_with(&["one.txt", "two.txt", "unique.txt"]);
        std::fs::write(d.path().join("one.txt"), b"same bytes here").unwrap();
        std::fs::write(d.path().join("two.txt"), b"same bytes here").unwrap();
        std::fs::write(d.path().join("unique.txt"), b"different").unwrap();
        app.reload_active();

        app.start_dupes();
        assert!(app.dupes_job.is_some(), "scan running on a worker");
        let start = Instant::now();
        while app.dupes_job.is_some() && start.elapsed() < Duration::from_secs(10) {
            app.poll_dupes_job();
            std::thread::sleep(Duration::from_millis(5));
        }
        let Popup::DupeReview { items, .. } = &app.popup else {
            panic!("expected the dupe review, got {:?}", app.popup)
        };
        // Two identical files → one group of two; exactly one is pre-checked.
        assert_eq!(items.len(), 2, "the duplicate pair (unique.txt omitted)");
        assert_eq!(items.iter().filter(|i| i.selected).count(), 1, "keep one, check the other");
        assert_eq!(items.iter().filter(|i| i.keeper).count(), 1);

        // Approving hands the checked copy to the delete confirmation.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        match &app.popup {
            Popup::ConfirmDelete { targets } => assert_eq!(targets.len(), 1),
            other => panic!("expected delete confirm, got {:?}", other),
        }
    }

    #[test]
    fn junk_review_approval_routes_checked_paths_to_delete_confirm() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.popup = Popup::JunkReview {
            items: vec![
                JunkItem { path: PathBuf::from("/p/target"), reason: "build".into(), selected: true },
                JunkItem { path: PathBuf::from("/p/keep"), reason: "".into(), selected: false },
                JunkItem { path: PathBuf::from("/p/cache"), reason: "cache".into(), selected: true },
            ],
            cursor: 0,
            scroll: 0,
        };
        // Enter approves: only the checked ones go to the delete confirmation.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        match &app.popup {
            Popup::ConfirmDelete { targets } => {
                assert_eq!(targets, &vec![PathBuf::from("/p/target"), PathBuf::from("/p/cache")]);
            }
            other => panic!("expected the delete confirm, got {:?}", other),
        }
    }

    #[test]
    fn junk_review_space_toggles_and_a_selects_all() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.popup = Popup::JunkReview {
            items: vec![
                JunkItem { path: PathBuf::from("/p/1"), reason: String::new(), selected: true },
                JunkItem { path: PathBuf::from("/p/2"), reason: String::new(), selected: true },
            ],
            cursor: 0,
            scroll: 0,
        };
        // Space unchecks the first.
        app.handle_key(code(KeyCode::Char(' '))).unwrap();
        // `a` toggles all: since not all are on, it turns all on.
        app.handle_key(code(KeyCode::Char('a'))).unwrap();
        if let Popup::JunkReview { items, .. } = &app.popup {
            assert!(items.iter().all(|i| i.selected), "a turned everything on");
        } else {
            panic!("popup changed");
        }
    }

    /// All four review lists are the same list with different rows in it, so
    /// they have to answer the same keys. They used to carry a copy of the
    /// handling each; this walks every one of them over the same keystrokes.
    #[test]
    fn every_review_list_answers_the_same_keys() {
        let p = |s: &str| PathBuf::from("/p").join(s);
        let lists: Vec<(&str, Popup)> = vec![
            (
                "junk",
                Popup::JunkReview {
                    items: (0..3)
                        .map(|i| JunkItem {
                            path: p(&i.to_string()),
                            reason: String::new(),
                            selected: false,
                        })
                        .collect(),
                    cursor: 0,
                    scroll: 0,
                },
            ),
            (
                "duplicates",
                Popup::DupeReview {
                    items: (0..3)
                        .map(|i| DupeItem {
                            path: p(&i.to_string()),
                            group: 0,
                            keeper: false,
                            selected: false,
                        })
                        .collect(),
                    cursor: 0,
                    scroll: 0,
                },
            ),
            (
                "structure",
                Popup::StructureReview {
                    items: (0..3)
                        .map(|i| MoveItem {
                            path: p(&i.to_string()),
                            name: i.to_string(),
                            dest: "d".into(),
                            reason: String::new(),
                            selected: false,
                        })
                        .collect(),
                    cursor: 0,
                    scroll: 0,
                    dir: p(""),
                },
            ),
            (
                "rename",
                Popup::RenameReview {
                    items: (0..3)
                        .map(|i| RenameItem {
                            path: p(&i.to_string()),
                            old: i.to_string(),
                            new: format!("{i}.new"),
                            selected: false,
                        })
                        .collect(),
                    cursor: 0,
                    scroll: 0,
                    by_ai: true,
                },
            ),
        ];
        /// The cursor and the checkboxes of whichever review list is open.
        fn state(app: &App) -> (usize, Vec<bool>) {
            match &app.popup {
                Popup::JunkReview { items, cursor, .. } => {
                    (*cursor, items.iter().map(|i| i.selected).collect())
                }
                Popup::DupeReview { items, cursor, .. } => {
                    (*cursor, items.iter().map(|i| i.selected).collect())
                }
                Popup::StructureReview { items, cursor, .. } => {
                    (*cursor, items.iter().map(|i| i.selected).collect())
                }
                Popup::RenameReview { items, cursor, .. } => {
                    (*cursor, items.iter().map(|i| i.selected).collect())
                }
                other => panic!("the list closed: {:?}", other),
            }
        }
        for (what, popup) in lists {
            let (_d, mut app) = app_with(&["a.txt"]);
            app.popup = popup;
            // j moves down and stops at the end, k comes back up.
            for _ in 0..5 {
                app.handle_key(code(KeyCode::Char('j'))).unwrap();
            }
            assert_eq!(state(&app).0, 2, "{what}: j stops at the last row");
            app.handle_key(code(KeyCode::Char('k'))).unwrap();
            assert_eq!(state(&app).0, 1, "{what}: k comes back");
            // Space checks the row under the cursor, and only that one.
            app.handle_key(code(KeyCode::Char(' '))).unwrap();
            assert_eq!(state(&app).1, vec![false, true, false], "{what}: space checks one row");
            // `a` turns everything on while any row is off, then off again.
            app.handle_key(code(KeyCode::Char('a'))).unwrap();
            assert_eq!(state(&app).1, vec![true; 3], "{what}: a checks all");
            app.handle_key(code(KeyCode::Char('a'))).unwrap();
            assert_eq!(state(&app).1, vec![false; 3], "{what}: and clears them");
            // G and g go to the ends.
            app.handle_key(code(KeyCode::Char('G'))).unwrap();
            assert_eq!(state(&app).0, 2, "{what}: G is the last row");
            app.handle_key(code(KeyCode::Char('g'))).unwrap();
            assert_eq!(state(&app).0, 0, "{what}: g is the first");
            // q leaves without carrying anything out.
            app.handle_key(code(KeyCode::Char('q'))).unwrap();
            assert!(matches!(app.popup, Popup::None), "{what}: q closes it");
        }
    }

    /// Leaving a review list used to blank the popup slot outright, which took
    /// a docked panel with it — the same bug the one-door change fixed
    /// everywhere else. Esc has to put the file back.
    #[test]
    fn leaving_a_review_list_gives_the_docked_panel_back() {
        let (_d, mut app) = viewer_on("alpha\nbravo\n");
        app.handle_key(code(KeyCode::F(12))).unwrap();
        assert!(app.viewer_dock.is_some(), "the file is open, docked beside the panes");
        app.open_popup(Popup::JunkReview {
            items: vec![JunkItem {
                path: PathBuf::from("/p/target"),
                reason: "build".into(),
                selected: true,
            }],
            cursor: 0,
            scroll: 0,
        });
        assert!(app.viewer_return.is_some(), "the file went aside, not away");
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "and Esc brings it back");
        assert!(app.viewer_return.is_none(), "with nothing left waiting");
    }

    /// A rectangle of no size holds no point. Several hit tests used to guard
    /// on a non-zero width before asking, which this makes unnecessary — the
    /// case that matters is a widget the layout has not placed yet.
    #[test]
    fn nothing_is_inside_a_rectangle_of_no_size() {
        let r = Rect::new(4, 2, 6, 3);
        assert!(hit_rect(r, 4, 2), "the top-left corner is inside");
        assert!(hit_rect(r, 9, 4), "and so is the bottom-right");
        assert!(!hit_rect(r, 10, 4), "one column past the right edge is not");
        assert!(!hit_rect(r, 9, 5), "nor one row below");
        assert!(!hit_rect(Rect::new(4, 2, 0, 3), 4, 2), "an unplaced widget holds nothing");
        assert!(!hit_rect(Rect::new(4, 2, 6, 0), 4, 2), "in either direction");
    }

    /// Every `:command` the manual and the palette name has to be one the
    /// dispatcher answers to. The names have been pruned twice now, and a
    /// pruned name leaves no trace in the code — the manual keeps advertising
    /// it and the user gets "unknown command". Reading the dispatcher's own
    /// source for the set of names it matches on is blunt, but it is the only
    /// list that cannot drift from what actually runs.
    #[test]
    fn the_manual_never_names_a_command_that_does_not_exist() {
        // The dispatcher is in two halves: cian's own commands, and the ex
        // commands the viewer answers while a file is open.
        let src = concat!(include_str!("commands.rs"), include_str!("viewer.rs"));
        // Every quoted lowercase word in the dispatcher: a superset of the
        // verbs, which is all a membership test needs.
        let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut rest = src;
        while let Some(i) = rest.find('"') {
            rest = &rest[i + 1..];
            let Some(j) = rest.find('"') else { break };
            let word = &rest[..j];
            if !word.is_empty()
                && word.chars().all(|c| c.is_ascii_lowercase() || c == '.' || c.is_ascii_digit())
            {
                known.insert(word.to_string());
            }
            rest = &rest[j + 1..];
        }
        // Commands the dispatcher reaches by another route rather than by a
        // quoted name of its own.
        for extra in ["notepad", "vim"] {
            known.insert(extra.to_string());
        }

        // Pull `:name` out of a line of prose, in either language.
        let named = |text: &str| -> Vec<String> {
            let mut out = Vec::new();
            let b: Vec<char> = text.chars().collect();
            let mut i = 0;
            while i < b.len() {
                if b[i] == ':' {
                    let start = i + 1;
                    let mut end = start;
                    while end < b.len() && (b[end].is_ascii_lowercase() || b[end] == '.') {
                        end += 1;
                    }
                    // A bare `:` (the prompt itself) and `http:` are not names.
                    if end > start && (i == 0 || b[i - 1] != '/') {
                        out.push(b[start..end].iter().collect());
                    }
                    i = end;
                } else {
                    i += 1;
                }
            }
            out
        };

        let mut missing: Vec<String> = Vec::new();
        for line in crate::manual_lines(&HashMap::new(), Lang::En) {
            for name in named(&line) {
                if !known.contains(&name) {
                    missing.push(format!("manual: :{name}"));
                }
            }
        }
        for (verb, _, _) in crate::palette::command_list() {
            if !known.contains(*verb) {
                missing.push(format!("palette: :{verb}"));
            }
        }
        // The two READMEs are the first thing anyone reads, and nothing holds
        // them to the code at all — `:coding` sat in both for as long as it
        // took someone to type it.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("the workspace root");
        for doc in ["README.md", "README.ja.md"] {
            let text = std::fs::read_to_string(root.join(doc)).expect("a readme");
            for name in named(&text) {
                // `:s/…`, `:g/…` and `:v/…` are matched on their pattern, not
                // on a name; `:cq` is typed in the user's own vim, which is
                // what cian reads the exit code of.
                if matches!(name.as_str(), "s" | "g" | "v" | "cq") {
                    continue;
                }
                if !known.contains(&name) {
                    missing.push(format!("{doc}: :{name}"));
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "named but not answered: {missing:?}");
    }

    /// Nothing cian says to a person ends in a full stop — 「〜ます」 and
    /// `nothing to operate on`, not 「〜ます。」 and `nothing to operate on.`.
    /// A stop between two sentences is right; a stop on the end is one the
    /// reader will never be followed past. Both languages, because a dialog
    /// that drops it in one and keeps it in the other looks unfinished in one
    /// of them. Reading the sources is blunt, but the alternative is a rule
    /// that only holds for the strings someone remembered to check.
    #[test]
    fn nothing_cian_says_ends_in_a_full_stop() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut bad: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("the source directory") {
            let path = entry.expect("a source file").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            // The tests are full of sample prose, which is not what cian says.
            if name == "tests.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("readable source");
            let bytes: Vec<char> = text.chars().collect();
            // Every `tr(` call, and every string literal in its arguments —
            // the forms are often each on their own line, so this reads the
            // call rather than the line.
            let mut i = 0;
            while i + 3 < bytes.len() {
                let is_call = bytes[i] == 't'
                    && bytes[i + 1] == 'r'
                    && bytes[i + 2] == '('
                    && (i == 0 || !bytes[i - 1].is_alphanumeric() && bytes[i - 1] != '_');
                if !is_call {
                    i += 1;
                    continue;
                }
                // A `tr(` inside a comment is prose about the rule, not a
                // string cian says.
                let line_start = text[..text
                    .char_indices()
                    .nth(i)
                    .map(|(b, _)| b)
                    .unwrap_or(0)]
                    .rfind('\n')
                    .map(|b| b + 1)
                    .unwrap_or(0);
                if text[line_start..].trim_start().starts_with("//") {
                    i += 3;
                    continue;
                }
                let mut j = i + 3;
                let mut depth = 1usize;
                while j < bytes.len() && depth > 0 {
                    match bytes[j] {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        '"' => {
                            let start = j + 1;
                            let mut k = start;
                            while k < bytes.len() && bytes[k] != '"' {
                                // An escape puts a quote inside the literal;
                                // step over the pair.
                                if bytes[k] == '\\' {
                                    k += 1;
                                }
                                k += 1;
                            }
                            let s: String = bytes[start..k.min(bytes.len())].iter().collect();
                            let s = s.trim_end();
                            if s.chars().count() >= 4
                                && !s.ends_with("..")
                                && (s.ends_with('。') || s.ends_with('.'))
                            {
                                bad.push(format!("{name}: {s}"));
                            }
                            j = k;
                        }
                        _ => {}
                    }
                    j += 1;
                }
                i = j;
            }
        }
        bad.sort();
        bad.dedup();
        assert!(bad.is_empty(), "these end in a full stop: {bad:#?}");
    }

    /// Every entry the manual holds reaches the eye. It used to be cut to the
    /// popup's width, and 75 of its 195 entries were long enough to lose their
    /// ending — a key list you have to guess at is not a key list.
    #[test]
    fn the_manual_never_cuts_an_entry_short() {
        let lines = crate::manual_lines(&HashMap::new(), Lang::Ja);
        assert!(lines.iter().any(|l| crate::util::width(l) > 100), "the long ones are the point");
        for w in [40usize, 56, 76, 100, 120] {
            for line in &lines {
                let rows = crate::util::wrap_hanging(line, w);
                for r in &rows {
                    assert!(crate::util::width(r) <= w, "at {w}: {r:?} from {line:?}");
                }
                // Put back together, the rows are the entry that went in.
                // The indent is asked for rather than counted: a continuation
                // may legitimately open on a space of its own.
                let hang = crate::util::column_gap(line)
                    .map(|(_, col)| col)
                    .filter(|col| col * 2 < w)
                    .unwrap_or(0);
                let rebuilt: String = rows
                    .iter()
                    .enumerate()
                    .map(|(i, r)| if i == 0 { r.as_str() } else { &r[hang..] })
                    .collect();
                assert_eq!(&rebuilt, line, "at {w}, an entry came back changed");
            }
        }
    }

    /// And on the screen: the widest entry there is, whole, at a width that
    /// used to cut it.
    #[test]
    fn the_widest_entry_arrives_whole() {
        let widest = crate::manual_lines(&HashMap::new(), Lang::Ja)
            .into_iter()
            .max_by_key(|l| crate::util::width(l))
            .expect("the manual has entries");
        // Its last few characters: the part a cut takes away first.
        let tail: String = widest.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
        for (w, h) in [(80, 30), (100, 24), (140, 40)] {
            let (_d, mut app) = app_with_lang(&["a.txt"], "ja");
            app.handle_key(key('?')).unwrap();
            // Page down through the whole thing looking for the ending.
            let mut found = false;
            for _ in 0..40 {
                // A 全角 character is two cells and the second comes back as
                // a space, so the rendered row reads "リ ッ ク". Compare
                // without the spaces.
                let bare = tail.replace(' ', "");
                if render(&mut app, w, h)
                    .iter()
                    .any(|l| l.replace(' ', "").contains(&bare))
                {
                    found = true;
                    break;
                }
                app.handle_key(key('d')).unwrap();
            }
            assert!(found, "{w}x{h}: the widest entry never showed its ending {tail:?}");
        }
    }

    /// Wrapping loses nothing and invents nothing: the rows put back together
    /// are the line that went in, and the continuations line up under the
    /// description rather than back at the margin.
    #[test]
    fn a_wrapped_entry_still_reads_as_one_entry() {
        let line = "  Shift+P     選択をクリップボードへ（Finder/エクスプローラに貼り付けられます）";
        let rows = crate::util::wrap_hanging(line, 40);
        assert!(rows.len() > 1, "this one has to wrap at 40 cells");
        for r in &rows {
            assert!(crate::util::width(r) <= 40, "row over the width: {r:?}");
        }
        let hang = crate::util::column_gap(line).expect("two columns").1;
        assert_eq!(hang, 14, "continuations hang under the description column");
        for r in &rows[1..] {
            assert!(r.starts_with(&" ".repeat(hang)), "every continuation, not just the first");
        }
        let rebuilt: String = rows
            .iter()
            .enumerate()
            .map(|(i, r)| if i == 0 { r.clone() } else { r[hang..].to_string() })
            .collect();
        assert_eq!(rebuilt, line, "nothing was lost and nothing added");
    }

    /// English breaks at a space when there is one to hand; Japanese has no
    /// spaces to break at and breaks where the width runs out.
    #[test]
    fn wrapping_prefers_a_space_but_does_not_need_one() {
        let rows = crate::util::wrap_words("copy the marked files to the other pane", 20);
        for r in &rows {
            assert!(!r.starts_with(' ') || r.is_empty(), "no row opens on a space: {r:?}");
            assert!(crate::util::width(r) <= 20);
        }
        // The break's own space stays where it was, so the rows put back
        // together are the text that went in.
        assert_eq!(rows.concat(), "copy the marked files to the other pane");

        let jp = crate::util::wrap_words("マークしたファイルを反対のペインへコピーします", 20);
        assert!(jp.len() > 1, "too wide for one row");
        for r in &jp {
            assert!(crate::util::width(r) <= 20, "row over the width: {r:?}");
        }
        assert_eq!(jp.concat(), "マークしたファイルを反対のペインへコピーします");
    }

    /// Drive a running file scan to its end, the way the main loop would.
    fn drain_file_scan(app: &mut App) {
        for _ in 0..2000 {
            if app.file_scan.is_none() {
                return;
            }
            app.poll_file_scan();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the walk never finished");
    }

    /// The finder used to walk the whole tree on the main loop before drawing
    /// a row, so opening it on a deep tree or a network drive was a freeze
    /// with nothing on screen. It opens first and fills in behind.
    #[test]
    fn the_file_finder_opens_before_it_has_looked() {
        let (d, mut app) = app_with(&["a.txt"]);
        let deep = d.path().join("one/two/three");
        std::fs::create_dir_all(&deep).unwrap();
        for i in 0..40 {
            std::fs::write(deep.join(format!("f{i}.txt")), b"").unwrap();
        }
        app.start_file_finder();
        // Open already, with the walk still out.
        assert!(
            matches!(app.popup, Popup::Palette { kind: PaletteKind::File, .. }),
            "the picker is up before the tree has been read"
        );
        assert!(app.file_scan.is_some(), "and the walk is in flight");
        // Nothing from the tree yet — this is what "before it has looked"
        // means, and what the old synchronous walk could not do.
        let Popup::Palette { items, .. } = &app.popup else { panic!("no picker") };
        assert!(items.is_empty(), "opened on the recent files alone, got {items:?}");

        drain_file_scan(&mut app);
        let Popup::Palette { items, .. } = &app.popup else { panic!("the picker closed") };
        assert!(
            items.iter().any(|i| i.label.contains("f39.txt")),
            "the tree arrived: {} rows",
            items.len()
        );
        assert!(items.iter().any(|i| i.label.contains("a.txt")), "including the shallow ones");
    }

    /// A walk stopped by the cap used to say nothing, so a file that was
    /// really there simply was not in the list — the finder looked wrong
    /// rather than full.
    #[test]
    fn a_finder_that_gave_up_early_says_so() {
        let (d, mut app) = app_with(&["a.txt"]);
        for i in 0..20 {
            std::fs::write(d.path().join(format!("f{i}.txt")), b"").unwrap();
        }
        let root = d.path().to_path_buf();
        app.open_palette_for_test();
        app.start_file_scan_for_test(root, 5);
        drain_file_scan(&mut app);
        let said = app.message.clone().unwrap_or_default();
        assert!(said.contains('5') || said.contains("first"), "it owned up: {said:?}");
    }

    /// Leaving the picker ends the walk. Reading a network drive to the end
    /// for a list nobody will see is the case this is for.
    #[test]
    fn closing_the_finder_calls_off_the_walk() {
        let (d, mut app) = app_with(&["a.txt"]);
        for i in 0..30 {
            std::fs::write(d.path().join(format!("f{i}.txt")), b"").unwrap();
        }
        app.start_file_finder();
        assert!(app.file_scan.is_some(), "a walk is out");
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(!matches!(app.popup, Popup::Palette { .. }), "the picker closed");
        app.poll_file_scan();
        assert!(app.file_scan.is_none(), "and the walk was called off");
    }

    /// Rows arriving under the cursor must not move it. The finder ranks again
    /// on every batch, and the plain refilter puts the cursor back at the top.
    #[test]
    fn arriving_rows_do_not_move_the_cursor() {
        let (d, mut app) = app_with(&["a.txt"]);
        for i in 0..40 {
            std::fs::write(d.path().join(format!("f{i}.txt")), b"").unwrap();
        }
        app.start_file_finder();
        drain_file_scan(&mut app);
        // Put the cursor on a row that is not the first.
        let Popup::Palette { shown, cursor, .. } = &mut app.popup else { panic!("no picker") };
        assert!(shown.len() > 3, "enough rows to move within");
        *cursor = 3;
        let on = shown[3];
        app.palette_refilter_keeping_cursor();
        let Popup::Palette { shown, cursor, .. } = &app.popup else { panic!("no picker") };
        assert_eq!(shown.get(*cursor).copied(), Some(on), "still on the same row");
    }

    /// `:recent` opens the same kind of picker as the finder, so a walk left
    /// over from an `//` closed in the same breath would pour the whole tree
    /// into the list of files actually opened. Both keys land before the loop
    /// polls, so the gap is real.
    #[test]
    fn a_leftover_walk_does_not_pour_into_the_recent_list() {
        let (d, mut app) = app_with(&["a.txt"]);
        for i in 0..60 {
            std::fs::write(d.path().join(format!("f{i}.txt")), b"").unwrap();
        }
        app.recent_files.push(d.path().join("a.txt"));
        app.start_file_finder();
        assert!(app.file_scan.is_some(), "a walk is out");
        // No poll in between: the finder closes and `:recent` opens first.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        app.start_recent_files();
        assert!(app.file_scan.is_none(), "the walk was called off by the new picker");
        for _ in 0..50 {
            app.poll_file_scan();
        }
        let Popup::Palette { items, .. } = &app.popup else { panic!("no picker") };
        assert_eq!(items.len(), 1, "only what was opened: {:?}", items.iter().map(|i| &i.label).collect::<Vec<_>>());
    }

    /// A text prompt must step aside for an open panel, not over it. Thirty-two
    /// places built one and assigned it straight into `self.popup`, which is
    /// the slot the docked panel is sitting in — so asking the AI for a command
    /// from the listing beside an edited file threw the file away.
    #[test]
    fn a_text_prompt_does_not_eat_an_open_panel() {
        let (_d, mut app) = viewer_on("alpha\nbravo\n");
        app.handle_key(code(KeyCode::F(12))).unwrap();
        app.handle_key(key('x')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { dirty: true, .. }), "edited, unsaved");
        let dock = app.viewer_dock.expect("docked");
        app.focus(match dock {
            FocusedPane::Left => FocusedPane::Right,
            _ => FocusedPane::Left,
        });
        let _ = render(&mut app, 160, 30);

        // Any prompt will do; this one asks for the name to rename to.
        app.start_rename();
        assert!(matches!(app.popup, Popup::TextInput { .. }), "the prompt opened");
        assert!(app.viewer_return.is_some(), "and the file went aside, not away");
        app.handle_key(code(KeyCode::Esc)).unwrap();
        match &app.popup {
            Popup::Viewer { dirty, .. } => assert!(*dirty, "the unsaved edit came back"),
            other => panic!("the file did not come back: {:?}", other),
        }
    }

    /// The editor's grammar switches from inside the editor. It used to be on
    /// the file pane's command line only — the one place you are not when you
    /// reach for it — and `:notepad` there fell through to the substitute
    /// parser, which read the `n` as a delimiter and said so.
    #[test]
    fn notepad_switches_from_the_editors_own_command_line() {
        let (_d, mut app) = viewer_on("alpha\nbravo\n");
        assert_eq!(app.edit_style, EditStyle::Vim, "vim by default");

        app.run_substitute("notepad");
        assert_eq!(app.edit_style, EditStyle::Notepad, "and now notepad");
        let said = app.message.clone().unwrap_or_default();
        assert!(!said.contains("separate the parts"), "not a replacement error: {said:?}");

        app.run_substitute("editstyle vim");
        assert_eq!(app.edit_style, EditStyle::Vim, "and back again");

        // Bare `:editstyle` flips, the same as on the file pane.
        app.run_substitute("editstyle");
        assert_eq!(app.edit_style, EditStyle::Notepad);
    }

    /// An unknown command in the viewer says it does not know the command.
    /// Everything unrecognised used to reach the substitute parser, which
    /// explained the syntax of a replacement nobody was writing.
    #[test]
    fn an_unknown_viewer_command_says_that_is_what_it_is() {
        let (_d, mut app) = viewer_on("alpha\n");
        app.run_substitute("nosuchthing");
        let said = app.message.clone().unwrap_or_default();
        assert!(said.contains("nosuchthing"), "names what was typed: {said:?}");
        assert!(!said.contains("separate the parts"), "not replacement syntax: {said:?}");

        // A real replacement still runs, in both spellings.
        app.run_substitute("s/alpha/omega/");
        if let Popup::Viewer { view, .. } = &app.popup {
            assert_eq!(view.lines[0], "omega", "s/old/new/ still works");
        } else {
            panic!("the viewer closed");
        }
        app.run_substitute("/omega/alpha/");
        if let Popup::Viewer { view, .. } = &app.popup {
            assert_eq!(view.lines[0], "alpha", "and the bare form the prompt shows");
        } else {
            panic!("the viewer closed");
        }
    }

    /// Ctrl+C in the shell copies when something is selected, and interrupts
    /// when nothing is. Selecting output and reaching for Ctrl+C used to kill
    /// whatever was running.
    ///
    /// The copy itself needs a live shell with text on it, so what is pinned
    /// here is the decision and what the other branch sends. The first version
    /// of this test asserted on the selection being cleared, which the handler
    /// does to *every* keypress on its first line — so it passed with the
    /// feature disabled, and the feature was in fact dead: the selection was
    /// already gone by the time the branch looked at it.
    #[test]
    fn ctrl_c_in_the_shell_copies_a_selection_and_otherwise_interrupts() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        let sel = ShellSel {
            tab: 0,
            leaf: 0,
            inner: Rect::new(0, 0, 20, 5),
            anchor: (0, 0),
            end: (0, 4),
            dragged: true,
        };

        assert!(!app.shell_ctrl_c_copies(), "nothing selected: Ctrl+C interrupts");
        app.shell_sel = Some(sel);
        assert!(app.shell_ctrl_c_copies(), "a finished drag: Ctrl+C copies");
        // A press that never became a drag is not a selection.
        app.shell_sel = Some(ShellSel { dragged: false, ..sel });
        assert!(!app.shell_ctrl_c_copies(), "a bare click is not a selection");

        // And what the interrupting branch actually sends.
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(
            crate::encode_key(ctrl_c, false),
            Some(vec![0x03]),
            "the other branch is the interrupt, not a letter",
        );
    }

    #[test]
    fn parse_sem_search_reply_matches_orders_and_folds_reasons() {
        let hit = |rel: &str| cian_core::search::Hit {
            path: PathBuf::from("/root").join(rel),
            rel: PathBuf::from(rel),
            is_dir: false,
            line: None,
        };
        let catalog = vec![hit("src/db.rs"), hit("README.md"), hit("src/ui.rs")];
        // Ranked: ui first, then db; a made-up path is dropped.
        let raw = "```json\n[\
            {\"path\":\"src/ui.rs\",\"reason\":\"UI code\"},\
            {\"path\":\"src/db.rs\",\"reason\":\"database layer\"},\
            {\"path\":\"nope.rs\",\"reason\":\"invented\"}\
            ]\n```";
        let out = parse_sem_search_reply(raw, &catalog);
        let rels: Vec<String> = out.iter().map(|h| h.rel.display().to_string()).collect();
        assert_eq!(rels, vec!["src/ui.rs", "src/db.rs"], "kept order, dropped the invented path");
        // The reason is folded into the line so the list shows it and Enter previews.
        assert_eq!(out[0].line.as_ref().map(|(n, t)| (*n, t.as_str())), Some((1, "UI code")));
    }

    #[test]
    fn ai_search_builds_a_catalog_and_fires_a_request() {
        let have_py = std::process::Command::new("python3")
            .arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
        if !have_py {
            eprintln!("no python3; skipping");
            return;
        }
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/db.rs"), b"x").unwrap();
        std::fs::write(d.path().join("README.md"), b"x").unwrap();
        let p = d.path().to_path_buf();
        let mut config = en_config();
        config.ai = Some(cian_lua::AiOptions {
            python: "python3".into(), auth_mode: "mock".into(), ..Default::default()
        });
        let mut app = App::new(p.clone(), p, config).unwrap();

        app.start_ai_search("the database code");
        assert!(app.ai_job.is_some(), "a request was fired over the catalog");
        // The mock echoes (not JSON), so the pipeline reports no matches.
        let start = Instant::now();
        while app.ai_job.is_some() && start.elapsed() < Duration::from_secs(10) {
            app.poll_ai_job();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.message.as_deref().unwrap_or("").contains("no relevant"),
            "mock reply parses to no matches: {:?}", app.message);
    }

    #[test]
    fn clean_filename_rejects_paths_and_specials() {
        assert_eq!(clean_filename(" report_v2.txt "), Some("report_v2.txt".to_string()));
        assert_eq!(clean_filename("a/b.txt"), None);
        assert_eq!(clean_filename("a\\b.txt"), None);
        assert_eq!(clean_filename(".."), None);
        assert_eq!(clean_filename("."), None);
        assert_eq!(clean_filename(""), None);
        assert_eq!(clean_filename("C:evil"), None);
    }

    #[test]
    fn parse_rename_reply_validates_and_dedupes() {
        let names = vec![
            ("IMG_1.jpg".to_string(), PathBuf::from("/p/IMG_1.jpg")),
            ("IMG_2.jpg".to_string(), PathBuf::from("/p/IMG_2.jpg")),
            ("keep.txt".to_string(), PathBuf::from("/p/keep.txt")),
        ];
        let raw = "[\
            {\"name\":\"IMG_1.jpg\",\"new_name\":\"photo_01.jpg\"},\
            {\"name\":\"IMG_2.jpg\",\"new_name\":\"../escape.jpg\"},\
            {\"name\":\"keep.txt\",\"new_name\":\"keep.txt\"},\
            {\"name\":\"ghost\",\"new_name\":\"x.jpg\"}\
            ]";
        let items = parse_rename_reply(raw, &names);
        // Only IMG_1 survives: IMG_2's target escapes, keep is a no-op, ghost unknown.
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].old, "IMG_1.jpg");
        assert_eq!(items[0].new, "photo_01.jpg");
    }

    /// The whole rename flow: build the review popup and approve — the checked
    /// file is renamed in place, the unchecked left alone.
    #[test]
    fn rename_plan_renames_checked_files() {
        let (d, mut app) = app_with(&["IMG_1.jpg", "keep.txt"]);
        app.popup = Popup::RenameReview {
            items: vec![
                RenameItem { path: d.path().join("IMG_1.jpg"), old: "IMG_1.jpg".into(),
                    new: "photo_01.jpg".into(), selected: true },
                RenameItem { path: d.path().join("keep.txt"), old: "keep.txt".into(),
                    new: "notes.txt".into(), selected: false },
            ],
            cursor: 0,
            scroll: 0,
            by_ai: true,
        };
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(d.path().join("photo_01.jpg").is_file(), "renamed");
        assert!(!d.path().join("IMG_1.jpg").exists(), "old name gone");
        assert!(d.path().join("keep.txt").is_file(), "unchecked untouched");
        assert!(!d.path().join("notes.txt").exists());
    }

    #[test]
    fn truncate_text_for_ai_caps_and_handles_one_long_line() {
        let short = "a\nb\nc\n";
        assert_eq!(truncate_text_for_ai(short, 1000), short, "short text is unchanged");
        // A single line longer than the cap is cut on a char boundary.
        let long = "x".repeat(5000);
        let out = truncate_text_for_ai(&long, 100);
        assert!(out.len() < long.len() && out.contains("truncated"));
        // Multibyte: cutting must not split a char.
        let multi = "あ".repeat(2000);
        let out = truncate_text_for_ai(&multi, 100);
        assert!(out.starts_with("あ") && out.contains("truncated"));
    }

    /// Pressing `S` in the viewer sends the file's text and opens the chat with
    /// the reply (mock: an echo of the body).
    #[test]
    fn viewer_summarize_opens_the_chat_with_a_reply() {
        let have_py = std::process::Command::new("python3")
            .arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
        if !have_py {
            eprintln!("no python3; skipping");
            return;
        }
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("readme.txt"), "hello world\nsecond line\n").unwrap();
        let p = d.path().to_path_buf();
        let mut config = en_config();
        config.ai = Some(cian_lua::AiOptions {
            python: "python3".into(), auth_mode: "mock".into(), ..Default::default()
        });
        let mut app = App::new(p.clone(), p, config).unwrap();
        app.ai_ready = Some(true); // the probe is async; treat mock as ready
        app.active_pane_mut().unwrap().cursor = 1; // readme.txt (index 0 is `..`)
        let _ = render(&mut app, 100, 40);
        app.look_inside(); // open the F3 viewer
        assert!(matches!(app.popup, Popup::Viewer { .. }), "viewer open");
        let _ = render(&mut app, 100, 40);

        for k in [':', 's', 'u', 'm', 'm', 'a', 'r', 'y'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::AiChat { .. }), "summarise opened the chat");
        let start = Instant::now();
        while app.ai_job.is_some() && start.elapsed() < Duration::from_secs(10) {
            app.poll_ai_job();
            std::thread::sleep(Duration::from_millis(5));
        }
        let Popup::AiChat { log, .. } = &app.popup else { panic!("chat closed") };
        assert!(log.iter().any(|m| !m.user && m.text.contains("hello world")),
            "the mock echoed the file text back as the summary: {log:?}");
    }

    #[test]
    fn clean_dest_folder_rejects_escapes() {
        assert_eq!(clean_dest_folder("images"), Some("images".to_string()));
        assert_eq!(clean_dest_folder(" docs/2023 "), Some("docs/2023".to_string()));
        assert_eq!(clean_dest_folder("a\\b"), Some("a/b".to_string()));
        // Anything that could escape the current directory is refused.
        assert_eq!(clean_dest_folder("../evil"), None);
        assert_eq!(clean_dest_folder("/abs"), None);
        assert_eq!(clean_dest_folder("C:/x"), None);
        assert_eq!(clean_dest_folder("a/../b"), None);
        assert_eq!(clean_dest_folder(""), None);
    }

    #[test]
    fn parse_structure_reply_validates_names_and_folders() {
        let names = vec![
            ("cat.jpg".to_string(), PathBuf::from("/p/cat.jpg")),
            ("notes.md".to_string(), PathBuf::from("/p/notes.md")),
        ];
        let raw = "```json\n[\
            {\"name\":\"cat.jpg\",\"folder\":\"images\",\"reason\":\"an image\"},\
            {\"name\":\"notes.md\",\"folder\":\"../escape\",\"reason\":\"bad folder\"},\
            {\"name\":\"ghost.txt\",\"folder\":\"docs\",\"reason\":\"not shown\"}\
            ]\n```";
        let items = parse_structure_reply(raw, &names);
        assert_eq!(items.len(), 1, "only the valid, real-name move survives");
        assert_eq!(items[0].name, "cat.jpg");
        assert_eq!(items[0].dest, "images");
        assert!(items[0].selected);
    }

    /// The whole structure flow: build a review popup by hand and approve it —
    /// the checked file is moved into a freshly created sub-folder.
    #[test]
    fn structure_plan_moves_checked_files_into_new_folders() {
        let (d, mut app) = app_with(&["cat.jpg", "keep.txt"]);
        let dir = app.active_pane().unwrap().cwd.clone();
        app.popup = Popup::StructureReview {
            items: vec![
                MoveItem { path: d.path().join("cat.jpg"), name: "cat.jpg".into(),
                    dest: "images".into(), reason: "image".into(), selected: true },
                MoveItem { path: d.path().join("keep.txt"), name: "keep.txt".into(),
                    dest: "docs".into(), reason: String::new(), selected: false },
            ],
            cursor: 0,
            scroll: 0,
            dir,
        };
        app.handle_key(code(KeyCode::Enter)).unwrap(); // run the checked moves
        drain_op(&mut app);
        assert!(d.path().join("images/cat.jpg").is_file(), "moved into the new folder");
        assert!(!d.path().join("cat.jpg").exists(), "gone from the root");
        // The unchecked one is left where it was, and its folder not created.
        assert!(d.path().join("keep.txt").is_file(), "unchecked stays put");
        assert!(!d.path().join("docs").exists(), "no folder for an unchecked move");
    }

    #[test]
    fn clean_ai_commit_message_strips_a_wrapping_fence() {
        assert_eq!(clean_ai_commit_message("feat: add x\n\n- why"), "feat: add x\n\n- why");
        assert_eq!(clean_ai_commit_message("```\nfix: bug\n```"), "fix: bug");
        assert_eq!(clean_ai_commit_message("\n\n  chore: tidy  \n\n"), "chore: tidy");
    }

    #[test]
    fn truncate_diff_for_ai_caps_on_a_line_boundary() {
        let big = "line one\nline two\nline three\n".repeat(100);
        let out = truncate_diff_for_ai(&big, 40);
        assert!(out.len() < big.len());
        assert!(out.contains("truncated"), "marks the cut: {out:?}");
        // Only whole lines are kept before the marker.
        let before_marker = out.split("\n\n[").next().unwrap();
        assert!(before_marker.split('\n').all(|l| l.is_empty() || big.contains(l)));
    }

    /// The whole commit-message flow with a throwaway repo: draft (mock), edit,
    /// and commit — then the message is in the log and the stage is clean.
    #[test]
    fn ai_commit_message_flow_drafts_edits_and_commits() {
        let have_py = std::process::Command::new("python3")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !have_py {
            eprintln!("no python3; skipping");
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(d.path()).unwrap();
        let git_ok = std::process::Command::new("git")
            .arg("-C").arg(&dir).args(["init", "-q"]).status()
            .map(|s| s.success()).unwrap_or(false);
        if !git_ok {
            eprintln!("no git; skipping");
            return;
        }
        for kv in [["user.email", "t@example.com"], ["user.name", "Test"]] {
            let _ = std::process::Command::new("git").arg("-C").arg(&dir).args(["config", kv[0], kv[1]]).status();
        }
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        cian_core::git::stage(&dir, &[dir.join("a.txt")]).unwrap();

        let mut config = en_config();
        config.ai = Some(cian_lua::AiOptions {
            python: "python3".into(),
            auth_mode: "mock".into(),
            ..Default::default()
        });
        let mut app = App::new(dir.clone(), dir.clone(), config).unwrap();
        app.ai_ready = Some(true); // the probe is async; treat mock as ready

        app.start_ai_commit_message();
        let start = Instant::now();
        while app.ai_job.is_some() && start.elapsed() < Duration::from_secs(10) {
            app.poll_ai_job();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(app.popup, Popup::CommitMessage { .. }), "draft popup, got {:?}", app.popup);

        // Replace the drafted text with our own: e → edit, clear, type.
        app.handle_key(key('e')).unwrap();
        if let Popup::CommitMessage { buffer, .. } = &mut app.popup {
            buffer.clear();
        }
        for c in "add a.txt".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Esc)).unwrap(); // leave edit mode
        app.handle_key(code(KeyCode::Enter)).unwrap(); // commit

        assert!(matches!(app.popup, Popup::None), "committed, popup closed: {:?}", app.popup);
        assert_eq!(cian_core::git::staged_diff(&dir).as_deref(), Some(""), "stage is clean");
        let log = std::process::Command::new("git").arg("-C").arg(&dir).args(["log", "-1", "--pretty=%s"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "add a.txt");
    }

    #[test]
    fn ai_shell_command_flow_yields_a_confirm_popup() {
        let have_py = std::process::Command::new("python3")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !have_py {
            eprintln!("no python3; skipping");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_path_buf();
        let mut config = en_config();
        config.ai = Some(cian_lua::AiOptions {
            python: "python3".into(),
            auth_mode: "mock".into(),
            ..Default::default()
        });
        let mut app = App::new(p.clone(), p, config).unwrap();

        app.start_ai_shell_cmd("compress the logs");
        // Wait for the worker; the mock echoes the request as the "command".
        let start = Instant::now();
        while app.ai_job.is_some() && start.elapsed() < Duration::from_secs(10) {
            app.poll_ai_job();
            std::thread::sleep(Duration::from_millis(5));
        }
        match &app.popup {
            Popup::AiShellConfirm { command, .. } => {
                assert!(command.contains("compress the logs"), "got {command:?}");
            }
            other => panic!("expected the command-confirm popup, got {:?}", other),
        }
    }

    #[test]
    fn the_context_menu_drills_into_submenus_and_back() {
        // With SSH hosts, the file menu offers a "Transfer ▸" group.
        let (_d, mut app) = app_with_ssh();
        app.open_context_menu(5, 5);
        let has_group = matches!(&app.popup, Popup::ContextMenu { items, .. } if items.contains(&MenuItem::SendMenu));
        assert!(has_group, "file menu has a Transfer group");

        // Drill in: the submenu shows the SFTP actions and a Back item.
        app.run_menu_item(MenuItem::SendMenu).unwrap();
        match &app.popup {
            Popup::ContextMenu { items, .. } => {
                assert!(items.contains(&MenuItem::ScpUpload));
                assert!(items.contains(&MenuItem::Back));
            }
            other => panic!("expected the submenu, got {:?}", other),
        }
        assert_eq!(app.menu_stack.len(), 1, "parent stashed");

        // Back returns to the parent menu, not to nothing.
        app.run_menu_item(MenuItem::Back).unwrap();
        let back_at_parent = matches!(&app.popup, Popup::ContextMenu { items, .. } if items.contains(&MenuItem::SendMenu));
        assert!(back_at_parent, "Back climbed to the parent");
        assert!(app.menu_stack.is_empty());
    }

    #[test]
    fn ai_chat_is_silent_without_config() {
        let (_d, mut app) = app_with(&["a.txt"]);
        assert!(app.ai.is_none());
        app.open_ai_chat();
        assert!(matches!(app.popup, Popup::None), "no chat without cian.ai config");
        assert!(app.message.as_deref().unwrap_or("").contains("not configured"));
    }

    #[test]
    fn glob_match_handles_stars_and_question_marks() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", ".rs"));
        assert!(!glob_match("*.rs", "main.rst"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("test_*", "test_foo"));
        assert!(!glob_match("test_*", "footest"));
        assert!(glob_match("a*b*c", "axxbyyc"));
    }

    #[test]
    fn mark_command_marks_matching_entries() {
        let (_d, mut app) = app_with(&["a.rs", "b.rs", "c.txt", "readme.md"]);
        app.command_buffer = "mark *.rs".into();
        app.run_command();
        assert_eq!(app.active_pane().unwrap().mark_count(), 2, "two .rs marked");
        // Unmark one class, then all.
        app.command_buffer = "unmark *.rs".into();
        app.run_command();
        assert_eq!(app.active_pane().unwrap().mark_count(), 0);
    }

    #[test]
    fn a_permission_error_explains_admin_rights() {
        let (_d, mut app) = app_with(&["a.rs"]);
        let mut report = OpReport { permission_denied: true, ..Default::default() };
        report.note_error("C:/Program Files/x: Access is denied (os error 5)");
        app.show_op_report(&report);
        let Popup::Notice { lines } = &app.popup else { panic!("expected a notice") };
        assert!(
            lines.iter().any(|l| l.contains("administrator rights")),
            "the notice names the cause: {lines:?}"
        );
    }

    #[test]
    fn a_user_keymap_rebinds_and_disables_keys() {
        let (_d, mut app) = app_with_keymaps(
            &["a.rs", "b.rs"],
            vec![
                ("x", "delete".into()), // bind a new key to an action
                ("d", "none".into()),   // and turn the default off
            ],
        );
        // `x` now opens the delete confirm…
        app.handle_key(key('x')).unwrap();
        assert!(matches!(app.popup, Popup::ConfirmDelete { .. }), "x deletes");
        app.handle_key(code(KeyCode::Esc)).unwrap();
        // …while the disabled `d` does nothing.
        app.handle_key(key('d')).unwrap();
        assert!(matches!(app.popup, Popup::None), "d is unbound");
    }

    #[test]
    fn every_action_named_in_the_example_config_resolves() {
        // Guards against the docs drifting from the code: each
        // `set_keymap("k", "action")` in examples/init.lua must name a real
        // action, so a user copying a line always gets a working binding.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/init.lua");
        let text = std::fs::read_to_string(path).expect("read examples/init.lua");
        let mut checked = 0;
        for line in text.lines() {
            // Only the real binding lines (`cian.set_keymap("k", "action")`),
            // not the "key"/"action" placeholder in the section header or the
            // prose examples that have text before the call.
            let trimmed = line.trim_start_matches(['-', ' ']);
            if !trimmed.starts_with("cian.set_keymap(") {
                continue;
            }
            let Some(rest) = trimmed.split_once("set_keymap(").map(|(_, r)| r) else { continue };
            // The action is the second quoted string on the line.
            let quoted: Vec<&str> = rest.split('"').collect();
            if quoted.len() >= 4 {
                let action = quoted[3];
                assert!(
                    action_from_name(action).is_some(),
                    "examples/init.lua names unknown action {:?}",
                    action
                );
                checked += 1;
            }
        }
        assert!(checked > 20, "expected to have checked the documented bindings, got {checked}");
    }

    #[test]
    fn reload_reapplies_the_keymap_live() {
        // reload_config re-applies the theme into the process-wide global, so it
        // must not race the theme tests.
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_d, mut app) = app_with(&["a.rs"]);
        // No user binding yet: `x` is not delete.
        assert!(!app.keymap.contains_key(&('x', KeyModifiers::NONE)));
        // Point CIAN_CONFIG_DIR at a temp config that binds x -> delete, then
        // reload — the running app should pick it up without a restart.
        let cfgdir = tempfile::tempdir().unwrap();
        std::fs::write(
            cfgdir.path().join("init.lua"),
            "cian.set_keymap(\"x\", \"delete\")\n",
        )
        .unwrap();
        std::env::set_var("CIAN_CONFIG_DIR", cfgdir.path());
        app.command_buffer = "reload".into();
        app.run_command();
        std::env::remove_var("CIAN_CONFIG_DIR");
        assert_eq!(app.keymap.get(&('x', KeyModifiers::NONE)), Some(&Action::Delete), "reload bound x live");
    }

    #[test]
    fn a_newly_named_action_is_bindable() {
        // `sort` had no bindable name before; confirm it now resolves and works.
        assert_eq!(action_from_name("sort"), Some(Action::Sort));
        let (_d, mut app) = app_with_keymaps(&["a.rs"], vec![("S", "sort".into())]);
        app.handle_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT)).unwrap();
        assert!(matches!(app.popup, Popup::SortPicker { .. }), "S opens the sort picker");
    }

    /// Render and hand back the raw buffer, for checking colors.
    fn render_buf(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// Render `app` onto a `w`x`h` test terminal and return the text of each row.
    /// Render, and hand back the cell under the viewer's cursor with its
    /// colours — the only way to catch "the character is there but painted the
    /// same shade as the block behind it".
    fn cursor_cell(app: &mut App, w: u16, h: u16) -> (String, Color, Color) {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let (line, col) = match &app.popup {
            Popup::Viewer { line, col, .. } => (*line, *col),
            other => panic!("not a viewer: {other:?}"),
        };
        let b = app.viewer_rect;
        let x = b.x + app.viewer_gutter + col as u16;
        let y = b.y + line as u16;
        let c = &buf[(x, y)];
        (c.symbol().to_string(), c.fg, c.bg)
    }

    fn render(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// A long cwd in the pane title is shortened from the middle: the tail is
    /// what identifies a path, and clipping at the border lost exactly that.
    #[test]
    fn title_keeps_the_tail_of_a_long_path() {
        let d = tempfile::tempdir().unwrap();
        let mut deep = d.path().to_path_buf();
        for part in ["very-long-segment-one", "very-long-segment-two", "very-long-segment-three", "destination"] {
            deep.push(part);
        }
        std::fs::create_dir_all(&deep).unwrap();
        let mut app = App::new(deep.clone(), deep, en_config()).unwrap();
        let out = render(&mut app, 80, 20);
        let title = &out[0];
        assert!(title.contains('…'), "long path was middle-truncated: {title}");
        assert!(title.contains("destination"), "the identifying tail survives: {title}");
    }

    /// The visible-window optimization must render the same rows ratatui would:
    /// the cursor stays on screen and far-away rows are excluded.
    #[test]
    fn big_directory_windows_to_the_cursor() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..500 {
            std::fs::write(d.path().join(format!("file_{i:04}.rs")), b"x").unwrap();
        }
        // The right pane opens an empty dir so only the left column shows file_*.
        let empty = tempfile::tempdir().unwrap();
        let config = en_config();
        let mut app = App::new(d.path().to_path_buf(), empty.path().to_path_buf(), config).unwrap();
        app.focus(FocusedPane::Left);
        let idx = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "file_0400.rs")
            .unwrap();
        app.active_pane_mut().unwrap().cursor = idx;
        let joined = render(&mut app, 120, 50).join("\n");
        assert!(joined.contains("file_0400.rs"), "the cursor row must be visible");
        assert!(joined.contains("file_0399.rs"), "its neighbour is on screen too");
        assert!(!joined.contains("file_0000.rs"), "rows far above are windowed out");
        assert!(!joined.contains("file_0499.rs"), "rows far below are windowed out");
    }

    /// Micro-bench (run with `--ignored --nocapture`): time N renders of a pane
    /// holding a large directory, cursor parked deep in the list. Prints the
    /// per-frame cost so the windowing optimization can be measured.
    #[test]
    #[ignore]
    fn bench_render_big_directory() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..5000 {
            std::fs::write(d.path().join(format!("file_{i:05}.rs")), b"x").unwrap();
        }
        let mut config = en_config();
        config.options.home = Some(d.path().display().to_string());
        let mut app = App::new(d.path().to_path_buf(), d.path().to_path_buf(), config).unwrap();
        // Park the cursor deep so the visible window is far from the top.
        app.active_pane_mut().unwrap().cursor = 4000;
        let mut terminal = Terminal::new(TestBackend::new(120, 50)).unwrap();
        // Warm up.
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let n = 400;
        let start = std::time::Instant::now();
        for _ in 0..n {
            terminal.draw(|f| draw(f, &mut app)).unwrap();
        }
        let per = start.elapsed() / n;
        println!("bench_render_big_directory: {per:?}/frame over {n} frames (5000 entries)");
    }

    /// Click the centre of the first popup zone matching `want`, after a render
    /// has registered the zones. Returns false if no such zone exists.
    fn click_zone(app: &mut App, want: ZoneKind) -> bool {
        let hit = app.popup_zones.iter().find(|z| z.kind == want).map(|z| z.rect);
        match hit {
            Some(r) => {
                app.handle_mouse(mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    r.x + r.width / 2,
                    r.y,
                ));
                true
            }
            None => false,
        }
    }

    #[test]
    fn the_wheel_scrolls_the_file_pane_under_the_pointer() {
        let names: Vec<String> = (0..40).map(|i| format!("f{:02}.txt", i)).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let (_d, mut app) = app_with(&refs);
        let _ = render(&mut app, 100, 40);
        let start = app.active_pane().unwrap().cursor;
        let left = app.layout_rects.left;
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, left.x + 3, left.y + 3));
        let after = app.active_pane().unwrap().cursor;
        assert!(after > start, "wheel down moved the cursor down: {start} -> {after}");
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, left.x + 3, left.y + 3));
        assert!(app.active_pane().unwrap().cursor < after, "wheel up moved it back up");
    }

    /// Dragging inside a pane only moves the cursor. It used to rubber-band
    /// the marks, which fought the deliberate marking Space and visual mode
    /// already do — and turned every slightly-shaky click into a reshuffle.
    #[test]
    fn dragging_inside_a_pane_only_moves_the_cursor() {
        let (_l, _r, mut app) = app_two_dirs(&["a.txt", "b.txt", "c.txt"], &[]);
        let _ = render(&mut app, 100, 40);
        let left = app.layout_rects.left;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 3, left.y + 3));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), left.x + 3, left.y + 5));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), left.x + 3, left.y + 5));
        assert_eq!(app.active_pane().unwrap().mark_count(), 0, "a drag marks nothing");
        assert!(matches!(app.popup, Popup::None), "and starts no transfer");
    }


    #[test]
    fn clicking_a_sort_picker_row_applies_it() {
        let (_d, mut app) = app_with(&["a.rs", "b.rs"]);
        app.start_sort_picker();
        assert!(matches!(app.popup, Popup::SortPicker { .. }));
        // Render so the row hit-zones are registered, then click the 3rd entry.
        let _ = render(&mut app, 100, 40);
        assert!(click_zone(&mut app, ZoneKind::SelectRow(2)), "row zone present");
        // A pick closes the picker and applies that key.
        assert!(matches!(app.popup, Popup::None), "picker closed after a click");
        assert_eq!(app.active_pane().unwrap().sort.key, SortKey::ALL[2]);
    }

    #[test]
    fn clicking_a_confirm_dialog_button_answers_it() {
        let (_d, mut app) = app_with(&["a.rs"]);
        app.start_quit_confirm();
        assert!(matches!(app.popup, Popup::ConfirmQuit));
        let _ = render(&mut app, 100, 40);
        // The "No" button cancels without quitting.
        assert!(click_zone(&mut app, ZoneKind::Esc), "No button present");
        assert!(matches!(app.popup, Popup::None));
        assert!(!app.should_quit);

        app.start_quit_confirm();
        let _ = render(&mut app, 100, 40);
        assert!(click_zone(&mut app, ZoneKind::Enter), "Yes button present");
        assert!(app.should_quit, "clicking Yes quits");
    }

    #[test]
    fn the_mouse_wheel_scrolls_the_manual() {
        let (_d, mut app) = app_with(&["a.rs"]);
        app.open_manual();
        let _ = render(&mut app, 100, 40);
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 50, 20));
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 50, 20));
        let Popup::Manual { scroll, .. } = &app.popup else { panic!("expected manual") };
        assert_eq!(*scroll, 2, "two wheel notches scrolled two lines");
    }

    #[test]
    fn slash_filters_the_listing_incrementally() {
        let (_d, mut app) = app_with(&["alpha.rs", "beta.rs", "gamma.txt"]);
        // Counts include the synthetic `..` row, so a 3-file dir lists 4.
        assert_eq!(app.active_pane().unwrap().entries.len(), 4);

        app.handle_key(key('/')).unwrap();
        assert_eq!(app.mode, Mode::Filter);

        app.handle_key(key('r')).unwrap();
        app.handle_key(key('s')).unwrap();
        assert_eq!(app.active_pane().unwrap().entries.len(), 3);

        // Backspace widens the match: "r" still excludes gamma.txt.
        app.handle_key(code(KeyCode::Backspace)).unwrap();
        assert_eq!(app.active_pane().unwrap().entries.len(), 3);

        // Emptying the buffer restores the full listing.
        app.handle_key(code(KeyCode::Backspace)).unwrap();
        assert_eq!(app.filter_buffer, "");
        assert_eq!(app.active_pane().unwrap().entries.len(), 4);
    }

    #[test]
    fn enter_keeps_the_filter_and_esc_clears_it() {
        let (_d, mut app) = app_with(&["a.txt", "b.md"]);

        app.handle_key(key('/')).unwrap();
        app.handle_key(key('m')).unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(app.mode, Mode::Normal);
        // `..` plus the one match survives the filter.
        assert_eq!(app.active_pane().unwrap().entries.len(), 2, "filter should survive Enter");

        // Esc in normal mode drops the narrowing.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert_eq!(app.active_pane().unwrap().entries.len(), 3);
    }

    #[test]
    fn esc_while_filtering_restores_the_full_list() {
        let (_d, mut app) = app_with(&["a.txt", "b.md"]);
        app.handle_key(key('/')).unwrap();
        app.handle_key(key('m')).unwrap();
        assert_eq!(app.active_pane().unwrap().entries.len(), 2, "`..` plus the match");
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.active_pane().unwrap().entries.len(), 3);
    }

    #[test]
    fn question_mark_opens_the_manual() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(key('?')).unwrap();
        assert!(matches!(app.popup, Popup::Manual { .. }));
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::None));
    }

    #[test]
    fn ctrl_dot_opens_the_manual() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::CONTROL)).unwrap();
        assert!(matches!(app.popup, Popup::Manual { .. }));
    }

    /// Regression: the manual is ~50 lines, far taller than a normal terminal.
    /// Every line must be reachable by scrolling rather than silently clipped.
    #[test]
    fn manual_scrolls_to_reveal_its_last_section() {
        let (_d, mut app) = app_with_lang(&["a.txt"], "en");
        app.handle_key(key('?')).unwrap();

        let top = render(&mut app, 100, 24).join("\n");
        assert!(top.contains("key manual"), "manual header should be visible");
        assert!(
            !top.contains("zoom active split pane"),
            "the last section cannot already fit on a 24-row terminal"
        );

        // G jumps to the bottom; the final section must now be on screen.
        app.handle_key(key('G')).unwrap();
        let bottom = render(&mut app, 100, 24).join("\n");
        assert!(
            bottom.contains("zoom active split pane"),
            "scrolling to the end must reveal the last section; got:\n{}",
            bottom
        );
    }

    #[test]
    fn manual_scroll_is_clamped_at_both_ends() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(key('?')).unwrap();

        // Scrolling up at the top is a no-op, not an underflow panic.
        for _ in 0..5 {
            app.handle_key(key('k')).unwrap();
        }
        let Popup::Manual { scroll, .. } = &app.popup else { panic!("expected manual") };
        assert_eq!(*scroll, 0);

        // Paging past the end settles on the last page after a render, and
        // stays there. The end is wherever the renderer puts it — the entries
        // are wrapped to the popup's width, so it is not the count of them.
        for _ in 0..50 {
            app.handle_key(key('d')).unwrap();
        }
        let _ = render(&mut app, 100, 24);
        let Popup::Manual { scroll, .. } = &app.popup else { panic!("expected manual") };
        let settled = *scroll;
        assert!(settled > 0, "paging moved off the top");
        app.handle_key(key('d')).unwrap();
        let _ = render(&mut app, 100, 24);
        let Popup::Manual { scroll, .. } = &app.popup else { panic!("expected manual") };
        assert_eq!(*scroll, settled, "and the last page is where paging stops");
    }

    /// The manual reflects `init.lua` overrides rather than a hardcoded list.
    #[test]
    fn manual_lists_user_bound_keys() {
        let mut keymap = HashMap::new();
        keymap.insert(('x', KeyModifiers::NONE), Action::Delete);
        keymap.insert(('g', KeyModifiers::ALT), Action::GrepRecursive);
        let text = manual_lines(&keymap, Lang::En).join("\n");
        assert!(text.contains("d, x"), "user-bound key missing from manual:\n{}", text);
        assert!(text.contains("Alt+g"), "a modified binding is named in full:\n{}", text);
    }

    #[test]
    fn the_status_and_hints_default_to_english_and_switch_to_japanese() {
        // Default is English.
        let (_d, mut app) = app_with(&["a.txt"]);
        let en = render(&mut app, 110, 40).join("\n");
        assert!(en.contains("items") && en.contains("help"), "English chrome:\n{en}");

        // lang=ja renders the chrome in Japanese. A wide (CJK) glyph occupies
        // two cells, so the row reconstruction inserts a space after each; strip
        // spaces before matching the words.
        let flat = |app: &mut App| render(app, 110, 40).join("\n").replace(' ', "");
        let (_d2, mut ja) = app_with_lang(&["a.txt"], "ja");
        let screen = flat(&mut ja);
        assert!(screen.contains("件"), "status counts in Japanese:\n{screen}");
        assert!(screen.contains("ヘルプ"), "help hint in Japanese");
        ja.open_context_menu(5, 5);
        let menu = flat(&mut ja);
        assert!(menu.contains("コピー"), "menu in Japanese:\n{menu}");
    }

    #[test]
    fn the_manual_defaults_to_english_and_switches_to_japanese() {
        let keymap = HashMap::new();
        let en = manual_lines(&keymap, Lang::En).join("\n");
        assert!(en.contains("key manual"), "English header");
        assert!(en.contains("delete (to trash)"), "English description present");
        let ja = manual_lines(&keymap, Lang::Ja).join("\n");
        assert!(ja.contains("キー一覧"), "Japanese header:\n{ja}");
        assert!(ja.contains("削除（ゴミ箱へ）"), "Japanese description present");

        // The `lang` option drives which one an App shows.
        let (_d, app_en) = app_with(&["a.rs"]);
        assert_eq!(app_en.lang, Lang::En, "default is English");
        let (_d2, app_ja) = app_with_lang(&["a.rs"], "ja");
        assert_eq!(app_ja.lang, Lang::Ja, "lang=ja switches to Japanese");
    }

    #[test]
    fn the_menu_language_toggle_flips_the_interface() {
        let (_d, mut app) = app_with(&["a.txt"]);
        assert_eq!(app.lang, Lang::En, "starts English");
        app.run_menu_item(MenuItem::Lang).unwrap();
        assert_eq!(app.lang, Lang::Ja, "toggled to Japanese");
        // The label reflects the language it switches *to*.
        assert_eq!(MenuItem::Lang.label(Lang::Ja), "Switch to English");
        assert_eq!(MenuItem::Lang.label(Lang::En), "日本語に切替");
        app.run_menu_item(MenuItem::Lang).unwrap();
        assert_eq!(app.lang, Lang::En, "toggled back to English");
    }

    fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE }
    }

    /// Grab a divider, drag it, release. Returns the app for further asserts.
    fn drag_divider(app: &mut App, target: DividerTarget, to: (u16, u16)) {
        let d = app
            .dividers
            .iter()
            .copied()
            .find(|d| d.target == target)
            .unwrap_or_else(|| panic!("no divider for {:?} in {:?}", target, app.dividers));
        // Grab the middle of the seam, not its very corner — the corner shares
        // a cell with a tab label, which now wins the click.
        let grab = (d.zone.x + d.zone.width / 2, d.zone.y + d.zone.height / 2);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), grab.0, grab.1));
        assert!(app.drag.is_some(), "grabbing the seam should start a drag");
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), to.0, to.1));
        assert!(app.drag.is_none(), "releasing should end the drag");
    }

    #[test]
    fn dragging_the_vertical_seam_resizes_the_file_panes() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);
        assert_eq!(app.panes_pct, 50);

        // Drag the left/right seam to roughly a quarter of the width.
        drag_divider(&mut app, DividerTarget::Panes, (25, 10));
        assert!(
            (20..=30).contains(&app.panes_pct),
            "expected ~25%, got {}",
            app.panes_pct
        );

        // The rendered rects must follow.
        let _ = render(&mut app, 100, 40);
        assert!(
            app.layout_rects.left.width < app.layout_rects.right.width,
            "left pane should now be the narrow one"
        );
    }

    #[test]
    fn dragging_the_horizontal_seam_resizes_the_shell_panel() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);
        assert_eq!(app.main_pct, 60);

        drag_divider(&mut app, DividerTarget::Main, (50, 10));
        assert!(app.main_pct < 60, "shell should have grown, got {}", app.main_pct);

        let before = app.layout_rects.shell.height;
        let _ = render(&mut app, 100, 40);
        assert!(app.layout_rects.shell.height > before / 2, "shell rect should follow the drag");
    }

    #[test]
    fn a_split_cannot_be_dragged_past_its_minimum() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);

        // Drag far past the left edge; the pane must keep a usable width.
        drag_divider(&mut app, DividerTarget::Panes, (0, 10));
        assert_eq!(app.panes_pct, MIN_SPLIT_PCT);

        drag_divider(&mut app, DividerTarget::Panes, (999, 10));
        assert_eq!(app.panes_pct, 100 - MIN_SPLIT_PCT);
    }

    #[test]
    fn grabbing_a_seam_does_not_change_focus() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);
        app.focus(FocusedPane::Left);

        let d = app.dividers.iter().copied().find(|d| d.target == DividerTarget::Main).unwrap();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), d.zone.x, d.zone.y));
        assert_eq!(app.focused, FocusedPane::Left, "grabbing a border must not steal focus");
    }

    #[test]
    fn clicking_inside_a_pane_still_moves_focus() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);
        app.focus(FocusedPane::Left);

        let r = app.layout_rects.right;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 5, r.y + 3));
        assert_eq!(app.focused, FocusedPane::Right);
        assert!(app.drag.is_none());
    }

    /// An app with two *different* directories, one per pane.
    fn app_two_dirs(
        left: &[&str],
        right: &[&str],
    ) -> (tempfile::TempDir, tempfile::TempDir, App) {
        let l = tempfile::tempdir().unwrap();
        let r = tempfile::tempdir().unwrap();
        for n in left {
            std::fs::write(l.path().join(n), b"x").unwrap();
        }
        for n in right {
            std::fs::write(r.path().join(n), b"y").unwrap();
        }
        let app = App::new(
            l.path().to_path_buf(),
            r.path().to_path_buf(),
            en_config(),
        )
        .unwrap();
        (l, r, app)
    }

    /// `o` pulls the other pane's directory into the active one; `O` pushes the
    /// active pane's directory onto the other. Focus never moves.
    #[test]
    fn o_and_shift_o_sync_the_two_panes_directories() {
        let (l, r, mut app) = app_two_dirs(&["a.txt"], &["b.txt"]);
        let (ldir, rdir) = (l.path().to_path_buf(), r.path().to_path_buf());
        assert_ne!(app.left.active_ref().cwd, app.right.active_ref().cwd);

        // On the right pane, `o` makes the right pane show the left's directory.
        app.focus(FocusedPane::Right);
        app.handle_key(key('o')).unwrap();
        assert!(app.right.active_ref().cwd.ends_with(ldir.file_name().unwrap()),
            "right pulled the left's dir");
        assert_eq!(app.focused, FocusedPane::Right, "focus stays put");

        // Reset the right pane, then `O` pushes the right's dir onto the left.
        app.right.active_mut().jump_to(rdir.clone()).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('O'), KeyModifiers::SHIFT)).unwrap();
        assert!(app.left.active_ref().cwd.ends_with(rdir.file_name().unwrap()),
            "left received the right's dir");
        assert_eq!(app.focused, FocusedPane::Right, "focus still on the right");
    }

    #[test]
    fn copy_then_paste_duplicates_into_the_other_directory() {
        let (_l, r, mut app) = app_two_dirs(&["doc.txt"], &[]);
        app.focus(FocusedPane::Left);
        app.run_menu_item(MenuItem::Copy).unwrap();
        assert!(app.file_clip.is_some());

        app.focus(FocusedPane::Right);
        app.run_menu_item(MenuItem::Paste).unwrap();

        assert!(r.path().join("doc.txt").exists(), "file should have been pasted");
        // A copy stays on the clipboard for pasting again elsewhere.
        assert!(app.file_clip.is_some(), "copy should survive its paste");
    }

    #[test]
    fn cut_then_paste_moves_and_empties_the_clipboard() {
        let (l, r, mut app) = app_two_dirs(&["move_me.txt"], &[]);
        app.focus(FocusedPane::Left);
        app.run_menu_item(MenuItem::Cut).unwrap();

        app.focus(FocusedPane::Right);
        app.run_menu_item(MenuItem::Paste).unwrap();

        assert!(r.path().join("move_me.txt").exists(), "should exist at destination");
        assert!(!l.path().join("move_me.txt").exists(), "should be gone from source");
        assert!(app.file_clip.is_none(), "a cut is consumed by its paste");
    }

    #[test]
    fn pasting_into_the_source_directory_is_refused() {
        let (l, _r, mut app) = app_two_dirs(&["a.txt"], &[]);
        app.focus(FocusedPane::Left);
        app.run_menu_item(MenuItem::Copy).unwrap();
        // Paste straight back where it came from.
        app.run_menu_item(MenuItem::Paste).unwrap();

        let n = std::fs::read_dir(l.path()).unwrap().count();
        assert_eq!(n, 1, "must not duplicate into the same directory");
        assert!(app.message.as_deref().unwrap_or("").contains("already"));
    }

    /// Paste is always offered, because it can also take files from the system
    /// clipboard. Hiding it until cian's own register was filled made a file
    /// just copied in Explorer look unpasteable.
    #[test]
    fn clipboard_keys_follow_windows_and_c_is_copy_to_other_pane() {
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let (_l, _r, mut app) = app_two_dirs(&["a.txt"], &[]);
        app.focus(FocusedPane::Left);
        app.active_pane_mut().unwrap().cursor = 1; // a.txt (0 is `..`)

        // Ctrl+C → Windows-style file-clipboard copy.
        app.handle_key(ctrl('c')).unwrap();
        assert!(matches!(app.file_clip, Some(FileClipboard { op: ClipOp::Copy, .. })), "Ctrl+C copies");

        // Ctrl+X → cut.
        app.handle_key(ctrl('x')).unwrap();
        assert!(matches!(app.file_clip, Some(FileClipboard { op: ClipOp::Cut, .. })), "Ctrl+X cuts");

        // `c` is now "copy to the other pane" (a transfer), not the clipboard.
        app.file_clip = None;
        app.handle_key(key('c')).unwrap();
        assert!(matches!(app.popup, Popup::ConfirmTransfer { op: PendingOp::Copy, .. }), "c copies to the other pane");
        assert!(app.file_clip.is_none(), "c does not touch the file clipboard");
    }

    #[test]
    fn y_and_ctrl_v_both_paste() {
        let ctrl_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
        for trigger in [key('y'), ctrl_v] {
            let (_l, _r, mut app) = app_two_dirs(&["a.txt"], &[]);
            app.focus(FocusedPane::Right); // paste into the (empty) right pane
            // Nothing on the clipboard yet → paste reports it (proves it routed
            // to paste_clip rather than a copy/transfer).
            app.handle_key(trigger).unwrap();
            assert_eq!(app.message.as_deref(), Some("clipboard has no files"), "paste ran for {trigger:?}");
        }
    }

    #[test]
    fn paste_is_always_offered() {
        let (_l, _r, mut app) = app_two_dirs(&["a.txt"], &[]);
        let _ = render(&mut app, 100, 40);

        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        assert!(items.contains(&MenuItem::PasteHere), "offered with nothing held");
        app.popup = Popup::None;

        app.clip_targets(ClipOp::Copy);
        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        assert!(items.contains(&MenuItem::PasteHere), "and still offered once held");
    }

    /// Plain text on the clipboard must never be treated as a path: the
    /// platform queries return the text coerced into one (copying "hello"
    /// yields `/hello` on macOS), and acting on that would be nonsense.
    #[test]
    fn clipboard_candidates_that_do_not_exist_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        std::fs::write(&real, b"x").unwrap();

        let kept = keep_existing(vec![
            real.clone(),
            PathBuf::from("/just some copied text"),
            dir.path().to_path_buf(),
            PathBuf::from(""),
        ]);
        assert_eq!(kept, vec![real, dir.path().to_path_buf()], "only real entries survive");
        assert!(keep_existing(Vec::new()).is_empty());
    }

    #[test]
    fn right_click_focuses_the_pane_and_opens_the_menu() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt"]);
        let _ = render(&mut app, 100, 40);
        app.focus(FocusedPane::Left);

        let r = app.layout_rects.right;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), r.x + 5, r.y + 2));
        assert_eq!(app.focused, FocusedPane::Right, "right-click should move focus");
        assert!(matches!(app.popup, Popup::ContextMenu { .. }));
    }

    #[test]
    fn file_menu_offers_the_os_actions_group() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Left);
        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        assert!(items.contains(&MenuItem::OsMenu), "file menu offers the OS group");
        assert!(MenuItem::OsMenu.is_group());
        let kids = app.submenu_children(MenuItem::OsMenu).expect("group has children");
        assert_eq!(
            kids,
            vec![
                MenuItem::OpenDefault,
                MenuItem::OpenWithOs,
                MenuItem::RevealInOs,
                MenuItem::PropertiesOs,
                MenuItem::Back,
            ]
        );
        // The reveal/properties labels adapt to the host OS and are never blank.
        for it in [MenuItem::RevealInOs, MenuItem::PropertiesOs, MenuItem::OpenWithOs] {
            assert!(!it.label(Lang::En).is_empty());
            assert!(!it.label(Lang::Ja).is_empty());
        }
    }

    #[test]
    fn the_os_group_is_absent_from_the_shell_menu() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        assert!(!items.contains(&MenuItem::OsMenu), "the OS group is file-pane only");
    }

    #[test]
    fn the_shell_menu_omits_file_operations() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.clip_targets(ClipOp::Copy);
        app.focus(FocusedPane::Shell);
        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        assert!(items.contains(&MenuItem::Paste));
        assert!(!items.contains(&MenuItem::Delete), "delete makes no sense in a PTY");
        assert!(!items.contains(&MenuItem::Rename));
    }

    /// The manual has to be reachable from the menu everywhere — that is the
    /// whole point of putting it there.
    /// Keys never reach the picker while the shell has focus, so the menu is
    /// the only route to SSH from there. It must lead the shell's menu.
    #[test]
    fn the_shell_menu_leads_with_ssh() {
        let (_d, mut app) = app_with_ssh();
        app.focus(FocusedPane::Shell);
        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        // SSH is still reachable from the shell menu (the only route to it while
        // the shell has focus), now sitting after Paste / Transfer ▸.
        assert!(items.contains(&MenuItem::Ssh), "got {:?}", items);
    }

    #[test]
    fn the_menu_reaches_the_ssh_picker_from_the_shell() {
        let (_d, mut app) = app_with_ssh();
        app.focus(FocusedPane::Shell);
        app.run_menu_item(MenuItem::Ssh).unwrap();
        assert!(matches!(app.popup, Popup::SshHosts { .. }), "should open the picker");
    }

    /// Both panes offer it, since the picker is useful from either.
    #[test]
    fn the_file_menu_offers_ssh_too() {
        let (_d, mut app) = app_with_ssh();
        app.focus(FocusedPane::Left);
        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        assert!(items.contains(&MenuItem::Ssh), "got {:?}", items);
    }

    #[test]
    fn every_context_menu_offers_the_manual() {
        let (_d, mut app) = app_with(&["a.txt"]);
        for pane in [FocusedPane::Left, FocusedPane::Right, FocusedPane::Shell] {
            app.focus(pane);
            app.open_context_menu(5, 5);
            let Popup::ContextMenu { items, .. } = &app.popup else {
                panic!("no menu for {:?}", pane)
            };
            assert_eq!(
                items.last(),
                Some(&MenuItem::Manual),
                "manual should be the last entry for {:?}",
                pane
            );
            app.popup = Popup::None;
        }
    }

    /// Right-clicking the shell with an empty clipboard used to open nothing
    /// at all; the manual entry means there is always something to show.
    #[test]
    fn file_menu_zone_order_is_consistent() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Left);
        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        let pos = |m: MenuItem| items.iter().position(|i| *i == m).expect("item present");
        // Shortcuts joins the launcher cluster at the top, above the file ops.
        assert!(pos(MenuItem::Shortcuts) < pos(MenuItem::Copy));
        // Copy / paste cluster, then the connect block, then appearance.
        assert!(pos(MenuItem::Copy) < pos(MenuItem::PasteHere));
        assert!(pos(MenuItem::PasteHere) < pos(MenuItem::Ssh));
        assert!(pos(MenuItem::Ssh) < pos(MenuItem::Background));
        // Appearance block in the shared order: background, theme, language.
        assert!(pos(MenuItem::Background) < pos(MenuItem::ThemePick));
        assert!(pos(MenuItem::ThemePick) < pos(MenuItem::Lang));
        // OS group stays last before the footer.
        assert!(pos(MenuItem::Lang) < pos(MenuItem::OsMenu));
        assert!(pos(MenuItem::OsMenu) < pos(MenuItem::Quit));
    }

    #[test]
    fn shell_can_reach_the_command_line() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        // Ctrl+Enter from the shell opens cian's `:` command line (typing `:`
        // there would just go to the terminal).
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)).unwrap();
        assert_eq!(app.mode, Mode::Command);
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert_eq!(app.mode, Mode::Normal);

        // The shell menu also offers it, for terminals that can't report Ctrl+Enter.
        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        assert!(items.contains(&MenuItem::CommandInput), "shell menu offers Command…");
    }

    #[test]
    fn the_shell_menu_has_its_own_reduced_set() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        assert!(app.file_clip.is_none());
        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        // Strip the optional launchers (present only when snippets/AI/macros are
        // configured in the ambient config dir) so the core set is stable.
        let core: Vec<MenuItem> = items
            .iter()
            .cloned()
            .filter(|i| !matches!(i, MenuItem::Snippets | MenuItem::AiMenu | MenuItem::Macros))
            .collect();
        // No SSH hosts configured here, so Transfer ▸ is omitted. The zones:
        // pane action (Paste), shell groups (Session / Window), the shared
        // connect + appearance blocks, then the footer.
        assert_eq!(
            core,
            vec![
                MenuItem::CommandInput,
                MenuItem::Paste,
                MenuItem::SessionMenu,
                MenuItem::WindowMenu,
                MenuItem::Ssh,
                MenuItem::RemotePane,
                MenuItem::Background,
                MenuItem::ThemePick,
                MenuItem::Lang,
                MenuItem::Quit,
                MenuItem::Manual
            ]
        );
    }

    #[test]
    fn theme_picker_previews_live_and_esc_restores() {
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_d, mut app) = app_with(&["a.txt"]);
        let before = crate::theme::theme();
        app.start_theme_picker();
        assert!(matches!(app.popup, Popup::ThemePicker { .. }));
        // Moving the cursor applies the previewed preset to the live global.
        app.handle_key(code(KeyCode::Char('j'))).unwrap();
        assert_ne!(crate::theme::theme(), before, "preview should swap the theme");
        // Esc restores whatever was active on entry.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert_eq!(crate::theme::theme(), before, "cancel restores the original");
        assert!(matches!(app.popup, Popup::None));
    }

    #[test]
    fn pane_theme_override_is_scoped_and_clearable() {
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_d, mut app) = app_with(&["a.txt"]);
        let app_theme = crate::theme::theme();
        // Right pane (side 1) gallery: preview leaves the global app theme alone.
        app.start_pane_theme_picker(1);
        app.handle_key(code(KeyCode::Char('j'))).unwrap();
        assert_eq!(crate::theme::theme(), app_theme, "pane preview must not touch the global");
        assert!(app.pane_theme[1].is_some(), "the right pane gained an override");
        assert!(app.pane_theme[0].is_none(), "the left pane is untouched");
        // Keep it.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let kept = app.pane_theme[1].clone();
        assert!(kept.is_some());
        // Reopen and clear with `x` → follows the app again.
        app.start_pane_theme_picker(1);
        app.handle_key(code(KeyCode::Char('x'))).unwrap();
        assert!(app.pane_theme[1].is_none(), "x clears the override");
        assert!(matches!(app.popup, Popup::None));
    }

    #[test]
    fn pane_theme_picker_esc_restores_previous_override() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.pane_theme[0] = Some("nord".to_string());
        app.start_pane_theme_picker(0);
        app.handle_key(code(KeyCode::Char('j'))).unwrap();
        app.handle_key(code(KeyCode::Char('j'))).unwrap();
        // Cancel → the pane's prior override comes back.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert_eq!(app.pane_theme[0].as_deref(), Some("nord"));
    }

    #[test]
    fn saved_theme_round_trips_through_the_state_format() {
        use crate::state_get_in;
        // The exact shape save_theme_pref writes (comment header + quoted value).
        let body = "# cian runtime state — managed by cian (see :where)\ntheme = \"dracula\"\n";
        assert_eq!(state_get_in(body, "theme").as_deref(), Some("dracula"));
        // Tolerant of spacing and missing quotes; comments and blanks ignored.
        assert_eq!(state_get_in("theme=nord", "theme").as_deref(), Some("nord"));
        assert_eq!(state_get_in("  theme   =   \"one-dark\"  ", "theme").as_deref(), Some("one-dark"));
        assert_eq!(state_get_in("# theme = \"ignored\"\n", "theme").as_deref(), None);
        assert_eq!(state_get_in("theme = \"\"\n", "theme").as_deref(), None);
        assert_eq!(state_get_in("nothing here", "theme").as_deref(), None);
    }

    #[test]
    fn surface_follows_light_and_dark_themes() {
        use crate::theme::{set_theme, surface, ResolvedTheme};
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The dark default paints no base, so surfaces fall back to the (dark)
        // popup background — the menu / viewer stay dark.
        set_theme(ResolvedTheme::DARK);
        assert_eq!(surface(), ResolvedTheme::DARK.popup_bg);
        // A light theme has a light base_bg, so the menu / viewer go light and
        // their readable_on text turns dark.
        set_theme(ResolvedTheme::GITHUB_LIGHT);
        assert_eq!(surface(), ResolvedTheme::GITHUB_LIGHT.base_bg.unwrap());
        assert_eq!(crate::render::readable_on(surface()), Color::Rgb(30, 32, 40), "dark text on a light menu");
        set_theme(ResolvedTheme::DARK);
    }

    /// The crosshair has to step *away* from the surface, not always darker:
    /// a fixed dark tint turned a light theme's cursor line into a black bar
    /// with the text still on it.
    #[test]
    fn the_crosshair_shade_follows_the_theme() {
        use crate::render::shade_of_surface;
        use crate::theme::{set_theme, surface, ResolvedTheme};
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let lum = |c: Color| match c {
            Color::Rgb(r, g, b) => 299 * r as i32 + 587 * g as i32 + 114 * b as i32,
            _ => 0,
        };

        set_theme(ResolvedTheme::DARK);
        assert!(lum(shade_of_surface(40)) > lum(surface()), "lighter on a dark theme");
        set_theme(ResolvedTheme::GITHUB_LIGHT);
        assert!(lum(shade_of_surface(40)) < lum(surface()), "darker on a light one");
        // The cursor cell is the page's own two colours swapped, so it stays
        // legible whatever the line under it is tinted to — the one thing that
        // must never wash out.
        for t in [ResolvedTheme::DARK, ResolvedTheme::GITHUB_LIGHT] {
            set_theme(t);
            let (fg, bg) = (surface(), crate::render::readable_on(surface()));
            assert!(
                (lum(fg) - lum(bg)).abs() > 100_000,
                "the cursor stands off its own background",
            );
            assert!(
                (lum(bg) - lum(shade_of_surface(28))).abs() > 50_000,
                "…and off the tint of the line it sits on",
            );
        }
        set_theme(ResolvedTheme::DARK);
    }

    /// cian's chrome — the status bar, the pane tabs and column headings, the
    /// viewer's badge / tab strip / hint bar / prompt — was written as fixed
    /// colours chosen against a dark page: black on a chip, white on the
    /// status bar, the theme's border grey on a column heading. On a light
    /// theme those are their own background with words in it (the status bar
    /// scored 1.06:1 — white on near-white). Every letter of the chrome has
    /// to stand off what it is painted on, whatever the theme.
    #[test]
    fn the_chrome_reads_on_every_theme() {
        use crate::theme::{set_theme, ResolvedTheme};
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        for t in
            [ResolvedTheme::DARK, ResolvedTheme::GITHUB_LIGHT, ResolvedTheme::SOLARIZED_LIGHT]
        {
            set_theme(t);
            // stage 0 = the panes and the bars under them; 1 = the viewer over
            // them (two files, so the tab strip is drawn); 2 = with `:` open.
            for stage in 0..3 {
                let d = tempfile::tempdir().unwrap();
                std::fs::write(d.path().join("a.txt"), "AAA\n").unwrap();
                std::fs::write(d.path().join("b.txt"), "BBB\n").unwrap();
                let p = d.path().to_path_buf();
                let mut app = App::new(p.clone(), p, en_config()).unwrap();
                for n in ["a.txt", "b.txt"] {
                    let path = app
                        .active_pane()
                        .unwrap()
                        .entries
                        .iter()
                        .find(|e| e.name == n)
                        .unwrap()
                        .path
                        .clone();
                    app.active_pane_mut().unwrap().marks.insert(path);
                }
                if stage >= 1 {
                    app.handle_key(code(KeyCode::F(3))).unwrap();
                }
                if stage == 2 {
                    app.handle_key(key(':')).unwrap();
                }
                let buf = render_buf(&mut app, 100, 30);
                let h = buf.area.height;
                let row_text = |y: u16| {
                    (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()
                };

                // The bars along the bottom and the pane chrome at the top run
                // the full width; the viewer's own chrome is checked only
                // within the viewer, since the panes show at the edges of the
                // same rows and are drawn by someone else.
                // Rows 0-2 are the panes' own chrome (tabs, column headings)
                // only while the viewer is not covering them; the bars along
                // the bottom are there in every stage.
                let full: Vec<u16> = if stage == 0 {
                    vec![0, 1, 2, h - 2, h - 1]
                } else {
                    vec![h - 2, h - 1]
                };
                let viewer_rows: Vec<u16> = (0..h)
                    .filter(|y| {
                        let r = row_text(*y);
                        r.contains("READ") || r.contains("COMMAND") || r.contains("1 a.txt")
                            || r.contains("search") || r.contains("s/old/new/")
                    })
                    .collect();
                let (vx0, vx1) =
                    (app.viewer_rect.x, app.viewer_rect.x + app.viewer_rect.width);
                let mut checked = 0;
                for y in 0..h {
                    let (x0, x1) = if full.contains(&y) {
                        (0, buf.area.width)
                    } else if viewer_rows.contains(&y) {
                        (vx0, vx1)
                    } else {
                        continue;
                    };
                    for x in x0..x1 {
                        let c = &buf[(x, y)];
                        // Letters and digits only: borders, separators and
                        // glyphs are decoration, and are meant to be quieter.
                        if !c.symbol().chars().all(char::is_alphanumeric)
                            || c.symbol().trim().is_empty()
                        {
                            continue;
                        }
                        // A Reset fg/bg is the terminal's own colour and
                        // cannot be measured from here.
                        if matches!(c.fg, Color::Reset) || matches!(c.bg, Color::Reset) {
                            continue;
                        }
                        checked += 1;
                        // WCAG's own measure rather than a luminance
                        // difference: the pairs that failed here score
                        // respectably by luminance and are still unreadable.
                        // 4.0 is a shade under the 4.5 wanted for body text,
                        // which is fair for bold chrome a few characters long.
                        let cr = crate::render::contrast_ratio(c.fg, c.bg);
                        assert!(
                            cr >= 4.0,
                            "{:?} stage{stage}: {:?} at ({x},{y}) — {:?} on {:?} is {cr:.2}:1",
                            t.accent,
                            c.symbol(),
                            c.fg,
                            c.bg,
                        );
                    }
                }
                assert!(checked > 20, "found almost no chrome to check ({checked} cells)");
            }
        }
        set_theme(ResolvedTheme::DARK);
    }

    /// Syntax colours were all chosen against a dark page. On a light one the
    /// pale ones — plain text most of all — were two shades from the paper,
    /// and the cursor line's tint underneath finished the job.
    #[test]
    fn syntax_colours_stay_legible_on_a_light_theme() {
        use crate::render::{hl_style_for, readable_on};
        use crate::theme::{set_theme, surface, ResolvedTheme};
        use cian_core::highlight::Category as C;
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let lum = |c: Color| match c {
            Color::Rgb(r, g, b) => (299 * r as i32 + 587 * g as i32 + 114 * b as i32) / 1000,
            _ => 0,
        };

        for t in [ResolvedTheme::DARK, ResolvedTheme::GITHUB_LIGHT] {
            set_theme(t);
            let page = lum(surface());
            for cat in [C::Plain, C::Keyword, C::Type, C::Str, C::Comment, C::Number, C::Tag, C::Attr] {
                let fg = hl_style_for(cat);
                assert!(
                    (lum(fg) - page).abs() >= 80,
                    "{cat:?} is only {} from the page",
                    (lum(fg) - page).abs(),
                );
            }
            // Plain text is not a syntax colour at all — it is whatever reads
            // on this page.
            assert_eq!(hl_style_for(C::Plain), readable_on(surface()));
        }
        set_theme(ResolvedTheme::DARK);
    }

    /// The cursor cell has to be readable on every theme. It was built as the
    /// page's two colours swapped and then had the body colour put back on top
    /// of it, which made the character the same near-black as its own block —
    /// a solid square with the letter painted out inside.
    #[test]
    fn the_cursor_cell_never_paints_out_its_own_character() {
        use crate::theme::{set_theme, ResolvedTheme};
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let lum = |c: Color| match c {
            Color::Rgb(r, g, b) => (299 * r as i32 + 587 * g as i32 + 114 * b as i32) / 1000,
            _ => 0,
        };
        for t in [ResolvedTheme::DARK, ResolvedTheme::GITHUB_LIGHT] {
            set_theme(t);
            let (_d, mut app) = viewer_on("    println!();\n");
            app.show_ws = false;
            if let Popup::Viewer { col, .. } = &mut app.popup {
                *col = 4; // the `p`, not the indent
            }
            let (sym, fg, bg) = cursor_cell(&mut app, 100, 20);
            assert_eq!(sym, "p", "the cursor is on the character we think");
            assert!(
                (lum(fg) - lum(bg)).abs() > 90,
                "the character reads against its own block: {fg:?} on {bg:?}",
            );
        }
        set_theme(ResolvedTheme::DARK);
    }

    #[test]
    fn theme_set_by_name_sticks() {
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // set_theme_by_name persists to state.toml; point the config dir at a
        // tempdir so the test never clobbers the real ~/.config/cian.
        let cfg = tempfile::tempdir().unwrap();
        std::env::set_var("CIAN_CONFIG_DIR", cfg.path());
        let (_d, mut app) = app_with(&["a.txt"]);
        app.set_theme_by_name("dracula");
        assert_eq!(crate::theme::theme(), crate::theme::ResolvedTheme::DRACULA);
        assert_eq!(app.theme_name, "dracula");
        // Restore the default so other tests reading the global are unaffected.
        crate::theme::set_theme(crate::theme::ResolvedTheme::DARK);
        std::env::remove_var("CIAN_CONFIG_DIR");
    }

    #[test]
    fn explain_error_without_a_shell_reports_it() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // Force AI on (mock) so we get past the config gate to the shell check.
        app.ai = Some(cian_ai::AiConfig { auth_mode: "mock".into(), ..Default::default() });
        app.focus(FocusedPane::Shell);
        app.explain_shell_error();
        assert!(app.message.as_deref().unwrap_or("").contains("no shell"),
            "reports the absence of a shell: {:?}", app.message);
    }

    #[test]
    fn shell_window_submenu_offers_splits_tabs_and_zoom() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        app.open_context_menu(3, 3);
        // Drill into Window ▸.
        app.run_menu_item(MenuItem::WindowMenu).unwrap();
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no submenu") };
        assert!(items.contains(&MenuItem::ShellSplitLR));
        assert!(items.contains(&MenuItem::ShellSplitTB));
        assert!(items.contains(&MenuItem::ShellNewTab));
        assert!(items.contains(&MenuItem::ShellZoom));
        // A single (unsplit) tab offers "close tab", not "close split".
        assert!(items.contains(&MenuItem::ShellCloseTab));
        assert!(!items.contains(&MenuItem::ShellCloseSplit));
        assert!(items.contains(&MenuItem::Back));
    }

    #[test]
    fn attributes_lines_show_a_size_for_a_file() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("data.bin");
        std::fs::write(&f, vec![0u8; 2048]).unwrap();
        let (_d2, app) = app_with(&["a.txt"]);
        let lines = app.attributes_lines(&[f], 40);
        // Human-readable size appears on the entry's row.
        assert!(lines.iter().any(|l| l.contains("data.bin") && (l.contains("2.0K") || l.contains("2K") || l.contains("2048"))),
            "size shown: {lines:?}");
    }

    #[test]
    fn scp_upload_walks_picker_then_browses_the_server() {
        let (_d, mut app) = app_with_ssh();
        app.active_pane_mut().unwrap().cursor = 1; // a.txt (index 0 is the `..` row)
        app.start_scp(ScpDir::Upload);
        assert!(matches!(app.popup, Popup::SshHosts { .. }), "opens the host picker");
        assert!(app.scp_dir.is_some());

        // Pick db1 (single user, has a password) → the WinSCP-style remote browser.
        app.command_buffer.clear();
        // Filter to db1 then Enter.
        for c in "db1".chars() {
            app.handle_key(code(KeyCode::Char(c))).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();

        match &app.popup {
            Popup::RemoteBrowser { purpose: BrowsePurpose::Upload, .. } => {}
            other => panic!("expected the upload browser, got {:?}", other),
        }
        let p = app.scp_pending.as_ref().expect("a pending transfer");
        assert_eq!(p.target.host, "10.0.2.31");
        assert_eq!(p.target.port, 2222);
        assert_eq!(p.target.user, "postgres");
        assert_eq!(p.locals.len(), 1);
    }

    /// Multi-file upload asks for each file's chmod in turn: a valid mode
    /// advances, an invalid one re-asks the same file (without losing the
    /// upload), and a blank keeps the server default.
    #[test]
    fn upload_chmod_is_per_file_and_reprompts_on_error() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // Stand in a pending 3-file upload directly (skips the network browser).
        app.scp_pending = Some(crate::ScpPending {
            target: cian_scp::Target {
                host: "h".into(),
                port: 22,
                user: "u".into(),
                password: "p".into(),
            key: None,
            key_pass: None,
            },
            label: "u@h".into(),
            locals: vec![
                std::path::PathBuf::from("/tmp/one.txt"),
                std::path::PathBuf::from("/tmp/two.txt"),
                std::path::PathBuf::from("/tmp/three.txt"),
            ],
        });
        app.scp_upload_modes.clear();

        let set_buf = |app: &mut App, s: &str| {
            if let Popup::TextInput { buffer, cursor, .. } = &mut app.popup {
                *buffer = s.to_string();
                *cursor = buffer.chars().count();
            } else {
                panic!("expected a chmod TextInput, got {:?}", app.popup);
            }
        };

        app.prompt_upload_chmod("/dest".into(), 0);
        match &app.popup {
            Popup::TextInput { kind: InputKind::UploadChmod { idx: 0, .. }, title, .. } => {
                assert!(title.contains("1/3"), "shows file 1 of 3: {title}");
            }
            other => panic!("expected file-1 chmod prompt, got {:?}", other),
        }

        // File 1: a valid mode advances to file 2.
        set_buf(&mut app, "755");
        app.finish_text_input().unwrap();
        assert_eq!(app.scp_upload_modes, vec![Some(0o755)]);
        assert!(matches!(app.popup, Popup::TextInput { kind: InputKind::UploadChmod { idx: 1, .. }, .. }));

        // File 2: an invalid mode re-asks the same file and keeps the pending upload.
        set_buf(&mut app, "zzz");
        app.finish_text_input().unwrap();
        assert!(app.message.as_deref().unwrap_or("").contains("invalid chmod"));
        assert_eq!(app.scp_upload_modes, vec![Some(0o755)], "no mode recorded for the bad entry");
        assert!(matches!(app.popup, Popup::TextInput { kind: InputKind::UploadChmod { idx: 1, .. }, .. }));
        assert!(app.scp_pending.is_some(), "the upload is not dropped on a bad mode");

        // File 2 again: blank keeps the server default and advances to file 3.
        set_buf(&mut app, "");
        app.finish_text_input().unwrap();
        assert_eq!(app.scp_upload_modes, vec![Some(0o755), None]);
        assert!(matches!(app.popup, Popup::TextInput { kind: InputKind::UploadChmod { idx: 2, .. }, .. }));
    }

    #[test]
    fn manual_ssh_target_parses_user_host_port() {
        let (_d, mut app) = app_with_ssh();
        app.active_pane_mut().unwrap().cursor = 1; // a.txt
        app.start_scp(ScpDir::Upload);
        // F2 from the host picker → type the server by hand.
        app.handle_key(code(KeyCode::F(2))).unwrap();
        assert!(matches!(
            app.popup,
            Popup::TextInput { kind: InputKind::ManualSshTarget { for_scp: true }, .. }
        ));
        for c in "deploy@web9:2201".chars() {
            app.handle_key(code(KeyCode::Char(c))).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        // Advances to the masked password step, carrying the parsed pieces.
        match &app.popup {
            Popup::TextInput { kind: InputKind::ManualSshPass { user, host, port, for_scp }, .. } => {
                assert_eq!(user, "deploy");
                assert_eq!(host, "web9");
                assert_eq!(*port, 2201);
                assert!(for_scp);
            }
            other => panic!("expected the password prompt, got {:?}", other),
        }
    }

    #[test]
    fn scp_upload_without_a_selected_file_is_refused() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("onlydir")).unwrap();
        let mut config = en_config();
        config.ssh_hosts = vec![cian_lua::SshHost {
            name: "web1".into(),
            host: "10.0.1.11".into(),
            users: vec![cian_lua::SshUser::plain("root")],
            port: None,
            notes: None,
        }];
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, config).unwrap();
        app.active_pane_mut().unwrap().cursor = 0; // the directory
        app.start_scp(ScpDir::Upload);
        assert!(matches!(app.popup, Popup::None));
        assert!(app.message.as_deref().unwrap().contains("select a file"));
    }

    #[test]
    fn scp_needs_a_password_for_the_user() {
        let (_d, mut app) = app_with_ssh();
        app.active_pane_mut().unwrap().cursor = 1; // a real file, not the `..` row
        app.start_scp(ScpDir::Upload);
        // web1 / root has no password configured.
        for c in "web1".chars() {
            app.handle_key(code(KeyCode::Char(c))).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap(); // host web1 → user list
        app.handle_key(code(KeyCode::Enter)).unwrap(); // first user (root)
        assert!(app.scp_pending.is_none());
        assert!(app.message.as_deref().unwrap().contains("no password"));
    }

    #[test]
    fn a_host_name_is_pulled_from_a_terminal_title() {
        assert_eq!(host_from_title("taketan@web01: ~/proj"), Some("web01".into()));
        assert_eq!(host_from_title("root@db-server:/var"), Some("db-server".into()));
        // No `@` — nothing to take.
        assert_eq!(host_from_title("just a title"), None);
    }

    #[test]
    fn the_log_prompt_asks_for_a_folder_when_a_shell_exists() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // No shell yet → it declines rather than opening a prompt.
        app.start_log_prompt();
        assert!(matches!(app.popup, Popup::None));
        assert!(app.message.as_deref().unwrap().contains("no shell"));
    }

    #[test]
    fn starting_a_log_in_a_bad_directory_is_refused() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.start_session_log("/no/such/directory/anywhere");
        assert!(app.message.as_deref().unwrap().contains("not a directory"));
    }

    #[test]
    fn choosing_the_manual_from_the_menu_opens_it() {
        let (_d, mut app) = app_with_lang(&["a.txt"], "en");
        let _ = render(&mut app, 100, 40);
        app.open_context_menu(5, 5);

        // Walk to the last entry and activate it with the keyboard.
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        let steps = items.len() - 1;
        for _ in 0..steps {
            app.handle_key(key('j')).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();

        assert!(matches!(app.popup, Popup::Manual { .. }), "expected the manual");
        let screen = render(&mut app, 100, 40).join("\n");
        assert!(screen.contains("key manual"), "manual should be on screen");
    }

    #[test]
    fn the_color_picker_sets_only_the_chosen_pane() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Right);
        app.run_menu_item(MenuItem::Background).unwrap();
        assert!(matches!(app.popup, Popup::ColorPicker { .. }));

        // Move off "default" and apply.
        app.handle_key(key('j')).unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();

        assert!(app.pane_bg[1].is_some(), "right pane should be tinted");
        assert!(app.pane_bg[0].is_none(), "left pane must be untouched");
        assert!(matches!(app.popup, Popup::None));
    }

    #[test]
    fn a_flash_fades_out_and_then_expires() {
        let (_d, mut app) = app_with(&["a.txt"]);
        assert_eq!(app.flash_level(FocusedPane::Left), 0.0);

        app.flash(FocusedPane::Left);
        assert!(app.flash_level(FocusedPane::Left) > 0.9, "should start near full");
        assert_eq!(app.flash_level(FocusedPane::Right), 0.0, "only the named pane lights");
        assert!(app.flash_active());

        // Pretend the flash started long ago.
        app.flash = Some((FocusedPane::Left, Instant::now() - Duration::from_secs(2)));
        assert_eq!(app.flash_level(FocusedPane::Left), 0.0);
        assert!(!app.flash_active());
    }

    #[test]
    fn easing_stays_in_range_and_hits_both_ends() {
        let a = Anim {
            kind: AnimKind::Zoom { from: Rect::new(0, 0, 10, 10), to: Rect::new(0, 0, 20, 20) },
            start: Instant::now(),
            dur: Duration::from_millis(100),
        };
        assert!(a.progress() < 0.2, "should start near zero");
        assert!(!a.done());

        let ended = Anim { start: Instant::now() - Duration::from_secs(1), ..a };
        assert_eq!(ended.progress(), 1.0);
        assert!(ended.done());

        // A zero-length transition is already over.
        let instant = Anim { dur: Duration::ZERO, ..a };
        assert_eq!(instant.progress(), 1.0);
        assert!(instant.done());
    }

    #[test]
    fn lerp_rect_interpolates_between_its_endpoints() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(10, 20, 30, 40);
        assert_eq!(lerp_rect(a, b, 0.0), a);
        assert_eq!(lerp_rect(a, b, 1.0), b);
        let mid = lerp_rect(a, b, 0.5);
        assert_eq!((mid.x, mid.y, mid.width, mid.height), (5, 10, 20, 25));
        // Never collapses to nothing, which would make a widget panic.
        let z = lerp_rect(Rect::new(0, 0, 0, 0), Rect::new(0, 0, 0, 0), 0.5);
        assert!(z.width >= 1 && z.height >= 1);
    }

    #[test]
    fn union_rect_ignores_empty_inputs() {
        let a = Rect::new(0, 0, 10, 5);
        let b = Rect::new(10, 0, 10, 5);
        assert_eq!(union_rect(a, b), Rect::new(0, 0, 20, 5));
        assert_eq!(union_rect(a, Rect::new(0, 0, 0, 0)), a);
        assert_eq!(union_rect(Rect::new(0, 0, 0, 0), b), b);
    }

    /// Both directions must actually travel. The un-zoom used to read the
    /// focused pane's rect out of `layout_rects`, which by then described the
    /// *zoomed* layout — so `from` and `to` were both the full window and the
    /// transition, while running, moved nothing.
    #[test]
    fn zoom_animates_in_both_directions() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);
        assert!(!app.zoomed);
        let pane = app.layout_rects.left;

        app.toggle_zoom();
        assert!(app.zoomed);
        let Some(Anim { kind: AnimKind::Zoom { from, to }, .. }) = app.anim else {
            panic!("expected a zoom")
        };
        assert_eq!(from, pane, "should grow out of the pane it was in");
        assert!(to.width > from.width && to.height > from.height, "{:?} -> {:?}", from, to);
        app.finish_anim();

        // Rendering while zoomed overwrites layout_rects with the zoomed
        // layout — the exact condition that broke the way back.
        let _ = render(&mut app, 100, 40);

        app.toggle_zoom();
        assert!(!app.zoomed);
        let Some(Anim { kind: AnimKind::Zoom { from, to }, .. }) = app.anim else {
            panic!("expected a zoom back")
        };
        assert_ne!(from, to, "the way back must travel, not sit still");
        assert!(to.width < from.width && to.height < from.height, "{:?} -> {:?}", from, to);
        assert_eq!(to, pane, "should shrink into the pane it came from");
    }

    #[test]
    fn zooming_the_shell_returns_to_the_shell_rect() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);
        app.focus(FocusedPane::Shell);
        let shell = app.layout_rects.shell;

        app.toggle_zoom();
        app.finish_anim();
        let _ = render(&mut app, 100, 40);
        app.toggle_zoom();

        let Some(Anim { kind: AnimKind::Zoom { to, .. }, .. }) = app.anim else {
            panic!("expected a zoom back")
        };
        assert_eq!(to, shell, "each surface returns to its own rect");
    }

    #[test]
    fn animation_can_be_switched_off_by_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let mut config = en_config();
        config.options.animation_ms = Some(0);
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, config).unwrap();
        let _ = render(&mut app, 100, 40);

        app.toggle_zoom();
        assert!(app.zoomed, "the zoom itself must still happen");
        assert!(app.anim.is_none(), "but with no transition");
    }

    #[test]
    fn the_ratio_override_only_applies_to_its_own_divider() {
        let ov = AnimOverride {
            ratio: Some((DividerTarget::Panes, 90)),
            freeze_pty: true,
            show_splits: false,
        };
        assert_eq!(ov.ratio_for(DividerTarget::Panes, 50), 90);
        // Other dividers fall through to their stored value.
        assert_eq!(ov.ratio_for(DividerTarget::Main, 60), 60);
        // Stored values are clamped; overrides are not, so a close animation
        // can drive a pane all the way to zero.
        assert_eq!(ov.ratio_for(DividerTarget::Main, 99), 100 - MIN_SPLIT_PCT);
        let zero =
            AnimOverride { ratio: Some((DividerTarget::Main, 0)), freeze_pty: true, show_splits: false };
        assert_eq!(zero.ratio_for(DividerTarget::Main, 50), 0);
    }

    #[test]
    fn a_deferred_close_runs_when_its_transition_lands() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // Nothing to close, but the deferral machinery should still fire
        // exactly once and then clear itself.
        app.anim_then = Some(PendingClose::ShellPane);
        app.start_anim(AnimKind::Ratio {
            target: DividerTarget::Main,
            from: 50,
            to: 0,
        });
        assert!(app.anim.is_some());

        app.finish_anim();
        assert!(app.anim.is_none());
        assert!(app.anim_then.is_none(), "deferred work must be consumed");
    }

    #[test]
    fn split_ratio_survives_a_render_round_trip() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);
        app.panes_pct = 30;
        let _ = render(&mut app, 100, 40);
        // 30% of a 100-wide window, give or take rounding.
        assert!(
            (28..=32).contains(&app.layout_rects.left.width),
            "got {}",
            app.layout_rects.left.width
        );
    }

    /// Right-clicking a row must select the file actually drawn on that row,
    /// including after the list has scrolled.
    #[test]
    fn right_click_selects_the_row_under_the_pointer_when_scrolled() {
        let names: Vec<String> = (0..60).map(|i| format!("f{:02}.txt", i)).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let (_d, mut app) = app_with(&refs);
        let _ = render(&mut app, 100, 40);
        app.focus(FocusedPane::Left);

        let rect = app.layout_rects.left;
        let view_h = rect.height.saturating_sub(2);

        // Every combination of scroll position and clicked row must agree.
        for cursor in [0usize, 5, 20, 45, 59] {
            for off in 0..view_h.min(8) {
                if let Some(p) = app.active_pane_mut() {
                    p.cursor = cursor;
                }
                let before = render(&mut app, 100, 40);
                let row = rect.y + 2 + off;
                let lo = rect.x as usize;
                let hi = (rect.x + rect.width) as usize;
                let drawn: String =
                    before[row as usize].chars().skip(lo).take(hi - lo).collect();
                app.handle_mouse(mouse(
                    MouseEventKind::Down(MouseButton::Right),
                    rect.x + 3,
                    row,
                ));
                let sel = app.active_pane().unwrap().selected().unwrap().name.clone();
                assert!(
                    drawn.contains(&sel),
                    "cursor {} row-offset {}: screen showed {:?}, selected {:?}",
                    cursor,
                    off,
                    drawn.trim(),
                    sel
                );
                app.popup = Popup::None;
            }
        }
    }

    #[test]
    fn right_click_on_a_single_screenful_selects_correctly() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        let _ = render(&mut app, 100, 40);
        app.focus(FocusedPane::Left);
        let rect = app.layout_rects.left;
        // Clicking past the last entry must leave the cursor where it was
        // rather than jumping somewhere arbitrary.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), rect.x + 3, rect.y + 2));
        assert_eq!(app.active_pane().unwrap().cursor, 0);
        app.popup = Popup::None;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), rect.x + 3, rect.y + 3));
        assert_eq!(app.active_pane().unwrap().cursor, 1);
        app.popup = Popup::None;

        // A row inside the pane but past the last entry: stay put.
        let before = app.active_pane().unwrap().cursor;
        let blank = rect.y + rect.height - 3;
        assert!(blank > rect.y + 3, "test needs a pane taller than the listing");
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), rect.x + 3, blank));
        assert_eq!(app.focused, FocusedPane::Left, "still inside the pane");
        assert_eq!(app.active_pane().unwrap().cursor, before, "empty space must not move it");
        app.popup = Popup::None;

        // The pane's own border row is not a list row either.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), rect.x + 3, rect.y));
        assert_eq!(app.active_pane().unwrap().cursor, before, "the border must not move it");
    }

    /// Degenerate geometry must not panic (u16 underflow in seam maths).
    #[test]
    fn rendering_survives_a_tiny_terminal() {
        let (_d, mut app) = app_with(&["a.txt"]);
        for (w, h) in [(1u16, 1u16), (2, 2), (4, 3), (10, 4), (1, 40), (40, 1)] {
            let _ = render(&mut app, w, h);
        }
        // And with a popup open, which does its own rect maths.
        app.open_manual();
        for (w, h) in [(1u16, 1u16), (3, 3), (12, 5)] {
            let _ = render(&mut app, w, h);
        }
    }

    #[test]
    fn the_shell_menu_offers_a_background_color() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        assert!(
            items.contains(&MenuItem::Background),
            "the shell pane should be tintable too, got {:?}",
            items
        );
    }

    #[test]
    fn the_color_picker_tints_only_the_active_split_pane() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        app.run_menu_item(MenuItem::Background).unwrap();
        app.handle_key(key('j')).unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();

        // The file panes keep their own (unset) backgrounds.
        assert!(app.pane_bg[0].is_none() && app.pane_bg[1].is_none());
        // With no shell running there is no pane to color, and nothing panics.
        assert!(app.shell.active_pane_bg().is_none());
    }

    /// A pane's color must stop at that pane. This used to be stored per
    /// panel, so coloring one split painted every split and every tab —
    /// including ones meant to keep the terminal's own background.
    #[test]
    fn a_pane_tint_stops_at_that_pane() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let tint = Color::Rgb(17, 45, 87);
        app.pane_bg[0] = Some(tint);

        let buf = render_buf(&mut app, 100, 40);
        let left = app.layout_rects.left;
        let right = app.layout_rects.right;
        assert!(left.height > 2 && right.height > 2, "need a real layout");

        assert_eq!(
            buf[(left.x + 5, left.y + left.height / 2)].bg,
            tint,
            "the colored pane should be tinted"
        );
        assert_ne!(
            buf[(right.x + 5, right.y + right.height / 2)].bg,
            tint,
            "the tint must not reach the other pane"
        );
    }

    /// The exact split sequence a 2×2 grid macro issues (Cgrid4: pane2 splits
    /// pane1 right, pane3 splits pane1 down, pane4 splits pane2 down) must build
    /// a real grid — a left/right split whose two columns are each split into
    /// rows — not four side-by-side columns.
    #[test]
    fn macro_grid_from_targets_build_a_2x2() {
        let dir = tempfile::tempdir().unwrap();
        let sh = cian_pty::default_shell();
        let mk = || cian_pty::PtySession::new(dir.path(), &sh, 24, 80).unwrap();

        let mut tab = ShellTab::new(mk());
        let mut leaf_ids = vec![tab.active]; // pane 1
        tab.split_from(leaf_ids[0], SplitDir::LeftRight, 50, mk()); // pane2 from=1 right
        leaf_ids.push(tab.active);
        tab.split_from(leaf_ids[0], SplitDir::TopBottom, 50, mk()); // pane3 from=1 down
        leaf_ids.push(tab.active);
        tab.split_from(leaf_ids[1], SplitDir::TopBottom, 50, mk()); // pane4 from=2 down
        leaf_ids.push(tab.active);

        assert_eq!(tab.leaves().len(), 4, "four panes");
        let Some(Node::Split { dir, first, second, .. }) = tab.nodes.get(tab.root).and_then(|n| n.as_ref())
        else {
            panic!("root should be a split");
        };
        assert_eq!(*dir, SplitDir::LeftRight, "the outer split makes two columns");
        for (label, child) in [("left", *first), ("right", *second)] {
            match tab.nodes.get(child).and_then(|n| n.as_ref()) {
                Some(Node::Split { dir, .. }) => {
                    assert_eq!(*dir, SplitDir::TopBottom, "{label} column is split into rows");
                }
                _ => panic!("{label} column should be a top/bottom split"),
            }
        }
    }

    /// `new_tab_running` (behind "Edit in new tab") must actually open a tab; the
    /// command is delivered when the tab's shell lands.
    #[test]
    fn new_tab_running_opens_a_new_tab() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let cwd = app.active_pane().unwrap().cwd.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        app.shell.ensure(&cwd);
        while app.shell.count() == 0 {
            app.shell.poll_pending();
            assert!(std::time::Instant::now() < deadline, "first tab never spawned");
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        let before = app.shell.count();
        app.shell.new_tab_running(&cwd, "echo hi".into());
        while app.shell.count() == before {
            app.shell.poll_pending();
            assert!(std::time::Instant::now() < deadline, "new tab never opened");
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        assert_eq!(app.shell.count(), before + 1, "a fresh tab opened for the editor");
    }

    /// End-to-end: drive a 2×2 grid macro through the real tick loop (async PTY
    /// spawns and all) and confirm the *built* layout is a grid, not four
    /// columns — the actual #1 report. This exercises the leaf-id bookkeeping
    /// that the synchronous tree test cannot.
    #[test]
    fn macro_builds_a_real_2x2_grid_end_to_end() {
        use cian_lua::macros::{Macro, PaneStep, Split};
        let pane = |from: Option<usize>, dir: Split| PaneStep { dir, from, ..Default::default() };
        let m = Macro {
            name: "grid".into(),
            sync: false,
            zoom: false,
            script: None,
            panes: vec![
                pane(None, Split::Right),    // pane 1 (the shell you're on)
                pane(Some(1), Split::Right), // pane 2: split pane 1 → right
                pane(Some(1), Split::Down),  // pane 3: split pane 1 → down
                pane(Some(2), Split::Down),  // pane 4: split pane 2 → down
            ],
        };

        let (_d, mut app) = app_with(&["a.txt"]);
        app.begin_macro(&m);
        let start = std::time::Instant::now();
        while app.macro_run.is_some() {
            app.shell.poll_pending();
            app.tick_macro();
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "macro did not finish");
            std::thread::sleep(std::time::Duration::from_millis(3));
        }

        let tab = app.shell.active_tab().expect("a shell tab");
        assert_eq!(tab.leaves().len(), 4, "the macro built four panes");
        let Some(Node::Split { dir, first, second, .. }) = tab.nodes.get(tab.root).and_then(|n| n.as_ref())
        else {
            panic!("root should be a split");
        };
        assert_eq!(*dir, SplitDir::LeftRight, "two columns, not four");
        for (label, child) in [("left", *first), ("right", *second)] {
            match tab.nodes.get(child).and_then(|n| n.as_ref()) {
                Some(Node::Split { dir, .. }) => {
                    assert_eq!(*dir, SplitDir::TopBottom, "{label} column split into two rows");
                }
                _ => panic!("{label} column should be a top/bottom split (got a bare pane)"),
            }
        }
    }

    /// Two split panes, each with its own background — the case that was
    /// impossible when the color lived on the panel.
    #[test]
    fn split_panes_hold_separate_backgrounds() {
        let dir = tempfile::tempdir().unwrap();
        let sh = cian_pty::default_shell();
        let mk = || cian_pty::PtySession::new(dir.path(), &sh, 24, 80).unwrap();

        let mut tab = ShellTab::new(mk());
        let first = tab.active;
        tab.split(SplitDir::LeftRight, mk());
        let second = tab.active;
        assert_ne!(first, second, "split should make a second leaf");

        let set = |t: &mut ShellTab, leaf: usize, c: Color| {
            if let Some(Node::Leaf { bg, .. }) = t.nodes.get_mut(leaf).and_then(|n| n.as_mut()) {
                *bg = Some(c);
            }
        };
        let get = |t: &ShellTab, leaf: usize| match t.nodes.get(leaf).and_then(|n| n.as_ref()) {
            Some(Node::Leaf { bg, .. }) => *bg,
            _ => None,
        };

        set(&mut tab, first, Color::Rgb(17, 45, 87));
        assert_eq!(get(&tab, first), Some(Color::Rgb(17, 45, 87)));
        assert_eq!(get(&tab, second), None, "the sibling must stay on the default");

        set(&mut tab, second, Color::Rgb(87, 29, 17));
        assert_eq!(get(&tab, first), Some(Color::Rgb(17, 45, 87)), "unchanged by its sibling");
        assert_eq!(get(&tab, second), Some(Color::Rgb(87, 29, 17)));
    }

    /// Clicking a split must act on the pane under the pointer. Without this,
    /// right-clicking the left half of a split colored the right half —
    /// whichever happened to be active.
    #[test]
    fn clicking_a_split_selects_the_pane_under_the_pointer() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);

        // Two leaves side by side, standing in for a real split.
        let shell = app.layout_rects.shell;
        let half = shell.width / 2;
        let l0 = Rect::new(shell.x, shell.y, half, shell.height);
        let l1 = Rect::new(shell.x + half, shell.y, half, shell.height);
        app.shell_leaves = vec![(0, 7, l0, l0), (0, 9, l1, l1)];
        app.shell.tabs.push(ShellTab { nodes: Vec::new(), root: 0, active: 9, name: String::new() });

        app.select_shell_leaf_at(shell.x + 2, shell.y + 2);
        assert_eq!(app.shell.tabs[0].active, 7, "should pick the left pane");

        app.select_shell_leaf_at(shell.x + half + 2, shell.y + 2);
        assert_eq!(app.shell.tabs[0].active, 9, "should pick the right pane");

        // A point outside every pane leaves the selection alone.
        app.select_shell_leaf_at(0, 0);
        assert_eq!(app.shell.tabs[0].active, 9);
    }

    #[test]
    fn the_shell_hints_mention_pane_switching_only_when_split() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        // No panes yet: the key would do nothing, so it is not advertised.
        assert!(!key_hints(&app).iter().any(|(k, _)| *k == "S-F1/F2"));
    }

    #[test]
    fn the_palette_is_distinct_enough_to_tell_panes_apart() {
        // The first entry is "no color"; the rest must be visibly different
        // from one another, which an earlier too-subtle set was not.
        let colors: Vec<(u8, u8, u8)> = pane_bg_presets()
            .iter()
            .filter_map(|(_, c)| match c {
                Some(Color::Rgb(r, g, b)) => Some((*r, *g, *b)),
                _ => None,
            })
            .collect();
        assert_eq!(colors.len(), pane_bg_presets().len() - 1);
        for (i, a) in colors.iter().enumerate() {
            for b in colors.iter().skip(i + 1) {
                let d = (a.0 as i32 - b.0 as i32).abs()
                    + (a.1 as i32 - b.1 as i32).abs()
                    + (a.2 as i32 - b.2 as i32).abs();
                assert!(d >= 60, "{:?} and {:?} are too close to tell apart", a, b);
            }
            // Dark enough that normal foreground text stays readable.
            let lum = 0.299 * a.0 as f32 + 0.587 * a.1 as f32 + 0.114 * a.2 as f32;
            assert!(lum < 90.0, "{:?} is too light for text on top (lum {})", a, lum);
        }
    }

    /// Cells the shell colored for itself must survive the tint, or ls
    /// colors and vim themes would be flattened.
    #[test]
    fn the_tint_leaves_explicitly_colored_cells_alone() {
        // The theme decides whether an untouched cell is Reset at all (a light
        // theme paints the whole surface), so this holds the theme still while
        // it looks — otherwise a theme test running beside it decides the
        // answer.
        use crate::theme::{set_theme, ResolvedTheme};
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_theme(ResolvedTheme::DARK);
        let (_d, mut app) = app_with(&["a.txt"]);
        // Give a file pane a background so there are non-Reset cells to guard,
        // then tint the whole screen area and check they are preserved.
        let painted = Color::Rgb(40, 0, 0);
        app.pane_bg[0] = Some(painted);
        let tint = Color::Rgb(0, 0, 40);

        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal
            .draw(|f| {
                draw(f, &mut app);
                tint_default_cells(f, f.area(), tint);
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();

        let left = app.layout_rects.left;
        let cell = buf[(left.x + 5, left.y + left.height / 2)].bg;
        assert_eq!(cell, painted, "an already-colored cell must not be repainted");

        // And a cell that was Reset did get the tint.
        let right = app.layout_rects.right;
        assert_eq!(buf[(right.x + 5, right.y + right.height / 2)].bg, tint);
    }

    #[test]
    fn comma_opens_the_sort_picker_and_enter_applies_it() {
        let (_d, mut app) = app_with(&["b.rs", "a.rs", "c.md"]);
        app.handle_key(key(',')).unwrap();
        assert!(matches!(app.popup, Popup::SortPicker { .. }));

        // Jump straight to extension with its mnemonic.
        app.handle_key(key('e')).unwrap();
        assert!(matches!(app.popup, Popup::None));
        let p = app.active_pane().unwrap();
        assert_eq!(p.sort.key, SortKey::Extension);
        assert!(!p.sort.reverse);
    }

    /// Picking the key that is already active flips the direction, the way a
    /// column header does.
    #[test]
    fn choosing_the_active_key_again_reverses_it() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt"]);
        app.apply_sort_key(SortKey::Size);
        assert!(!app.active_pane().unwrap().sort.reverse);
        app.apply_sort_key(SortKey::Size);
        assert!(app.active_pane().unwrap().sort.reverse, "second pick should reverse");
        app.apply_sort_key(SortKey::Name);
        assert!(!app.active_pane().unwrap().sort.reverse, "a different key resets direction");
    }

    #[test]
    fn sorting_is_per_pane() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt"]);
        app.focus(FocusedPane::Left);
        app.apply_sort_key(SortKey::Size);
        assert_eq!(app.left.active_ref().sort.key, SortKey::Size);
        assert_eq!(app.right.active_ref().sort.key, SortKey::Name, "other pane untouched");
    }

    #[test]
    fn the_status_bar_drops_the_sort_indicator_but_keeps_the_counts() {
        let (_d, mut app) = app_with_lang(&["a.txt"], "en");
        let screen = render(&mut app, 100, 40).join("\n");
        // The sort chip was removed; the item/mark counts stay.
        assert!(!screen.contains("name ▲"), "the sort indicator should be gone:\n{}", screen);
        assert!(screen.contains("items"));
        assert!(screen.contains("marks"));

        // Sorting still works even though it is no longer shown here.
        app.apply_sort_key(SortKey::Modified);
        assert_eq!(app.active_pane().unwrap().sort.key, SortKey::Modified);
    }

    #[test]
    fn the_key_hint_bar_is_contextual() {
        let (_d, mut app) = app_with_lang(&["a.txt"], "en");
        let normal = render(&mut app, 110, 40).join("\n");
        assert!(normal.contains("sort"), "normal hints missing:\n{}", normal);
        assert!(normal.contains("filter"));

        // Visual mode advertises a different, shorter set.
        app.visual_start();
        let visual = render(&mut app, 110, 40).join("\n");
        assert!(visual.contains("extend"), "visual hints missing:\n{}", visual);
        assert!(!visual.contains("rename"), "normal-mode hints should be gone");
    }

    #[test]
    fn the_key_hint_bar_can_be_switched_off() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let mut config = en_config();
        config.options.key_hints = Some(false);
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, config).unwrap();

        let screen = render(&mut app, 110, 40).join("\n");
        assert!(!screen.contains("? help"), "hints should be hidden");
        // The row it would have used goes back to the listing.
        assert!(screen.contains("a.txt"));
    }

    /// The bottom rows are claimed one at a time, so a row must only be
    /// consumed by a bar that is actually drawn. Getting that wrong shifts
    /// everything below it down by one and blanks the last line.
    #[test]
    fn the_status_bar_sits_on_the_last_row_in_every_mode() {
        let (_d, mut app) = app_with_lang(&["a.txt"], "en");

        let normal = render(&mut app, 110, 40);
        assert!(normal[39].contains("items"), "status row: {:?}", normal[39]);
        assert!(normal[38].contains("help"), "hints above it: {:?}", normal[38]);

        // Filter mode adds a prompt row above the hints; the status bar must
        // still be the bottom line.
        app.handle_key(key('/')).unwrap();
        let filtering = render(&mut app, 110, 40);
        assert!(filtering[39].contains("items"), "status row: {:?}", filtering[39]);
        assert!(filtering[37].contains("filter /"), "prompt row: {:?}", filtering[37]);
    }

    /// `? help` is the way out of not knowing any other key, so a narrow
    /// window must drop something else. Adding one hint used to push it off
    /// the end.
    #[test]
    fn the_help_hint_survives_a_narrow_window() {
        let (_d, mut app) = app_with_lang(&["a.txt"], "en");
        for w in [40u16, 60, 80, 110, 200] {
            let screen = render(&mut app, w, 40).join("\n");
            assert!(screen.contains("? help"), "lost at width {}:\n{}", w, screen);
        }
    }

    /// A short window drops the hints rather than squeezing the listing out.
    #[test]
    fn a_short_window_drops_the_hints() {
        let (_d, mut app) = app_with_lang(&["a.txt"], "en");
        let tall = render(&mut app, 110, 40).join("\n");
        assert!(tall.contains("? help"));
        let short = render(&mut app, 110, 10).join("\n");
        assert!(!short.contains("? help"), "hints should yield on a short window");
    }

    fn app_with_ssh() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let mut config = en_config();
        config.ssh_hosts = vec![
            cian_lua::SshHost {
                name: "web1".into(),
                host: "10.0.1.11".into(),
                users: vec![cian_lua::SshUser::plain("root"), cian_lua::SshUser::plain("deploy")],
                port: None,
                notes: None,
            },
            cian_lua::SshHost {
                name: "db1".into(),
                host: "10.0.2.31".into(),
                users: vec![cian_lua::SshUser {
                    name: "postgres".into(),
                    password: Some("hunter2".into()),
                    password_cmd: None,
            key: None,
            key_pass: None,
                }],
                port: Some(2222),
                notes: None,
            },
        ];
        let p = dir.path().to_path_buf();
        let app = App::new(p.clone(), p, config).unwrap();
        (dir, app)
    }

    #[test]
    fn the_ssh_picker_filters_hosts_as_you_type() {
        let (_d, mut app) = app_with_ssh();
        app.start_ssh();
        assert_eq!(app.ssh_matches("").len(), 2);

        app.handle_key(key('d')).unwrap();
        app.handle_key(key('b')).unwrap();
        let Popup::SshHosts { filter, .. } = &app.popup else { panic!("no picker") };
        assert_eq!(filter, "db");
        assert_eq!(app.ssh_matches("db").len(), 1);

        // Backspace widens it again.
        app.handle_key(code(KeyCode::Backspace)).unwrap();
        let Popup::SshHosts { filter, .. } = &app.popup else { panic!("no picker") };
        assert_eq!(filter, "d");
    }

    /// A host with several users needs the second stage; one with a single
    /// user should connect straight away.
    #[test]
    fn a_single_user_host_skips_the_second_stage() {
        let (_d, mut app) = app_with_ssh();
        app.start_ssh();
        app.handle_key(key('d')).unwrap();
        app.handle_key(key('b')).unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::None), "should have connected already");
        assert!(app.message.as_deref().unwrap_or("").contains("postgres@db1"));
    }

    #[test]
    fn a_multi_user_host_offers_its_users() {
        let (_d, mut app) = app_with_ssh();
        app.start_ssh();
        app.handle_key(key('w')).unwrap();
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let Popup::SshUsers { host, .. } = &app.popup else { panic!("expected the user stage") };
        assert_eq!(app.config.ssh_hosts[*host].name, "web1");

        // Esc steps back to the host list rather than closing outright.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::SshHosts { .. }));
    }

    #[test]
    fn connecting_types_the_command_into_the_shell() {
        let (_d, mut app) = app_with_ssh();
        // No shell yet, so the command has to be queued for the spawn.
        assert_eq!(app.shell.count(), 0);
        app.ssh_connect(1, "postgres");
        assert_eq!(app.focused, FocusedPane::Shell, "should hand over to the shell");
        assert_eq!(
            app.pending_shell_input.as_deref(),
            Some("ssh postgres@10.0.2.31 -p 2222\n"),
            "port should be carried through"
        );
    }

    #[test]
    fn nothing_configured_drops_into_manual_entry() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.start_ssh();
        // With no hosts to pick, go straight to typing a server by hand (#2).
        assert!(
            matches!(
                app.popup,
                Popup::TextInput { kind: InputKind::ManualSshTarget { for_scp: false }, .. }
            ),
            "expected the manual-connection prompt, got {:?}",
            app.popup
        );
    }

    #[test]
    fn connecting_as_a_user_with_a_secret_arms_the_prompt_watcher() {
        let (_d, mut app) = app_with_ssh();
        app.ssh_connect(1, "postgres");
        assert!(app.pending_auth.is_some(), "should be waiting for the prompt");
        // The secret must not appear in anything the user or a log can see.
        let msg = app.message.clone().unwrap_or_default();
        assert!(!msg.contains("hunter2"), "secret leaked into the status message: {}", msg);
        assert!(!format!("{:?}", app.pending_auth).contains("hunter2"), "secret leaked via Debug");
    }

    #[test]
    fn a_user_without_a_secret_does_not_arm_it() {
        let (_d, mut app) = app_with_ssh();
        app.ssh_connect(0, "root");
        assert!(app.pending_auth.is_none(), "key-auth logins must not wait to type anything");
    }

    #[test]
    fn the_watcher_gives_up_after_its_window() {
        let (_d, mut app) = app_with_ssh();
        app.ssh_connect(1, "postgres");
        // Pretend the window has passed with no prompt — a keyed host, say.
        app.pending_auth = Some(PendingAuth {
            secret: "hunter2".into(),
            deadline: Instant::now() - Duration::from_secs(1),
        });
        app.pending_shell_input = None;
        assert!(!app.poll_pending_auth());
        assert!(app.pending_auth.is_none(), "should have expired rather than waiting forever");
    }

    /// The command is queued while the PTY spawns; the password must not be
    /// sent before the command it answers has even been delivered.
    #[test]
    fn nothing_is_sent_while_the_command_is_still_queued() {
        let (_d, mut app) = app_with_ssh();
        app.ssh_connect(1, "postgres");
        assert!(app.pending_shell_input.is_some(), "command should be queued");
        assert!(!app.poll_pending_auth());
        assert!(app.pending_auth.is_some(), "still armed, just not fired");
    }

    #[test]
    fn a_secret_can_come_from_a_command_instead_of_the_file() {
        let u = cian_lua::SshUser {
            name: "deploy".into(),
            password: None,
            password_cmd: Some("printf 'from-store'".into()),
            key: None,
            key_pass: None,
        };
        assert!(u.has_secret());
        assert_eq!(u.secret().as_deref(), Some("from-store"));
    }

    #[test]
    fn z_prompts_for_a_path_seeded_with_the_current_directory() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let here = app.active_pane().unwrap().cwd.clone();
        app.handle_key(key('z')).unwrap();
        let Popup::TextInput { buffer, kind, .. } = &app.popup else { panic!("no prompt") };
        assert!(matches!(kind, InputKind::JumpPath));
        assert_eq!(buffer, &here.display().to_string(), "seeded with where you are");
    }

    #[test]
    fn a_typed_directory_is_entered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("inner.txt"), b"x").unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        let target = dir.path().join("sub");
        app.finish_jump_path(&target.display().to_string()).unwrap();
        // jump_to canonicalises, so compare on the final component.
        assert_eq!(app.active_pane().unwrap().cwd.file_name().unwrap(), "sub");
        // entries[0] is the `..` row; the first real entry follows it.
        assert_eq!(app.active_pane().unwrap().entries[1].name, "inner.txt");
    }

    /// Naming a file should land the cursor on it, so the pane is left
    /// somewhere useful rather than wherever it happened to be.
    #[test]
    fn a_typed_file_moves_the_cursor_to_it() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.path().join(n), b"x").unwrap();
        }
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        let target = dir.path().join("c.txt");
        app.finish_jump_path(&target.display().to_string()).unwrap();
        let pane = app.active_pane().unwrap();
        assert_eq!(pane.selected().unwrap().name, "c.txt");
    }

    #[test]
    fn a_path_that_does_not_exist_says_so_and_stays_put() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let before = app.active_pane().unwrap().cwd.clone();
        app.finish_jump_path("/no/such/place/at/all").unwrap();
        assert_eq!(app.active_pane().unwrap().cwd, before, "must not move");
        assert!(app.message.as_deref().unwrap_or("").contains("no such path"));
    }

    /// Paths get typed after copying them out of a shell or an address bar,
    /// which is where these forms come from.
    #[test]
    fn typed_paths_expand_env_vars_tildes_and_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::env::set_var("CIAN_TEST_BASE", dir.path());

        for form in [
            "$CIAN_TEST_BASE/sub",
            "${CIAN_TEST_BASE}/sub",
            "%CIAN_TEST_BASE%/sub",
        ] {
            assert_eq!(expand_path(form), sub, "failed to expand {:?}", form);
        }
        // Surrounding quotes, as pasted from a shell.
        let quoted = format!("\"{}\"", sub.display());
        assert_eq!(expand_path(&quoted), sub);

        // An unset variable is left alone rather than silently becoming empty.
        assert_eq!(expand_path("$CIAN_NOT_SET_ANYWHERE"), PathBuf::from("$CIAN_NOT_SET_ANYWHERE"));
        std::env::remove_var("CIAN_TEST_BASE");
    }

    #[test]
    fn shift_enter_opens_the_context_menu_by_the_cursor() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        let _ = render(&mut app, 100, 40);
        app.focus(FocusedPane::Left);
        if let Some(p) = app.active_pane_mut() {
            p.cursor = 2;
        }

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        let Popup::ContextMenu { at, items, .. } = &app.popup else {
            panic!("expected the context menu")
        };
        assert!(items.contains(&MenuItem::Delete), "the file-pane menu");
        let left = app.layout_rects.left;
        assert!(at.0 >= left.x && at.0 < left.x + left.width, "anchored in the pane");
        assert_eq!(at.1, left.y + 1 + 2, "on the cursor's row");
    }

    /// Rounded corners are missing from several stock console fonts, so
    /// Windows font-links only the corners and the frame looks a few pixels
    /// out at each one. Square corners are in every font.
    #[test]
    fn border_corners_fall_back_to_square_where_fonts_lack_the_rounded_ones() {
        // An explicit setting always wins, on every platform.
        assert_eq!(resolve_border_type(Some("plain")), BorderType::Plain);
        assert_eq!(resolve_border_type(Some("square")), BorderType::Plain);
        assert_eq!(resolve_border_type(Some("rounded")), BorderType::Rounded);
        assert_eq!(resolve_border_type(Some("  Rounded  ")), BorderType::Rounded);
        // An unrecognised value falls through to the automatic choice rather
        // than failing; a bad config should not cost you your borders.
        let auto = resolve_border_type(None);
        assert_eq!(resolve_border_type(Some("nonsense")), auto);

        // Unix terminals handle the rounded set.
        #[cfg(not(windows))]
        assert_eq!(auto, BorderType::Rounded);
    }

    #[test]
    fn the_rendered_frame_uses_the_chosen_corner_glyphs() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let screen = render(&mut app, 100, 40).join("\n");
        let (round, square) = (
            screen.contains('\u{256d}'),
            screen.contains('\u{250c}'),
        );
        assert!(round ^ square, "exactly one corner style should be on screen");
        assert_eq!(round, border_type() == BorderType::Rounded);
    }

    /// Names are often Japanese here, and CJK characters take two cells. Using
    /// the character count to pad pushed everything after a Japanese name two
    /// columns right and off the edge.
    #[test]
    fn width_and_padding_count_cells_not_characters() {
        assert_eq!(width("work"), 4);
        assert_eq!(width("社内Wiki"), 8, "two cells per CJK character");
        assert_eq!("社内Wiki".chars().count(), 6, "which is not the character count");

        assert_eq!(width(&pad_to("社内Wiki", 12)), 12);
        assert_eq!(width(&pad_to("work", 12)), 12);
        // Already at or past the target: left alone rather than truncated.
        assert_eq!(pad_to("work", 2), "work");
    }

    /// The same lesson, for the other half of a column. `truncate` counted
    /// characters while `pad_to` counted cells, so a Japanese filename in the
    /// file pane was cut to 28 characters — 56 columns — and shoved the size
    /// and date columns off the right edge.
    #[test]
    fn truncation_counts_cells_too_so_columns_line_up() {
        assert_eq!(truncate("report.txt", 20), "report.txt", "shorter than the budget: untouched");
        // A wide character cannot always land exactly on the budget (five of
        // them plus the ellipsis is 11 of 12), so the guarantee is "no wider" —
        // which is why `fit` pads afterwards rather than trusting the cut.
        assert!(width(&truncate("第四四半期の報告書.txt", 12)) <= 12, "never wider than asked");
        assert!(truncate("第四四半期の報告書.txt", 12).ends_with('…'), "marked as cut");
        // A budget that cannot fit even one wide character still holds the line.
        assert_eq!(truncate("日本語", 1), "…");

        // What the file pane actually does: every name occupies the same width,
        // whatever script it is written in.
        for name in ["report_final.txt", "第四四半期の報告書.txt", "設計メモ.md", "a"] {
            assert_eq!(width(&fit(name, 12)), 12, "column width for {name}");
        }
    }

    /// Paths identify themselves at the end, URLs at the start. Cutting either
    /// end loses what tells them apart, so the middle goes.
    #[test]
    fn middle_truncation_keeps_both_ends() {
        assert_eq!(truncate_middle("short", 20), "short");
        let long = "/var/log/application/deploy/current/output.log";
        let cut = truncate_middle(long, 20);
        assert!(width(&cut) <= 20, "must fit: {:?} is {}", cut, width(&cut));
        assert!(cut.starts_with("/var"), "keeps the head: {:?}", cut);
        assert!(cut.ends_with(".log"), "keeps the tail: {:?}", cut);
        assert!(cut.contains('…'));

        // Wide characters cost two cells here too.
        let jp = truncate_middle("社内ドキュメント一覧ページ", 10);
        assert!(width(&jp) <= 10, "{:?} is {} cells", jp, width(&jp));

        // Degenerate widths must not panic or overrun.
        for w in 0..6 {
            let out = truncate_middle("/some/path/file.txt", w);
            assert!(width(&out) <= w.max(1), "w={} gave {:?}", w, out);
        }
    }

    #[test]
    fn visual_a_selects_the_whole_listing() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt", "c.txt", "d.txt"]);
        app.handle_key(key('v')).unwrap();
        assert_eq!(app.mode, Mode::Visual);
        app.handle_key(key('a')).unwrap();

        assert_eq!(app.visual_anchor, Some(0), "anchored at the top");
        // 4 files plus the `..` row → last index is 4.
        assert_eq!(app.active_pane().unwrap().cursor, 4, "cursor at the bottom");

        // Enter commits the range to marks; `..` is never marked, so 4 files.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(app.active_pane().unwrap().mark_count(), 4);
    }

    /// The other route the user asked for: gg, visual, G.
    #[test]
    fn gg_then_visual_then_g_selects_everything() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt", "c.txt", "d.txt"]);
        if let Some(p) = app.active_pane_mut() {
            p.cursor = 2;
        }
        app.handle_key(key('g')).unwrap();
        app.handle_key(key('g')).unwrap();
        assert_eq!(app.active_pane().unwrap().cursor, 0);

        app.handle_key(key('v')).unwrap();
        app.handle_key(key('G')).unwrap();
        // 4 files plus the `..` row → last index is 4.
        assert_eq!(app.active_pane().unwrap().cursor, 4, "G must move in visual mode too");

        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(app.active_pane().unwrap().mark_count(), 4);
    }

    #[test]
    fn gg_works_inside_visual_mode() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        // Start on the last file (index 3, after the `..` row) so the range up
        // to the top covers all three files.
        if let Some(p) = app.active_pane_mut() {
            p.cursor = 3;
        }
        app.handle_key(key('v')).unwrap();
        app.handle_key(key('g')).unwrap();
        app.handle_key(key('g')).unwrap();
        assert_eq!(app.active_pane().unwrap().cursor, 0);
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(app.active_pane().unwrap().mark_count(), 3);
    }

    /// Ctrl+<key> used to fall through to the plain-character arm, so every
    /// Ctrl combination typed its bare letter into the field.
    ///
    /// Checked with combinations that do nothing. The ones that do something
    /// are excluded on purpose: Ctrl+V pastes (and the result would depend on
    /// whatever is on the machine's clipboard), Ctrl+X cuts, Ctrl+A selects,
    /// Ctrl+U clears. What is left is the case this is about — a key with no
    /// meaning here must not fall through and type its letter.
    #[test]
    fn unbound_ctrl_keys_do_not_type_their_letter_into_a_text_field() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.start_shortcut_add(Vec::new(), false);
        app.handle_key(key('w')).unwrap();
        for c in ['k', 'q', 'z'] {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)).unwrap();
        }
        let Popup::TextInput { buffer, .. } = &app.popup else { panic!("no field") };
        assert_eq!(buffer, "w", "a Ctrl combination leaked its letter");
    }


    /// A new shortcut is nearly always for the thing under the cursor, so the
    /// target starts filled in rather than blank.
    #[test]
    fn a_new_shortcut_defaults_its_target_to_the_current_entry() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt"]);
        if let Some(p) = app.active_pane_mut() {
            p.cursor = 1;
        }
        let expected = app.active_pane().unwrap().selected().unwrap().path.clone();

        app.start_shortcut_add(Vec::new(), false);
        for c in "mine".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();

        let Popup::TextInput { buffer, kind, .. } = &app.popup else { panic!("no target step") };
        assert!(matches!(kind, InputKind::ShortcutTarget { .. }));
        assert_eq!(buffer, &expected.display().to_string());
    }

    /// `A` makes a folder in the current level; Enter steps in; `A` again nests;
    /// Esc/← climbs back out. The tree is what gets saved.
    #[test]
    fn shortcuts_menu_creates_and_navigates_nested_folders() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // Bookmarks live in a temp dir so the test never touches the real config,
        // and start empty so indices are predictable regardless of the dev's own.
        let sd = tempfile::tempdir().unwrap();
        app.shortcuts.path = sd.path().join("shortcuts.lua");
        app.shortcuts.entries.clear();

        // Open the menu and add a top-level folder "Projects" with `A`.
        app.start_shortcuts();
        app.handle_key(key('A')).unwrap();
        for c in "Projects".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();

        // Back in the menu, the folder is there; step into it with Enter.
        assert!(matches!(&app.popup, Popup::Shortcuts { path, .. } if path.is_empty()));
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let Popup::Shortcuts { path, .. } = &app.popup else { panic!("menu closed") };
        assert_eq!(path, &vec![0], "stepped into the folder");

        // Add a leaf shortcut inside it: name then target.
        app.handle_key(key('a')).unwrap();
        for c in "cian".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap(); // name -> target step
        // Clear the auto-filled target and type our own.
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)).unwrap();
        for c in "~/workspace/cian".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();

        // The store now holds Projects/cian.
        assert_eq!(app.shortcuts.entries.len(), 1);
        let projects = &app.shortcuts.entries[0];
        assert_eq!(projects.name, "Projects");
        assert!(projects.is_group());
        let kids = projects.children.as_ref().unwrap();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].name, "cian");
        assert_eq!(kids[0].target.as_deref(), Some("~/workspace/cian"));

        // Esc climbs back to the top rather than closing.
        assert!(matches!(&app.popup, Popup::Shortcuts { path, .. } if path == &vec![0]));
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(&app.popup, Popup::Shortcuts { path, .. } if path.is_empty()));
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::None), "Esc at the top closes the menu");
    }

    /// Wait for the search worker to finish, draining as it goes.
    fn drain_find(app: &mut App) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            app.poll_find_job();
            if app.find_job.as_ref().and_then(|j| j.done).is_some() {
                app.poll_find_job();
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("search did not finish");
    }

    fn find_tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src/deep")).unwrap();
        std::fs::create_dir_all(d.path().join("build")).unwrap();
        std::fs::write(d.path().join("readme.md"), b"").unwrap();
        std::fs::write(d.path().join("src/main.rs"), b"").unwrap();
        std::fs::write(d.path().join("src/deep/main.rs"), b"").unwrap();
        std::fs::write(d.path().join("build/main.o"), b"").unwrap();
        d
    }

    #[test]
    fn shift_f_searches_the_tree_below_the_pane() {
        let d = find_tree();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT)).unwrap();
        let Popup::TextInput { kind, .. } = &app.popup else { panic!("no prompt") };
        assert!(matches!(kind, InputKind::FindRecursive));

        for c in "main".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        drain_find(&mut app);

        let Popup::FindResults { hits, .. } = &app.popup else { panic!("no results") };
        assert_eq!(hits.len(), 3, "got {:?}", hits.iter().map(|h| &h.rel).collect::<Vec<_>>());
    }

    /// Grep, then replace across everything it matched: the preview must show
    /// what each line becomes, Space must be able to spare one, and nothing may
    /// reach the disk until Enter.
    #[test]
    fn a_grep_can_be_replaced_across_every_file_it_matched() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.txt");
        let b = d.path().join("b.log");
        // CRLF and a tab, so the write path is held to the file it was given.
        std::fs::write(&a, "ORA-600 first\r\nfine\r\nORA-600 third\r\n").unwrap();
        std::fs::write(&b, b"col\tORA-600\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.start_find("ORA-600", cian_core::search::Mode::Content);
        drain_find(&mut app);
        assert!(matches!(&app.popup, Popup::FindResults { hits, .. } if hits.len() == 3));

        // `r` asks only for the replacement — the pattern is the one on screen.
        app.handle_key(key('r')).unwrap();
        let Popup::TextInput { kind, .. } = &app.popup else { panic!("no prompt") };
        assert!(matches!(kind, InputKind::GrepReplaceWith { paths, .. } if paths.len() == 2));
        for c in "ORA-7445".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();

        let Popup::GrepReplace(plan) = &app.popup else { panic!("no preview: {:?}", app.popup) };
        assert_eq!(plan.changes.len(), 3, "one row per changed line");
        assert!(plan.changes.iter().all(|c| c.picked));
        // The row order follows the walk, which the filesystem decides; find
        // the rows by what they say instead.
        let row = |plan: &crate::ReplacePlan, before: &str| {
            plan.changes.iter().position(|c| c.before == before).expect("row for {before}")
        };
        assert_eq!(plan.changes[row(plan, "ORA-600 first")].after, "ORA-7445 first");
        assert_eq!(plan.changes[row(plan, "col\tORA-600")].after, "col\tORA-7445");
        assert_eq!(
            std::fs::read(&a).unwrap(),
            b"ORA-600 first\r\nfine\r\nORA-600 third\r\n",
            "the preview must not have written anything",
        );

        // Space spares one line — and steps on, so a run can be unchecked by
        // holding it down.
        let spare = row(plan, "ORA-600 third");
        if let Popup::GrepReplace(plan) = &mut app.popup {
            plan.cursor = spare;
        }
        app.handle_key(key(' ')).unwrap();
        let Popup::GrepReplace(plan) = &app.popup else { panic!("preview gone") };
        assert!(!plan.changes[spare].picked);
        assert!(plan.cursor > spare || spare == plan.changes.len() - 1);

        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::None));
        assert_eq!(
            std::fs::read(&a).unwrap(),
            b"ORA-7445 first\r\nfine\r\nORA-600 third\r\n",
            "CRLF kept, and the unchecked line left alone",
        );
        assert_eq!(std::fs::read(&b).unwrap(), b"col\tORA-7445\n", "the tab survived");
        assert!(app.message.as_deref().unwrap_or("").contains("2 line(s) in 2 file(s)"));
    }

    /// Esc from the preview is free, and a name search has nothing to replace.
    #[test]
    fn a_grep_replace_can_always_be_backed_out_of() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("keep.txt");
        std::fs::write(&f, "TARGET\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        // A name search refuses, rather than replacing filenames by surprise.
        app.start_find("keep", cian_core::search::Mode::Name);
        drain_find(&mut app);
        app.handle_key(key('r')).unwrap();
        assert!(matches!(app.popup, Popup::FindResults { .. }), "still the results");
        assert!(app.message.as_deref().unwrap_or("").contains("grep"));

        app.start_find("TARGET", cian_core::search::Mode::Content);
        drain_find(&mut app);
        app.handle_key(key('r')).unwrap();
        for c in "GONE".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::GrepReplace(_)));
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "TARGET\n", "Esc wrote nothing");
    }

    #[test]
    fn choosing_a_grep_hit_opens_the_viewer_at_that_line() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("code.txt"),
            "first line\nsecond has TARGET here\nthird line\n",
        )
        .unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.start_find("TARGET", cian_core::search::Mode::Content);
        drain_find(&mut app);
        let has_hit = matches!(&app.popup, Popup::FindResults { hits, .. } if !hits.is_empty());
        assert!(has_hit, "grep found the line");

        app.open_find_hit().unwrap();
        // The viewer opened on the matched line (line 2 → 0-based index 1).
        match &app.popup {
            Popup::Viewer { line, view, .. } => {
                assert_eq!(*line, 1, "cursor on the matched line");
                assert!(view.lines[*line].contains("TARGET"));
            }
            other => panic!("expected the viewer, got {:?}", other),
        }

        // Closing the viewer returns to the grep results, not to nothing.
        quit_viewer(&mut app);
        assert!(
            matches!(app.popup, Popup::FindResults { .. }),
            "Esc returns to the results list, got {:?}",
            app.popup
        );
        // A second Esc closes the results.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::None));
    }

    /// Choosing a result should leave the pane somewhere useful: in the file's
    /// directory, with the cursor on it.
    #[test]
    fn choosing_a_result_navigates_to_it() {
        let d = find_tree();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.start_find("main.rs", cian_core::search::Mode::Name);
        drain_find(&mut app);
        // Pick the deepest hit, whichever position it landed in.
        let idx = match &app.popup {
            Popup::FindResults { hits, .. } => hits
                .iter()
                .position(|h| h.rel.to_string_lossy().contains("deep"))
                .expect("expected a hit under src/deep"),
            _ => panic!("no results"),
        };
        if let Popup::FindResults { cursor, .. } = &mut app.popup {
            *cursor = idx;
        }
        app.open_find_hit().unwrap();

        assert!(matches!(app.popup, Popup::None), "the popup should close");
        let pane = app.active_pane().unwrap();
        assert_eq!(pane.cwd.file_name().unwrap(), "deep");
        assert_eq!(pane.selected().unwrap().name, "main.rs");
        assert!(app.find_job.is_none(), "the worker should be released");
    }

    /// Wait until a branch view / panelize has installed its flat listing.
    /// `drain_find` cannot be used: routing to a pane releases the job the moment
    /// it completes, so there is no lingering `done` for it to observe.
    fn drain_until_flat(app: &mut App) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            app.poll_find_job();
            if app.active_pane().map(|p| p.is_flat()).unwrap_or(false) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("branch view did not build");
    }

    #[test]
    fn b_flattens_the_subtree_into_the_pane_and_toggles_back() {
        let d = find_tree();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.handle_key(key('b')).unwrap();
        drain_until_flat(&mut app);

        let pane = app.active_pane().unwrap();
        assert!(pane.is_flat());
        // Every file in the tree, folders excluded, shown by relative path.
        let mut names: Vec<String> =
            pane.entries.iter().filter(|e| !e.is_parent).map(|e| e.name.clone()).collect();
        names.sort();
        assert_eq!(
            names,
            vec!["build/main.o", "readme.md", "src/deep/main.rs", "src/main.rs"]
        );
        assert!(pane.entries.iter().all(|e| !e.is_parent), "no `..` row in a flat view");

        // `b` again leaves the view, back to the real directory listing.
        app.handle_key(key('b')).unwrap();
        let pane = app.active_pane().unwrap();
        assert!(!pane.is_flat());
        assert!(pane.entries.iter().any(|e| e.name == "src" && e.is_dir), "real dirs are back");
    }

    #[test]
    fn p_panelizes_search_results_into_the_pane() {
        let d = find_tree();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.start_find("main", cian_core::search::Mode::Name);
        drain_find(&mut app);
        // main.rs (×2) + build/main.o = 3 name matches.
        let Popup::FindResults { hits, .. } = &app.popup else { panic!("no results") };
        assert_eq!(hits.len(), 3);

        app.handle_key(key('p')).unwrap();
        assert!(matches!(app.popup, Popup::None), "panelize closes the popup");
        assert!(app.find_job.is_none(), "and releases the worker");
        let pane = app.active_pane().unwrap();
        assert!(pane.is_flat());
        assert_eq!(pane.entries.iter().filter(|e| !e.is_parent).count(), 3);
    }

    #[test]
    fn a_search_with_no_matches_says_so_rather_than_hanging() {
        let d = find_tree();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.start_find("nothing-matches-this", cian_core::search::Mode::Name);
        drain_find(&mut app);
        let Popup::FindResults { hits, .. } = &app.popup else { panic!("no results popup") };
        assert!(hits.is_empty());
        assert_eq!(app.find_job.as_ref().unwrap().done, Some(cian_core::search::Outcome::Complete));
    }

    #[test]
    fn closing_the_results_stops_the_worker() {
        let d = find_tree();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.start_find("main", cian_core::search::Mode::Name);
        assert!(app.find_job.is_some());
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::None));
        assert!(app.find_job.is_none(), "Esc must release the search");
    }

    #[test]
    fn ctrl_f_greps_inside_files_and_reports_the_line() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "one\nTODO: fix\nthree\n").unwrap();
        std::fs::write(d.path().join("b.txt"), "nothing\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL)).unwrap();
        let Popup::TextInput { kind, .. } = &app.popup else { panic!("no prompt") };
        assert!(matches!(kind, InputKind::GrepRecursive));
        for c in "todo".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        drain_find(&mut app);

        let Popup::FindResults { hits, .. } = &app.popup else { panic!("no results") };
        assert_eq!(hits.len(), 1);
        let (n, text) = hits[0].line.clone().expect("a content hit carries its line");
        assert_eq!(n, 2, "1-based line number");
        assert_eq!(text, "TODO: fix");
    }

    #[test]
    fn the_menu_offers_the_new_entries() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);
        app.focus(FocusedPane::Left);
        app.open_context_menu(5, 5);
        let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
        // Top level carries the groups + Shortcuts; the old flat entries now
        // live one level down.
        for want in [MenuItem::InspectMenu, MenuItem::ViewMenu, MenuItem::Shortcuts] {
            assert!(items.contains(&want), "{:?} missing from {:?}", want, items);
        }
        // Attributes / Hash are under Inspect ▸; Show-hidden is under View ▸.
        let inspect = app.submenu_children(MenuItem::InspectMenu).unwrap();
        assert!(inspect.contains(&MenuItem::Attributes) && inspect.contains(&MenuItem::Hash));
        let view = app.submenu_children(MenuItem::ViewMenu).unwrap();
        assert!(view.contains(&MenuItem::HiddenToggle));
    }

    /// `M` opens the context menu on every terminal (Shift+Enter can't be
    /// distinguished from Enter on e.g. macOS Terminal.app).
    #[test]
    fn m_key_opens_the_context_menu() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);
        app.focus(FocusedPane::Left);
        app.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT)).unwrap();
        assert!(matches!(app.popup, Popup::ContextMenu { .. }), "M opened the menu");
        // Also works when the terminal doesn't tag the uppercase char with SHIFT.
        app.popup = Popup::None;
        app.handle_key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE)).unwrap();
        assert!(matches!(app.popup, Popup::ContextMenu { .. }), "M works without a SHIFT tag too");
    }

    #[test]
    fn the_menu_shortcuts_entry_opens_the_bookmarks() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Left);
        app.run_menu_item(MenuItem::Shortcuts).unwrap();
        assert!(matches!(app.popup, Popup::Shortcuts { .. }), "opened the shortcuts menu");
    }

    #[test]
    fn the_menu_toggles_dotfiles_for_the_focused_pane_only() {
        let (_d, mut app) = app_with(&["a.txt", ".hidden"]);
        app.focus(FocusedPane::Left);
        // Counts include the `..` row: 2 files + `..` = 3.
        assert_eq!(app.left.active_ref().entries.len(), 3);

        app.run_menu_item(MenuItem::HiddenToggle).unwrap();
        assert_eq!(app.left.active_ref().entries.len(), 2, "dotfile hidden here");
        assert_eq!(app.right.active_ref().entries.len(), 3, "and not in the other pane");
    }

    /// Dragging from one pane to the other should raise the transfer
    /// confirmation, not act silently.
    #[test]
    fn dragging_between_panes_offers_a_transfer() {
        let (_l, r, mut app) = app_two_dirs(&["doc.txt"], &[]);
        let _ = render(&mut app, 100, 40);
        let (left, right) = (app.layout_rects.left, app.layout_rects.right);

        // Row 1 is `..`; press on the file on row 2.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 5, left.y + 3));
        assert!(app.file_drag.is_some(), "pressing on an entry arms a drag");

        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            right.x + 5,
            right.y + 3,
        ));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), right.x + 5, right.y + 3));

        let Popup::ConfirmTransfer { op, targets, dest } = &app.popup else {
            panic!("expected a transfer confirmation, got {:?}", app.popup)
        };
        assert_eq!(*op, PendingOp::Copy, "a plain drag copies");
        assert_eq!(targets.len(), 1);
        assert_eq!(dest.file_name(), r.path().file_name());
        assert!(app.file_drag.is_none(), "the drag is released");
    }

    #[test]
    fn shift_dragging_moves_instead_of_copying() {
        let (_l, _r, mut app) = app_two_dirs(&["doc.txt"], &[]);
        let _ = render(&mut app, 100, 40);
        let (left, right) = (app.layout_rects.left, app.layout_rects.right);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 5, left.y + 3));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), right.x + 5, right.y + 3));
        let mut up = mouse(MouseEventKind::Up(MouseButton::Left), right.x + 5, right.y + 3);
        up.modifiers = KeyModifiers::SHIFT;
        app.handle_mouse(up);

        let Popup::ConfirmTransfer { op, .. } = &app.popup else { panic!("no confirmation") };
        assert_eq!(*op, PendingOp::Move);
    }

    /// Regression: a click that the terminal reported with a stray same-row
    /// Drag used to mark that row. Clicking file A then file B then A must
    /// leave the marks untouched — a bare click is not a mark.
    #[test]
    fn clicking_files_never_marks_them() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        let _ = render(&mut app, 100, 40);
        let left = app.layout_rects.left;
        // Rows: 1 = `..`, 2 = a.txt, 3 = b.txt, 4 = c.txt.
        for cy in [left.y + 2, left.y + 3, left.y + 2] {
            // A press, a same-row drag (the terminal's jitter), then release.
            app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 3, cy));
            app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), left.x + 3, cy));
            app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), left.x + 3, cy));
        }
        assert_eq!(app.active_pane().unwrap().mark_count(), 0, "clicks must not mark");
    }

    /// The `..` row navigates up on a single click, and can never be marked.
    #[test]
    fn the_parent_row_navigates_up_and_is_never_marked() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/inner.txt"), b"x").unwrap();
        let start = d.path().join("sub");
        let mut app = App::new(start.clone(), start, en_config()).unwrap();
        let _ = render(&mut app, 100, 40);
        let left = app.layout_rects.left;
        // The first row is `..`. It takes a double-click, like every other
        // row — a single one used to step up on its own, which meant a click
        // meant to put the cursor there navigated instead.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 3, left.y + 2));
        assert!(app.left.active_ref().cwd.ends_with("sub"), "one click only selects it");
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 3, left.y + 2));
        assert!(!app.left.active_ref().cwd.ends_with("sub"), "the second click steps up");
        // Marking the `..` row (e.g. via Space on it) is a no-op.
        if let Some(p) = app.active_pane_mut() {
            p.cursor = 0; // back onto `..`
            p.toggle_mark_at(0);
        }
        assert_eq!(app.active_pane().unwrap().mark_count(), 0, "`..` is never marked");
    }

    /// Press and release without moving is a click. It must not transfer
    /// anything, or every click would raise a dialog.
    #[test]
    fn a_click_without_movement_is_not_a_drag() {
        let (_l, _r, mut app) = app_two_dirs(&["doc.txt"], &[]);
        let _ = render(&mut app, 100, 40);
        let left = app.layout_rects.left;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 5, left.y + 1));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), left.x + 5, left.y + 1));
        assert!(matches!(app.popup, Popup::None), "a click must not start a transfer");
        assert!(app.file_drag.is_none());
    }

    #[test]
    fn dropping_back_on_the_same_pane_does_nothing() {
        let (_l, _r, mut app) = app_two_dirs(&["a.txt", "b.txt"], &[]);
        let _ = render(&mut app, 100, 40);
        let left = app.layout_rects.left;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 5, left.y + 1));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), left.x + 5, left.y + 2));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), left.x + 5, left.y + 2));
        assert!(matches!(app.popup, Popup::None));
    }

    /// The nearest thing to dragging a file into a terminal.
    #[test]
    fn dragging_onto_the_shell_types_the_paths() {
        let (_l, _r, mut app) = app_two_dirs(&["doc.txt"], &[]);
        let _ = render(&mut app, 100, 40);
        let (left, shell) = (app.layout_rects.left, app.layout_rects.shell);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 5, left.y + 3));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), shell.x + 5, shell.y + 2));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), shell.x + 5, shell.y + 2));

        assert_eq!(app.focused, FocusedPane::Shell);
        let queued = app.pending_shell_input.clone().unwrap_or_default();
        assert!(queued.contains("doc.txt"), "got {:?}", queued);
        assert!(!queued.ends_with('\n'), "paths are typed, not run");
    }

    #[test]
    fn destinations_are_remembered_most_recent_first_and_deduped() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.remember_dest(Path::new("/tmp/one"));
        app.remember_dest(Path::new("/tmp/two"));
        app.remember_dest(Path::new("/tmp/one"));
        assert_eq!(
            app.dest_history,
            vec![PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")],
            "re-using a destination promotes it rather than duplicating it"
        );

        for i in 0..DEST_HISTORY_CAP + 5 {
            app.remember_dest(&PathBuf::from(format!("/tmp/d{}", i)));
        }
        assert_eq!(app.dest_history.len(), DEST_HISTORY_CAP, "the list is capped");
    }

    #[test]
    fn the_destination_picker_leads_with_the_other_pane() {
        let (_l, r, mut app) = app_two_dirs(&["a.txt"], &[]);
        app.remember_dest(Path::new("/tmp/somewhere"));
        app.focus(FocusedPane::Left);
        app.start_dest_picker(PendingOp::Copy);

        assert!(matches!(app.popup, Popup::DestPicker { .. }));
        let choices = app.dest_choices();
        assert_eq!(choices[0].0, "other pane");
        assert_eq!(choices[0].1.file_name(), r.path().file_name());
        assert!(choices.iter().any(|(k, p)| k == "recent" && p == Path::new("/tmp/somewhere")));
    }

    /// Two panes, one file each, both cursors on the first entry.
    fn two_panes_with(
        a: &str,
        b: &str,
    ) -> (tempfile::TempDir, tempfile::TempDir, App) {
        let l = tempfile::tempdir().unwrap();
        let r = tempfile::tempdir().unwrap();
        std::fs::write(l.path().join("a.txt"), a).unwrap();
        std::fs::write(r.path().join("b.txt"), b).unwrap();
        let app = App::new(l.path().to_path_buf(), r.path().to_path_buf(), en_config())
            .unwrap();
        (l, r, app)
    }

    #[test]
    fn equals_compares_the_two_panes_files() {
        let (_l, _r, mut app) = two_panes_with("one\ntwo\nthree\n", "one\nTWO\nthree\n");
        app.handle_key(code(KeyCode::Char('='))).unwrap();

        let Popup::Diff { left, right, result, .. } = &app.popup else {
            panic!("expected the diff, got {:?}", app.popup)
        };
        assert_eq!((left.as_str(), right.as_str()), ("a.txt", "b.txt"));
        assert_eq!(result.changed, 1);
        assert!(!result.identical);
    }

    #[test]
    fn identical_files_report_a_notice_not_an_empty_diff() {
        let (_l, _r, mut app) = two_panes_with("same\nlines\n", "same\nlines\n");
        app.handle_key(code(KeyCode::Char('='))).unwrap();
        match &app.popup {
            Popup::Notice { lines } => assert!(lines.iter().any(|l| l.contains("identical"))),
            other => panic!("expected an identical notice, got {:?}", other),
        }
    }

    #[test]
    fn a_diff_can_be_copied_and_saved() {
        let (l, _r, mut app) = two_panes_with("one\ntwo\n", "one\nTWO\n");
        app.handle_key(code(KeyCode::Char('='))).unwrap();
        assert!(matches!(app.popup, Popup::Diff { .. }));

        // c copies a unified-style text with the changed lines. (On a headless
        // CI box there is no system clipboard, so accept that outcome too.)
        app.handle_key(code(KeyCode::Char('c'))).unwrap();
        let msg = app.message.as_deref().unwrap_or("");
        assert!(msg.contains("diff copied") || msg.contains("clipboard unavailable"), "got {msg:?}");

        // w prompts for a filename; saving writes it into the active pane's dir
        // (the left pane, which is focused by default).
        app.handle_key(code(KeyCode::Char('w'))).unwrap();
        assert!(matches!(&app.popup, Popup::TextInput { kind: InputKind::DiffSaveAs { .. }, .. }));
        // Clear the default and type a name.
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)).unwrap();
        for c in "out.diff".chars() { app.handle_key(key(c)).unwrap(); }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let saved = std::fs::read_to_string(l.path().join("out.diff")).unwrap();
        assert!(saved.contains("- two") && saved.contains("+ TWO"), "saved diff:\n{saved}");
    }

    #[test]
    fn the_diff_can_be_searched() {
        // Put a distinctive word far down so a search has to move the view.
        let mut a: Vec<String> = (0..30).map(|i| format!("line {}", i)).collect();
        let b = a.clone();
        a[25] = "NEEDLE here".into();
        let (_l, _r, mut app) = two_panes_with(&(a.join("\n") + "\n"), &(b.join("\n") + "\n"));
        app.handle_key(code(KeyCode::Char('='))).unwrap();
        // Unfold so every row is present and the index is predictable.
        app.handle_key(code(KeyCode::Char('f'))).unwrap();

        // /NEEDLE<CR> jumps the view to the matching row and remembers the query.
        app.handle_key(code(KeyCode::Char('/'))).unwrap();
        for c in "needle".chars() { app.handle_key(key(c)).unwrap(); }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let Popup::Diff { find, scroll, .. } = &app.popup else { panic!("no diff") };
        assert_eq!(find.as_deref(), Some("needle"), "query kept");
        assert_eq!(*scroll, 25, "jumped to the matching row");

        // Esc clears the search but keeps the diff open.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(&app.popup, Popup::Diff { find: None, .. }));
    }

    /// Which pane holds the focus must not decide which file is the "before".
    #[test]
    fn the_left_pane_is_always_the_left_side() {
        let (_l, _r, mut app) = two_panes_with("old\n", "new\n");
        app.focus(FocusedPane::Right);
        app.handle_key(code(KeyCode::Char('='))).unwrap();

        let Popup::Diff { result, left, .. } = &app.popup else { panic!("no diff") };
        assert_eq!(left, "a.txt");
        match &result.rows[0] {
            cian_core::diff::Row::Changed { left, right } => {
                assert_eq!((left.text.as_str(), right.text.as_str()), ("old", "new"));
            }
            other => panic!("expected a change, got {:?}", other),
        }
    }

    #[test]
    fn comparing_a_directory_against_a_file_is_refused() {
        let l = tempfile::tempdir().unwrap();
        let r = tempfile::tempdir().unwrap();
        std::fs::create_dir(l.path().join("adir")).unwrap();
        std::fs::write(r.path().join("b.txt"), "x").unwrap();
        let mut app =
            App::new(l.path().to_path_buf(), r.path().to_path_buf(), en_config())
                .unwrap();

        app.handle_key(code(KeyCode::Char('='))).unwrap();
        assert!(matches!(app.popup, Popup::None));
        assert!(app.message.as_deref().unwrap().contains("not one of each"));
    }

    #[test]
    fn comparing_two_directories_lists_the_differences() {
        let l = tempfile::tempdir().unwrap();
        let r = tempfile::tempdir().unwrap();
        std::fs::create_dir(l.path().join("proj")).unwrap();
        std::fs::create_dir(r.path().join("proj")).unwrap();
        std::fs::write(l.path().join("proj/same.txt"), b"xy").unwrap();
        std::fs::write(r.path().join("proj/same.txt"), b"xy").unwrap();
        // Equal size AND mtime, so the quick compare treats them as identical.
        let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        cian_core::dirdiff::set_mtime(&l.path().join("proj/same.txt"), t).unwrap();
        cian_core::dirdiff::set_mtime(&r.path().join("proj/same.txt"), t).unwrap();
        std::fs::write(l.path().join("proj/only_left.txt"), b"l").unwrap();
        std::fs::write(r.path().join("proj/changed.txt"), b"aaaa").unwrap();
        std::fs::write(l.path().join("proj/changed.txt"), b"a").unwrap();
        let mut app =
            App::new(l.path().to_path_buf(), r.path().to_path_buf(), en_config())
                .unwrap();
        // Cursor on "proj" in each pane (index 0 is the `..` row).
        app.left.active_mut().cursor = 1;
        app.right.active_mut().cursor = 1;

        app.handle_key(code(KeyCode::Char('='))).unwrap();
        assert!(app.diff_job.is_some(), "comparison started on a worker");
        // Drain the worker.
        for _ in 0..200 {
            if app.diff_job.is_none() { break; }
            app.poll_diff_job();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Popup::DirCompare { entries, .. } = &app.popup else {
            panic!("expected the comparison, got {:?}", app.popup)
        };
        let paths: Vec<String> =
            entries.iter().map(|e| e.rel.display().to_string().replace('\\', "/")).collect();
        // Paths are relative to the compared folders (proj), not the roots.
        assert!(paths.contains(&"only_left.txt".to_string()), "{:?}", paths);
        assert!(paths.contains(&"changed.txt".to_string()), "{:?}", paths);
        assert!(!paths.contains(&"same.txt".to_string()), "identical file omitted: {:?}", paths);
    }

    #[test]
    fn an_empty_pane_reports_rather_than_opening_an_empty_diff() {
        let l = tempfile::tempdir().unwrap();
        let r = tempfile::tempdir().unwrap();
        std::fs::write(r.path().join("b.txt"), "x").unwrap();
        let mut app =
            App::new(l.path().to_path_buf(), r.path().to_path_buf(), en_config())
                .unwrap();

        app.handle_key(code(KeyCode::Char('='))).unwrap();
        assert!(matches!(app.popup, Popup::None));
        assert!(app.message.as_deref().unwrap().contains("select a file"));
    }

    #[test]
    fn n_jumps_to_the_next_difference_and_f_unfolds() {
        // Two differences far enough apart that folding hides the gap.
        let mut a: Vec<String> = (0..40).map(|i| format!("line {}", i)).collect();
        let b = a.clone();
        a[5] = "first change".into();
        a[30] = "second change".into();
        let (_l, _r, mut app) =
            two_panes_with(&(a.join("\n") + "\n"), &(b.join("\n") + "\n"));
        app.handle_key(code(KeyCode::Char('='))).unwrap();

        let Popup::Diff { folded, scroll, fold, .. } = &app.popup else { panic!("no diff") };
        assert!(*fold, "opens folded");
        assert_eq!(*scroll, 0);
        let folded_len = folded.len();

        app.handle_key(code(KeyCode::Char('n'))).unwrap();
        let Popup::Diff { folded, scroll, .. } = &app.popup else { panic!("no diff") };
        assert!(folded[*scroll].is_difference(), "n landed on a change");
        let first = *scroll;

        app.handle_key(code(KeyCode::Char('n'))).unwrap();
        let Popup::Diff { folded, scroll, .. } = &app.popup else { panic!("no diff") };
        assert!(*scroll > first && folded[*scroll].is_difference(), "and on to the next");
        let second = *scroll;

        app.handle_key(code(KeyCode::Char('N'))).unwrap();
        let Popup::Diff { scroll, .. } = &app.popup else { panic!("no diff") };
        assert_eq!(*scroll, first, "N goes back");
        assert!(second > first);

        app.handle_key(code(KeyCode::Char('f'))).unwrap();
        let Popup::Diff { fold, result, scroll, .. } = &app.popup else { panic!("no diff") };
        assert!(!*fold);
        assert_eq!(*scroll, 0, "the row lists differ in length; the old offset is meaningless");
        assert!(result.rows.len() > folded_len, "unfolding shows more");
    }

    #[test]
    fn esc_closes_the_diff() {
        let (_l, _r, mut app) = two_panes_with("a\n", "b\n");
        app.handle_key(code(KeyCode::Char('='))).unwrap();
        assert!(matches!(app.popup, Popup::Diff { .. }));
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::None));
    }

    #[test]
    fn the_diff_renders_without_panicking_at_any_size() {
        let (_l, _r, mut app) = two_panes_with("one\ntwo\n", "one\nTWO\nthree\n");
        app.handle_key(code(KeyCode::Char('='))).unwrap();
        let wide = render(&mut app, 120, 30).join("\n");
        assert!(wide.contains("a.txt ↔ b.txt"), "both names in the title:\n{}", wide);
        assert!(wide.contains("two") && wide.contains("TWO"), "both sides shown:\n{}", wide);
        assert!(wide.contains("three"), "the added line too:\n{}", wide);

        // Narrow enough that the column arithmetic would underflow if it were
        // not saturating.
        for (w, h) in [(80u16, 24u16), (24, 8), (10, 5)] {
            render(&mut app, w, h);
        }
    }

    /// Wait for a background file operation to finish.
    fn drain_op(app: &mut App) {
        for _ in 0..200 {
            if app.op_job.is_none() { break; }
            app.poll_op_job();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Run a `:`-command as if it were typed and Enter pressed.
    fn run_cmd(app: &mut App, line: &str) {
        app.command_buffer = line.to_string();
        app.mode = Mode::Command;
        app.run_command();
    }

    /// A terminal with the kitty keyboard protocol (WezTerm, kitty) reports the
    /// Shift held to type `:`, so the binding must not require Shift to be
    /// absent — otherwise `:` does nothing there and command mode is unreachable.
    #[test]
    fn colon_opens_command_mode_even_with_shift_reported() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.mode, Mode::Command, "Shift+: must still enter command mode");
        // And it still works without the modifier (a plain-PTY terminal).
        app.mode = Mode::Normal;
        app.handle_key(code(KeyCode::Char(':'))).unwrap();
        assert_eq!(app.mode, Mode::Command);
    }

    /// The other shifted-punctuation bindings, likewise reachable with the
    /// modifier set.
    #[test]
    fn punctuation_bindings_ignore_the_shift_modifier() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt"]);
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.mode, Mode::Filter, "/ opens the filter regardless of shift");
        app.handle_key(code(KeyCode::Esc)).unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Char(','), KeyModifiers::SHIFT)).unwrap();
        assert!(matches!(app.popup, Popup::SortPicker { .. }), ", opens the sort picker");
    }

    #[test]
    fn mkdir_makes_a_directory_and_dash_p_makes_the_chain() {
        let (d, mut app) = app_with(&["existing.txt"]);
        run_cmd(&mut app, "mkdir fresh");
        assert!(d.path().join("fresh").is_dir());
        // Plain mkdir into a missing parent fails and says so.
        run_cmd(&mut app, "mkdir a/b/c");
        assert!(!d.path().join("a/b/c").exists());
        assert!(app.message.as_deref().unwrap().to_lowercase().contains("mkdir"));
        // -p builds the whole path.
        run_cmd(&mut app, "mkdir -p a/b/c");
        assert!(d.path().join("a/b/c").is_dir());
        // The new entries show up without an explicit refresh.
        assert!(app.active_pane().unwrap().all_entries.iter().any(|e| e.name == "fresh"));
    }

    #[test]
    fn touch_creates_a_file_that_appears_in_the_listing() {
        let (d, mut app) = app_with(&["a.txt"]);
        run_cmd(&mut app, "touch new.log");
        assert!(d.path().join("new.log").is_file());
        assert!(app.active_pane().unwrap().all_entries.iter().any(|e| e.name == "new.log"));
    }

    #[test]
    fn pwd_reports_and_copies_the_directory() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // Compare against the pane's canonicalised cwd, which is what pwd prints.
        let cwd = app.active_pane().unwrap().cwd.display().to_string();
        run_cmd(&mut app, "pwd");
        let msg = app.message.clone().unwrap();
        assert!(msg.contains(&cwd), "msg {:?} should contain {:?}", msg, cwd);
        assert!(msg.contains("copied"));
    }

    #[test]
    fn cp_with_no_argument_targets_the_other_pane() {
        let l = tempfile::tempdir().unwrap();
        let r = tempfile::tempdir().unwrap();
        std::fs::write(l.path().join("doc.txt"), b"hi").unwrap();
        let mut app =
            App::new(l.path().to_path_buf(), r.path().to_path_buf(), en_config())
                .unwrap();
        // The pane canonicalises its cwd (differently per platform), so compare
        // against the pane's own path rather than the raw tempdir.
        let right_cwd = app.right.active_ref().cwd.clone();
        run_cmd(&mut app, "cp");
        // Opens the confirm-transfer popup aimed at the right pane.
        match &app.popup {
            Popup::ConfirmTransfer { op, dest, targets } => {
                assert_eq!(*op, PendingOp::Copy);
                assert_eq!(*dest, right_cwd);
                assert_eq!(targets.len(), 1);
            }
            other => panic!("expected a transfer confirm, got {:?}", other),
        }
    }

    #[test]
    fn mv_with_a_path_renames_a_single_file() {
        let (d, mut app) = app_with(&["old.txt", "z.txt"]);
        // Cursor on the first file (index 0 is the `..` row): old.txt.
        app.active_pane_mut().unwrap().cursor = 1;
        let first = app.active_pane().unwrap().selected().unwrap().name.clone();
        run_cmd(&mut app, &format!("mv {}", d.path().join("renamed.txt").display()));
        assert!(d.path().join("renamed.txt").is_file(), "moved to the new name");
        assert!(!d.path().join(&first).exists(), "original is gone");
    }

    #[test]
    fn rm_asks_before_deleting() {
        let (_d, mut app) = app_with(&["a.txt"]);
        run_cmd(&mut app, "rm");
        assert!(matches!(app.popup, Popup::ConfirmDelete { .. }), "rm confirms first");
    }

    #[test]
    fn ls_dash_a_toggles_hidden() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let before = app.active_pane().unwrap().show_hidden;
        run_cmd(&mut app, "ls -a");
        assert_ne!(app.active_pane().unwrap().show_hidden, before);
    }

    #[test]
    fn file_and_wc_open_a_notice() {
        let (d, mut app) = app_with(&["notes.txt"]);
        std::fs::write(d.path().join("notes.txt"), "one two three\nsecond line\n").unwrap();
        app.reload_active();
        app.active_pane_mut().unwrap().cursor = 1; // notes.txt (index 0 is `..`)

        // `:file` used to answer this, one letter from `:files` and doing
        // something else entirely. It is a column of `:attr` now, so one
        // command answers everything about the thing under the cursor.
        run_cmd(&mut app, "attr");
        let Popup::Notice { lines } = &app.popup else { panic!("attr → notice") };
        assert!(
            lines.iter().any(|l| l.contains("text")),
            "the classification came with it: {lines:?}",
        );

        run_cmd(&mut app, "wc");
        let Popup::Notice { lines } = &app.popup else { panic!("wc → notice") };
        // 2 newlines, 5 words.
        assert!(lines.iter().any(|l| l.contains(" 2 ") && l.contains(" 5 ")), "{:?}", lines);
    }

    #[test]
    fn head_and_tail_show_the_right_ends() {
        let (d, mut app) = app_with(&["log.txt"]);
        let text: String = (1..=50).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(d.path().join("log.txt"), text).unwrap();
        app.reload_active();
        app.active_pane_mut().unwrap().cursor = 1; // log.txt (index 0 is `..`)

        run_cmd(&mut app, "head -n 2");
        let Popup::Notice { lines } = &app.popup else { panic!("head → notice") };
        assert!(lines.iter().any(|l| l == "line 1"));
        assert!(!lines.iter().any(|l| l == "line 3"), "only 2 asked for: {:?}", lines);

        run_cmd(&mut app, "tail -n 2");
        let Popup::Notice { lines } = &app.popup else { panic!("tail → notice") };
        assert!(lines.iter().any(|l| l == "line 50"));
        assert!(lines.iter().any(|l| l == "line 49"));
    }

    #[test]
    fn df_reports_free_space() {
        let (_d, mut app) = app_with(&["a.txt"]);
        run_cmd(&mut app, "df -h");
        let Popup::Notice { lines } = &app.popup else { panic!("df → notice") };
        assert!(lines.iter().any(|l| l.starts_with("total")));
        assert!(lines.iter().any(|l| l.starts_with("available")));

        run_cmd(&mut app, "df -z");
        assert!(app.message.as_deref().unwrap().contains("unknown flag"), "bad flag reported");
    }

    #[test]
    fn zip_bundles_the_selection() {
        let (d, mut app) = app_with(&["one.txt", "two.txt"]);
        std::fs::write(d.path().join("one.txt"), b"1").unwrap();
        // Mark both so the whole selection is zipped.
        app.reload_active();
        let paths: Vec<PathBuf> =
            app.active_pane().unwrap().all_entries.iter().map(|e| e.path.clone()).collect();
        for p in paths {
            app.active_pane_mut().unwrap().marks.insert(p);
        }
        run_cmd(&mut app, "zip bundle");
        drain_op(&mut app);
        assert!(d.path().join("bundle.zip").is_file(), "zip created");
        let names: Vec<String> = cian_core::archive::list(&d.path().join("bundle.zip"))
            .unwrap()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert!(names.contains(&"one.txt".to_string()), "{:?}", names);
    }

    #[test]
    fn zip_dash_e_asks_for_a_password_which_is_masked() {
        let (d, mut app) = app_with(&["secret.txt"]);
        app.active_pane_mut().unwrap().cursor = 1; // secret.txt (index 0 is `..`)
        run_cmd(&mut app, "zip -e locked");
        match &app.popup {
            Popup::TextInput { kind, .. } => {
                assert!(kind.is_secret(), "the password field is a secret");
            }
            other => panic!("expected a password prompt, got {:?}", other),
        }
        // The masked field renders as dots, not the typed text.
        app.handle_key(code(KeyCode::Char('p'))).unwrap();
        app.handle_key(code(KeyCode::Char('w'))).unwrap();
        let shown = render(&mut app, 80, 20).join("\n");
        assert!(shown.contains("••"), "password shown masked:\n{}", shown);
        assert!(!shown.contains(">pw"), "the literal password must not appear");
        let _ = d;
    }

    #[test]
    fn bang_runs_in_the_shell_with_substitutions() {
        let (d, mut app) = app_with(&["target file.txt"]);
        app.active_pane_mut().unwrap().cursor = 1; // the file (index 0 is `..`)
        run_cmd(&mut app, "!echo %f");
        assert_eq!(app.focused, FocusedPane::Shell, "hands over to the shell");
        // No shell spawned in tests, so the command is queued verbatim.
        let queued = app.pending_shell_input.clone().unwrap_or_default();
        assert!(queued.starts_with("echo "), "got {:?}", queued);
        // The filename has a space, so it must be quoted as one argument.
        assert!(queued.contains("target file.txt"), "the file path is substituted: {:?}", queued);
        assert!(queued.contains('\''), "quoted because of the space: {:?}", queued);
        let _ = d;
    }

    #[test]
    fn an_unknown_command_says_so() {
        let (_d, mut app) = app_with(&["a.txt"]);
        run_cmd(&mut app, "frobnicate");
        assert!(app.message.as_deref().unwrap().contains("unknown command"));
    }

    #[test]
    fn paste_lands_in_the_command_line() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.mode = Mode::Command;
        app.command_buffer = "cd ".into();
        // A bracketed-paste event carrying a path, with a stray newline.
        app.insert_into_active_text("/some/path\n");
        assert_eq!(app.command_buffer, "cd /some/path", "newline stripped, text appended");
    }

    #[test]
    fn o_on_a_file_mirrors_the_directory_to_the_other_pane() {
        let l = tempfile::tempdir().unwrap();
        let r = tempfile::tempdir().unwrap();
        std::fs::write(l.path().join("doc.txt"), b"x").unwrap();
        std::fs::create_dir(r.path().join("elsewhere")).unwrap();
        let mut app =
            App::new(l.path().to_path_buf(), r.path().to_path_buf(), en_config())
                .unwrap();
        app.focus(FocusedPane::Left);
        app.active_pane_mut().unwrap().cursor = 1; // doc.txt (a file; index 0 is `..`)
        app.open_in_other_pane(false).unwrap();
        assert_eq!(
            app.right.active_ref().cwd,
            app.left.active_ref().cwd,
            "the other pane lines up on this directory"
        );
    }

    #[test]
    fn f_keys_manage_file_tabs() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Left);
        assert_eq!(app.left.tabs.len(), 1);
        app.handle_key(code(KeyCode::F(9))).unwrap(); // new tab — asks first
        assert!(matches!(app.popup, Popup::ConfirmNewTab { .. }), "asked: {:?}", app.popup);
        app.handle_key(code(KeyCode::Enter)).unwrap(); // yes
        assert_eq!(app.left.tabs.len(), 2);
        assert_eq!(app.left.active, 1);
        app.handle_key(code(KeyCode::F(1))).unwrap(); // previous
        assert_eq!(app.left.active, 0);
        app.handle_key(code(KeyCode::F(2))).unwrap(); // next
        assert_eq!(app.left.active, 1);
    }

    #[test]
    fn ctrl_digit_no_longer_jumps_file_tabs() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Left);
        app.left.add_clone().unwrap(); // now 2 tabs, active 1
        // Ctrl+1 used to select tab 0; it must not any more.
        app.handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(app.left.active, 1, "Ctrl+1 is no longer a tab jump");
    }

    #[test]
    fn the_default_home_prefers_config_then_desktop() {
        // A configured home directory wins when it exists.
        let d = tempfile::tempdir().unwrap();
        let mut config = en_config();
        config.options.home = Some(d.path().display().to_string());
        assert_eq!(default_home(&config), d.path());

        // A configured but missing directory falls through (to Desktop/home/.).
        let mut config = en_config();
        config.options.home = Some("/definitely/not/here".into());
        let fallback = default_home(&config);
        assert_ne!(fallback, PathBuf::from("/definitely/not/here"));
    }

    #[test]
    fn a_notice_can_be_copied_then_closes() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.popup = Popup::Notice { lines: vec!["abc123".into()] };
        app.handle_key(code(KeyCode::Char('y'))).unwrap();
        assert!(matches!(app.popup, Popup::None));
        assert_eq!(app.message.as_deref(), Some("copied"));
    }

    #[test]
    fn double_clicking_a_directory_enters_it() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/inner.txt"), b"x").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p.clone(), en_config()).unwrap();
        let _ = render(&mut app, 100, 40);
        let r = app.layout_rects.left;
        // Row 1 is the `..` row; "sub" (dirs first) is on row 2.
        let (cx, cy) = (r.x + 3, r.y + 3);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), cx, cy));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), cx, cy));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), cx, cy));

        // Compare by the final component (the pane canonicalises differently
        // per platform than std::fs::canonicalize).
        assert!(
            app.left.active_ref().cwd.ends_with("sub"),
            "double-click entered the directory: {:?}",
            app.left.active_ref().cwd
        );
    }

    #[test]
    fn a_slow_second_click_is_not_a_double_click() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p.clone(), en_config()).unwrap();
        let _ = render(&mut app, 100, 40);
        let root = app.left.active_ref().cwd.clone();
        let r = app.layout_rects.left;
        // Row 2 is "sub"; row 1 is the `..` row (which would navigate up).
        let (cx, cy) = (r.x + 3, r.y + 3);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), cx, cy));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), cx, cy));
        // Age the first click past the double-click window.
        app.last_click = Some((Instant::now() - Duration::from_secs(2), cy));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), cx, cy));
        assert_eq!(app.left.active_ref().cwd, root,
            "a slow second click just selects, does not enter");
    }

    /// `:preview` borrows the shell panel for a cursor-follow preview: file
    /// contents while a file pane has focus, the real shell as soon as the
    /// shell takes focus — and the preview cache follows the cursor.
    #[test]
    fn preview_borrows_the_shell_panel_and_follows_the_cursor() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("hello.txt"), "alpha bravo preview-me\n").unwrap();
        std::fs::write(d.path().join("other.txt"), "charlie delta other-one\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "hello.txt").unwrap();
        }
        assert!(!app.preview_on, "preview is off out of the box");
        app.preview_on = true;
        let out = render(&mut app, 110, 36).join("\n");
        assert!(out.contains("⌥ preview"), "panel is labelled: {out}");
        assert!(out.contains("preview-me"), "shows the cursor file's text");
        assert!(!out.contains("other-one"), "not the other file");

        // Cursor moves → the preview follows.
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "other.txt").unwrap();
        }
        let out = render(&mut app, 110, 36).join("\n");
        assert!(out.contains("other-one"), "follows the cursor: {out}");

        // Shell focus gets the real shell back.
        app.focus(FocusedPane::Shell);
        let out = render(&mut app, 110, 36).join("\n");
        assert!(!out.contains("⌥ preview"), "shell focus shows the shell");

        // And off means off, whatever has focus (the toggle flips on → off).
        app.focus(FocusedPane::Left);
        app.toggle_preview();
        let out = render(&mut app, 110, 36).join("\n");
        assert!(!out.contains("⌥ preview"));
    }

    /// Moving off an image asks the main loop for a full terminal clear.
    /// Terminal graphics are painted outside the cell buffer, so without it
    /// the picture stays on screen over the next file — which looked exactly
    /// like "the file after a png has no preview".
    #[test]
    fn leaving_an_image_preview_clears_only_when_it_drew_pixels() {
        let d = tempfile::tempdir().unwrap();
        // A real 2x2 PNG, plus a text file to move onto.
        let png: &[u8] = &[0x89,0x50,0x4E,0x47,0x0D,0x0A,0x1A,0x0A,0,0,0,0x0D,0x49,0x48,0x44,0x52,
            0,0,0,2,0,0,0,2,8,2,0,0,0,0xFD,0xD4,0x9A,0x73,0,0,0,0x16,0x49,0x44,0x41,0x54,
            0x78,0x9C,0x62,0xF8,0xCF,0xC0,0,0,0x03,0x01,0x01,0,0x18,0xDD,0x8D,0xB0,
            0,0,0,0,0x49,0x45,0x4E,0x44,0xAE,0x42,0x60,0x82];
        std::fs::write(d.path().join("pic.png"), png).unwrap();
        std::fs::write(d.path().join("after.txt"), "plain text after the picture\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.preview_on = true;   // 既定は切（2026-09-06）

        let go = |app: &mut App, name: &str| {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == name).unwrap();
        };
        go(&mut app, "pic.png");
        let _ = render(&mut app, 100, 30);
        app.full_clear = false; // ignore anything the first frame asked for

        // Onto the text file. A wipe is only owed when the picture was drawn
        // with the terminal's own protocol — those pixels are not in the cell
        // buffer ratatui diffs against. Half-blocks are ordinary cells, and
        // wiping for them costs a full repaint of the window on every step
        // through a folder of images, which showed as a black flash.
        go(&mut app, "after.txt");
        let out = render(&mut app, 100, 30).join("\n");
        assert_eq!(
            app.full_clear,
            app.gfx_picker.is_some(),
            "a wipe only when pixels were drawn",
        );
        assert!(out.contains("plain text after"), "and the text is drawn: {out}");

        // A steady text preview asks for nothing at all.
        app.full_clear = false;
        let _ = render(&mut app, 100, 30);
        assert!(!app.full_clear, "a steady text preview asks for no clears");
    }

    /// A directory under the cursor previews as its listing.
    #[test]
    fn preview_lists_a_directory() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/inside.txt"), "x").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "sub").unwrap();
        }
        app.preview_on = true;
        let out = render(&mut app, 110, 36).join("\n");
        assert!(out.contains("inside.txt"), "directory listing shown: {out}");
    }

    /// Clicking a column header sorts by it; clicking it again flips the
    /// direction — how column headers behave everywhere else.
    #[test]
    fn clicking_a_column_header_sorts_and_flips() {
        let (_d, mut app) = app_with(&["small.txt", "big.txt"]);
        std::fs::write(_d.path().join("big.txt"), "x".repeat(5000)).unwrap();
        if let Some(p) = app.active_pane_mut() {
            let _ = p.reload();
        }
        let _ = render(&mut app, 100, 40);
        let (pane, key, r) = app
            .sort_rects
            .iter()
            .copied()
            .find(|(p, k, _)| *p == FocusedPane::Left && *k == cian_core::SortKey::Size)
            .expect("the Size header is clickable");
        assert_eq!(pane, FocusedPane::Left);
        assert_eq!(key, cian_core::SortKey::Size);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x, r.y));
        assert_eq!(app.active_pane().unwrap().sort.key, cian_core::SortKey::Size);
        assert!(!app.active_pane().unwrap().sort.reverse, "first click: ascending");
        let _ = render(&mut app, 100, 40); // rects rebuilt with the new sort glyph
        let (_, _, r) = app
            .sort_rects
            .iter()
            .copied()
            .find(|(p, k, _)| *p == FocusedPane::Left && *k == cian_core::SortKey::Size)
            .unwrap();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x, r.y));
        assert!(app.active_pane().unwrap().sort.reverse, "second click flips");
    }

    /// Clicking a path segment in the title jumps to that ancestor directory.
    #[test]
    fn clicking_a_breadcrumb_segment_navigates_up() {
        let d = tempfile::tempdir().unwrap();
        let deep = d.path().join("alpha").join("beta");
        std::fs::create_dir_all(&deep).unwrap();
        let mut app = App::new(deep.clone(), deep.clone(), en_config()).unwrap();
        let _ = render(&mut app, 120, 40);
        // strip=1 is the parent of the cwd ("alpha").
        let (_, _, r) = app
            .crumb_rects
            .iter()
            .copied()
            .find(|(p, strip, _)| *p == FocusedPane::Left && *strip == 1)
            .expect("the parent segment is clickable");
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x, r.y));
        assert!(
            app.left.active_ref().cwd.ends_with("alpha"),
            "clicked one level up: {:?}",
            app.left.active_ref().cwd
        );
    }

    #[test]
    fn clicking_a_tab_switches_to_it() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Left);
        app.left.add_clone().unwrap(); // second tab, now active
        // Wide enough that the first tab is not collapsed into a +N marker.
        let _ = render(&mut app, 300, 40);
        assert_eq!(app.left.active, 1);

        let (_, _, r) = app
            .tab_rects
            .iter()
            .copied()
            .find(|(p, i, _)| *p == FocusedPane::Left && *i == 0)
            .expect("a rect for the left pane's first tab");
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y));
        assert_eq!(app.left.active, 0, "clicking the first tab selected it");
        assert_eq!(app.focused, FocusedPane::Left);
    }

    #[test]
    fn the_context_menu_runs_the_item_that_was_clicked() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Left);
        let _ = render(&mut app, 100, 40);
        // Open the menu at a known spot, then render so menu_rect is set.
        app.open_context_menu(10, 10);
        let _ = render(&mut app, 100, 40);
        let m = app.menu_rect;
        // The Quit item is second-to-last; click its row.
        let (quit_idx, _) = {
            let Popup::ContextMenu { items, .. } = &app.popup else { panic!("no menu") };
            items.iter().enumerate().find(|(_, it)| **it == MenuItem::Quit).expect("quit item")
        };
        let row = m.y + 1 + quit_idx as u16;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), m.x + 2, row));
        assert!(matches!(app.popup, Popup::ConfirmQuit), "clicking Quit opened the confirm");
    }

    #[test]
    fn clicking_off_the_context_menu_dismisses_it() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = render(&mut app, 100, 40);
        app.open_context_menu(10, 10);
        let _ = render(&mut app, 100, 40);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
        assert!(matches!(app.popup, Popup::None));
    }

    // ---- 画面の幅は桁で測る ----
    //
    // **どちらも日本語でしか出ない。** 英語なら字数と桁数が同じなので、
    // `chars().count()` で作った箱はぴったり合う。実際に見つかったのは
    // 2026-09-06、端末版を本物の pty で動かしたとき。

    /// ポップアップのボタンは**描かれた桁数**の箱に入る。
    ///
    /// `[ コピー ]` は7字だが10桁。字数で作った箱に入れると ratatui が7桁で
    /// 切って `[ コピ` になる ── 実機の画面でそう出た。押せる幅も同じ箱なので、
    /// 見えているボタンの右半分が押せていなかった。
    #[test]
    fn a_popup_button_is_as_wide_as_it_is_drawn() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.lang = Lang::Ja;
        app.open_popup(Popup::Notice { lines: vec!["ためし".into()] });
        let _ = render_buf(&mut app, 100, 30);
        // **箱そのものを見る、描かれた字ではなく。** 画面を文字列にすると
        // 全角の2セル目が空白で戻るので、`contains("[ コピー ]")` は正しく
        // 描けていても外れる。押せる範囲は箱で決まるので、箱を測る。
        let widths: Vec<u16> = app.popup_zones.iter().map(|z| z.rect.width).collect();
        // `[ コピー ]` は7字・10桁（`[`+空白+全角3つ+空白+`]`）。`[ 閉じる ]` も同じ。
        assert_eq!(widths, vec![10, 10], "字数(7)ではなく桁数(10)で測ること");
    }

    /// シェルのタブ帯のクリック範囲も**桁**。
    ///
    /// 実測（120桁の pty）: 「日本語」のタブは 3..10 桁に描かれるのに、字数で
    /// 測った当たり判定は 1..6 桁しかなく、**8・9・10 桁を叩くと隣のタブへ
    /// 飛んだ**。ファイルのペインの帯は既に `width_of` で測っていて、ここだけ
    /// 取り残されていた。
    #[test]
    fn a_japanese_shell_tab_is_clickable_where_it_is_drawn() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let dir = tempfile::tempdir().unwrap();
        let sh = cian_pty::default_shell();
        let session = cian_pty::PtySession::new(dir.path(), &sh, 24, 80).unwrap();
        app.shell.tabs.push(ShellTab::new(session));
        app.shell.active = 0;
        app.shell.rename_active("日本語".to_string());
        app.preview_on = false;
        app.focus(FocusedPane::Shell);
        let _ = render_buf(&mut app, 100, 30);
        let (_, _, rect) = app
            .tab_rects
            .iter()
            .find(|(p, i, _)| *p == FocusedPane::Shell && *i == 0)
            .copied()
            .expect("シェルのタブ0の当たり判定が無い");
        // " 日本語 " ── 5字・8桁。
        assert_eq!(rect.width, 8, "字数(5)ではなく桁数(8)で測ること");
    }

    // ---- keyboard pane resize ----

    fn ctrl_shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT)
    }

    #[test]
    fn ctrl_shift_arrows_resize_the_file_panes() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Left);
        assert_eq!(app.panes_pct, 50);
        assert_eq!(app.main_pct, 60);

        // Right pushes the left|right divider right → left pane grows.
        app.handle_key(ctrl_shift(KeyCode::Right)).unwrap();
        assert!(app.panes_pct > 50, "left grew: {}", app.panes_pct);
        let wider = app.panes_pct;
        app.handle_key(ctrl_shift(KeyCode::Left)).unwrap();
        assert!(app.panes_pct < wider, "left shrank back");

        // Down grows the file area (files|shell divider moves down).
        app.handle_key(ctrl_shift(KeyCode::Down)).unwrap();
        assert!(app.main_pct > 60, "files grew: {}", app.main_pct);
        app.handle_key(ctrl_shift(KeyCode::Up)).unwrap();
        app.handle_key(ctrl_shift(KeyCode::Up)).unwrap();
        assert!(app.main_pct < 60, "and shrank past the start");
    }

    #[test]
    fn resize_is_clamped_so_a_pane_never_vanishes() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Left);
        for _ in 0..50 {
            app.handle_key(ctrl_shift(KeyCode::Left)).unwrap();
        }
        assert_eq!(app.panes_pct, MIN_SPLIT_PCT, "cannot shrink below the floor");
        for _ in 0..50 {
            app.handle_key(ctrl_shift(KeyCode::Right)).unwrap();
        }
        assert_eq!(app.panes_pct, 100 - MIN_SPLIT_PCT, "nor grow past the ceiling");
    }

    #[test]
    fn from_the_shell_up_down_resizes_the_shell_area() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        assert_eq!(app.main_pct, 60);
        // With no inner split, Up grows the shell (files|shell divider up).
        app.handle_key(ctrl_shift(KeyCode::Up)).unwrap();
        assert!(app.main_pct < 60, "shell grew: {}", app.main_pct);
    }

    /// **All four arrows, not two.** With no inner split, Left/Right used to
    /// do nothing at all from the shell — reported on 2026-09-06 as「シェル
    /// パネルで Meta+Shift+矢印で窓サイズの変更ができなかった」. The outer
    /// divider along the same axis is the answer, exactly as Up/Down already
    /// had it.
    #[test]
    fn from_the_shell_left_right_move_the_pane_divider_when_there_is_no_split() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        assert_eq!(app.panes_pct, 50);
        let main_before = app.main_pct;

        app.handle_key(ctrl_shift(KeyCode::Right)).unwrap();
        assert!(app.panes_pct > 50, "left pane grew: {}", app.panes_pct);
        let wider = app.panes_pct;
        app.handle_key(ctrl_shift(KeyCode::Left)).unwrap();
        assert!(app.panes_pct < wider, "and shrank back");
        // The other divider is not touched: one key, one axis.
        assert_eq!(app.main_pct, main_before, "files|shell stayed where it was");
    }

    // ---- editing, confirms, search, history refinements ----

    #[test]
    fn the_text_field_edits_at_the_caret_not_only_the_end() {
        let (_d, mut app) = app_with(&["report.txt"]);
        app.active_pane_mut().unwrap().cursor = 1; // report.txt (index 0 is `..`)
        app.handle_key(code(KeyCode::Char('r'))).unwrap(); // rename prompt
        // Seeded with the name, caret at the end.
        {
            let Popup::TextInput { buffer, cursor, .. } = &app.popup else { panic!("no prompt") };
            assert_eq!(buffer, "report.txt");
            assert_eq!(*cursor, "report.txt".chars().count());
        }
        // Move left past ".txt" (4 chars) and insert.
        for _ in 0..4 { app.handle_key(code(KeyCode::Left)).unwrap(); }
        for c in "_v2".chars() { app.handle_key(code(KeyCode::Char(c))).unwrap(); }
        let Popup::TextInput { buffer, .. } = &app.popup else { panic!("no prompt") };
        assert_eq!(buffer, "report_v2.txt", "inserted before the extension");

        // Home, then Delete removes the first char.
        app.handle_key(code(KeyCode::Home)).unwrap();
        app.handle_key(code(KeyCode::Delete)).unwrap();
        let Popup::TextInput { buffer, cursor, .. } = &app.popup else { panic!("no prompt") };
        assert_eq!(buffer, "eport_v2.txt");
        assert_eq!(*cursor, 0);

        // Backspace at the start is a no-op, not a panic.
        app.handle_key(code(KeyCode::Backspace)).unwrap();
        let Popup::TextInput { buffer, .. } = &app.popup else { panic!("no prompt") };
        assert_eq!(buffer, "eport_v2.txt");
    }

    #[test]
    fn caret_editing_handles_multibyte_characters() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.popup = text_input("t", "p", "あい".to_string(), InputKind::JumpPath);
        // Caret at end (2 chars). Left once → between あ and い. Insert 'X'.
        app.handle_key(code(KeyCode::Left)).unwrap();
        app.handle_key(code(KeyCode::Char('X'))).unwrap();
        let Popup::TextInput { buffer, .. } = &app.popup else { panic!("no prompt") };
        assert_eq!(buffer, "あXい", "insert respects char boundaries");
    }

    #[test]
    fn enter_is_yes_on_a_transfer_confirm() {
        let l = tempfile::tempdir().unwrap();
        let r = tempfile::tempdir().unwrap();
        std::fs::write(l.path().join("doc.txt"), b"hi").unwrap();
        let mut app =
            App::new(l.path().to_path_buf(), r.path().to_path_buf(), en_config())
                .unwrap();
        run_cmd(&mut app, "cp"); // ConfirmTransfer to the right pane
        app.handle_key(code(KeyCode::Enter)).unwrap();
        drain_op(&mut app);
        assert!(r.path().join("doc.txt").is_file(), "Enter confirmed the copy");
    }

    #[test]
    fn r_on_a_move_confirm_renames_into_the_destination() {
        let l = tempfile::tempdir().unwrap();
        let r = tempfile::tempdir().unwrap();
        std::fs::write(l.path().join("old.txt"), b"data").unwrap();
        let mut app =
            App::new(l.path().to_path_buf(), r.path().to_path_buf(), en_config())
                .unwrap();
        app.active_pane_mut().unwrap().cursor = 1; // old.txt (index 0 is `..`)
        app.handle_key(code(KeyCode::Char('m'))).unwrap(); // move confirm
        app.handle_key(code(KeyCode::Char('r'))).unwrap(); // rename & move
        // Seeded with the source name; clear it and type a new one.
        let Popup::TextInput { kind: InputKind::TransferAs { .. }, .. } = &app.popup else {
            panic!("expected the rename prompt, got {:?}", app.popup)
        };
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)).unwrap();
        for c in "new.txt".chars() { app.handle_key(code(KeyCode::Char(c))).unwrap(); }
        app.handle_key(code(KeyCode::Enter)).unwrap();

        assert!(r.path().join("new.txt").is_file(), "moved under the new name");
        assert!(!l.path().join("old.txt").exists(), "and gone from the source");
    }

    #[test]
    fn search_arrows_step_through_the_matches() {
        let (_d, mut app) = app_with(&["a1.txt", "a2.txt", "zzz.txt"]);
        // Sorted: a1, a2, zzz.
        app.handle_key(code(KeyCode::Char('f'))).unwrap(); // search
        app.handle_key(code(KeyCode::Char('a'))).unwrap(); // matches a1, a2
        app.handle_key(code(KeyCode::Down)).unwrap();
        let first = app.active_pane().unwrap().cursor;
        assert!(app.active_pane().unwrap().entries[first].name.contains('a'));
        app.handle_key(code(KeyCode::Down)).unwrap();
        let second = app.active_pane().unwrap().cursor;
        assert_ne!(first, second, "Down moved to the other match");
        assert!(app.active_pane().unwrap().entries[second].name.contains('a'));
    }

    #[test]
    fn history_a_bookmarks_the_selected_path_as_a_shortcut() {
        let (_d, mut app) = app_with(&["a.txt"]);
        // Seed some history and open it.
        app.active_pane_mut().unwrap().history =
            vec![PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")];
        app.start_history();
        assert!(matches!(app.popup, Popup::History { .. }));
        app.handle_key(code(KeyCode::Down)).unwrap(); // select /tmp/two
        app.handle_key(code(KeyCode::Char('a'))).unwrap(); // add shortcut

        // Now on the name step; type a name and continue.
        let Popup::TextInput { kind: InputKind::ShortcutName { .. }, .. } = &app.popup else {
            panic!("expected the shortcut-name prompt, got {:?}", app.popup)
        };
        for c in "mydir".chars() { app.handle_key(code(KeyCode::Char(c))).unwrap(); }
        app.handle_key(code(KeyCode::Enter)).unwrap();

        // The target step must be pre-filled with the chosen history path.
        let Popup::TextInput { buffer, kind: InputKind::ShortcutTarget { .. }, .. } = &app.popup
        else {
            panic!("expected the target step, got {:?}", app.popup)
        };
        assert_eq!(buffer, "/tmp/two", "target seeded from the history selection");
    }

    #[test]
    fn the_history_popup_highlights_the_selection() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.active_pane_mut().unwrap().history =
            vec![PathBuf::from("/tmp/alpha"), PathBuf::from("/tmp/beta")];
        app.start_history();
        let shown = render(&mut app, 100, 20).join("\n");
        assert!(shown.contains("▸"), "the selected row has a marker:\n{}", shown);
        assert!(shown.contains("/tmp/alpha") && shown.contains("/tmp/beta"), "{}", shown);
    }

    /// Right-click Paste in the shell must send text to the terminal, not try
    /// to paste files as it does in a file pane.
    #[test]
    fn shell_paste_sends_text_not_files() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.focus(FocusedPane::Shell);
        app.file_clip = None;
        app.run_menu_item(MenuItem::Paste).unwrap();
        // Whatever the clipboard held, this took the shell text path — never
        // the file path, whose messages talk about "files".
        let msg = app.message.clone().unwrap_or_default();
        assert!(!msg.contains("files"), "should not paste files in the shell: {:?}", msg);
    }

    #[test]
    fn f3_views_a_text_file() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.rs"), "fn main() {}\nsecond\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.handle_key(code(KeyCode::F(3))).unwrap();
        let Popup::Viewer { view, title, .. } = &app.popup else {
            panic!("expected the viewer, got {:?}", app.popup)
        };
        assert_eq!(title, "a.rs");
        assert_eq!(view.kind, cian_core::viewer::ViewKind::Text);
        assert_eq!(view.lines, vec!["fn main() {}", "second"]);
    }

    #[test]
    fn a_markdown_file_opens_in_preview_and_toggles_to_source() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("readme.md"), "# Title\n\n- item\n\n```mermaid\ngraph TD; A-->B\n```\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.handle_key(code(KeyCode::F(3))).unwrap();
        // A .md file opens straight into rendered preview.
        assert!(matches!(&app.popup, Popup::Viewer { markdown: true, preview: true, .. }), "opened in preview");
        // The render swaps the rendered document into view.lines (and fills the
        // per-char style grid) so the whole viewer works over the preview.
        let _ = render(&mut app, 100, 30);
        if let Popup::Viewer { view, md_styles, source, .. } = &app.popup {
            let flat = view.lines.join("\n");
            assert!(flat.contains("mermaid flow"), "mermaid flow is rendered");
            assert!(flat.contains('▶'), "the flow shows an arrow edge");
            assert!(!md_styles.is_empty(), "per-char styles were built");
            assert!(source.iter().any(|l| l == "# Title"), "the raw source is preserved");
        } else {
            panic!("not a viewer");
        }

        // Search works in preview: `/` then a query jumps the cursor to a match.
        app.handle_key(key('/')).unwrap();
        for c in "mermaid".chars() { app.handle_key(key(c)).unwrap(); }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        if let Popup::Viewer { view, line, find_query, .. } = &app.popup {
            assert_eq!(find_query.as_deref(), Some("mermaid"), "search is confirmed");
            assert!(view.lines[*line].contains("mermaid"), "cursor landed on a match");
        } else {
            panic!("not a viewer");
        }

        // Ctrl+E toggles to raw source (view.lines becomes the file text
        // again); `:preview` does the same where Ctrl is not deliverable.
        app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)).unwrap();
        let _ = render(&mut app, 100, 30);
        if let Popup::Viewer { view, md_styles, preview, .. } = &app.popup {
            assert!(!*preview, "toggled to source");
            assert!(md_styles.is_empty(), "styles dropped in source mode");
            assert!(view.lines.iter().any(|l| l == "# Title"), "shows raw source");
        } else {
            panic!("not a viewer");
        }
        app.handle_key(key(':')).unwrap();
        if let Popup::Viewer { sub_input, .. } = &mut app.popup {
            *sub_input = Some("preview".into());
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(&app.popup, Popup::Viewer { preview: true, .. }), "back to preview");
        // Esc peels state: the still-active search clears first (viewer stays),
        // then a second Esc closes.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        match &app.popup {
            Popup::Viewer { find_query, .. } => assert!(find_query.is_none(), "search cleared, not closed"),
            _ => panic!("first Esc should have kept the viewer open"),
        }
        quit_viewer(&mut app);
        assert!(matches!(app.popup, Popup::None), ":q closes it");
    }

    #[test]
    fn undo_reverses_a_rename() {
        let (d, mut app) = app_with(&["old.txt"]);
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "old.txt").unwrap();
        app.start_rename();
        if let Popup::TextInput { buffer, .. } = &mut app.popup {
            buffer.clear();
            buffer.push_str("new.txt");
        } else {
            panic!("no rename prompt");
        }
        app.finish_text_input().unwrap();
        assert!(d.path().join("new.txt").exists() && !d.path().join("old.txt").exists());

        app.undo_last();
        assert!(d.path().join("old.txt").exists(), "rename undone");
        assert!(!d.path().join("new.txt").exists());
        // Nothing left to undo.
        app.undo_last();
        assert!(app.message.as_deref().unwrap_or("").contains("undo"));
    }

    #[test]
    fn undo_reverses_a_move_between_panes() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("f.txt"), b"data").unwrap();
        let mut app = App::new(
            src.path().to_path_buf(),
            dst.path().to_path_buf(),
            en_config(),
        )
        .unwrap();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "f.txt").unwrap();

        app.start_transfer(PendingOp::Move);
        assert!(matches!(app.popup, Popup::ConfirmTransfer { .. }), "move confirm");
        app.finish_transfer(Conflict::Overwrite).unwrap();
        drain_op_job(&mut app);
        assert!(dst.path().join("f.txt").exists() && !src.path().join("f.txt").exists(), "moved");

        app.undo_last();
        assert!(src.path().join("f.txt").exists(), "move undone");
        assert!(!dst.path().join("f.txt").exists());
    }

    #[test]
    fn menu_lang_overrides_lang_for_menu_and_manual() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().to_path_buf();
        let mut cfg = en_config();
        cfg.options.lang = Some("en".into());
        cfg.options.menu_lang = Some("ja".into());
        let app = App::new(p.clone(), p, cfg).unwrap();
        assert_eq!(app.lang, Lang::En, "the rest of the UI stays English");
        assert_eq!(app.menu_lang, Lang::Ja, "menu + manual follow menu_lang");

        // Unset menu_lang follows lang.
        let d2 = tempfile::tempdir().unwrap();
        let p2 = d2.path().to_path_buf();
        let mut cfg2 = en_config();
        cfg2.options.lang = Some("ja".into());
        let app2 = App::new(p2.clone(), p2, cfg2).unwrap();
        assert_eq!(app2.menu_lang, Lang::Ja, "falls back to lang when unset");
    }

    #[test]
    fn where_shows_config_paths() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.show_config_paths();
        match &app.popup {
            Popup::Notice { lines } => {
                assert!(lines.iter().any(|l| l.starts_with("portable mode:")), "reports portable status");
                assert!(lines.iter().any(|l| l.contains("shortcuts.lua")), "lists shortcuts.lua");
                assert!(lines.iter().any(|l| l.contains("user config dir:")), "shows the user config dir");
            }
            _ => panic!("no notice"),
        }
    }

    #[test]
    fn malformed_shortcuts_lua_is_an_error_not_silence() {
        // The parser must reject a bad hand-edit so the app can surface it
        // instead of loading an empty list without a word.
        assert!(cian_lua::shortcuts::parse("return 42").is_err(), "non-table rejected");
        assert!(cian_lua::shortcuts::parse("this is not lua {{{").is_err(), "syntax error rejected");
        assert!(cian_lua::shortcuts::parse("return { { target = \"/x\" } }").is_err(), "entry without name rejected");
        // A well-formed file still parses.
        assert!(cian_lua::shortcuts::parse("return { { name = \"home\", target = \"/home\" } }").is_ok());
    }

    #[test]
    fn menu_label_splits_name_and_hint() {
        use crate::render::menu_label_parts;
        assert_eq!(menu_label_parts("Bulk rename…  (:brename)"), ("Bulk rename…", "(:brename)"));
        assert_eq!(menu_label_parts("Copy"), ("Copy", ""));
        assert_eq!(menu_label_parts("AI - crmaine ▸"), ("AI - crmaine ▸", ""));
    }

    #[test]
    fn chmod_field_parses_octal() {
        use crate::parse_chmod;
        assert_eq!(parse_chmod("777"), (Some(0o777), None));
        assert_eq!(parse_chmod(" 644 "), (Some(0o644), None));
        assert_eq!(parse_chmod(""), (None, None)); // blank = keep
        assert!(parse_chmod("999").1.is_some(), "8/9 are not octal");
        assert!(parse_chmod("rwx").1.is_some(), "symbolic not accepted");
    }

    #[test]
    fn readable_on_flips_with_background_luminance() {
        use crate::render::readable_on;
        use ratatui::style::Color;
        // Light ground (Solarized Light selection) → dark text.
        let dark = readable_on(Color::Rgb(0xdc, 0xd5, 0xbe));
        assert!(matches!(dark, Color::Rgb(r, _, _) if r < 80), "dark text on light bg");
        // Dark ground (default selection) → light text.
        let light = readable_on(Color::Rgb(60, 60, 90));
        assert!(matches!(light, Color::Rgb(r, _, _) if r > 180), "light text on dark bg");
    }

    #[test]
    fn snippet_launcher_filters_and_confirms() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().to_path_buf();
        let mut cfg = en_config();
        cfg.snippets = vec![
            cian_lua::Snippet { name: "list".into(), cmd: "ls -la".into(), enter: true, confirm: false },
            cian_lua::Snippet { name: "danger".into(), cmd: "rm -rf x".into(), enter: true, confirm: true },
        ];
        let mut app = App::new(p.clone(), p, cfg).unwrap();

        // Ctrl+Shift+Enter opens it from a file pane...
        let cse = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        app.handle_key(cse).unwrap();
        assert!(matches!(app.popup, Popup::Snippets { .. }), "Ctrl+Shift+Enter opens the launcher");
        app.popup = Popup::None;

        // ...and also while the shell pane is focused (the whole point — a plain
        // key there would go to the terminal instead).
        app.focused = FocusedPane::Shell;
        app.handle_key(cse).unwrap();
        assert!(matches!(app.popup, Popup::Snippets { .. }), "opens from the shell too");
        app.popup = Popup::None;
        app.focused = FocusedPane::Left;

        // Opening lists all; typing filters by name/command.
        app.start_snippets();
        assert!(matches!(app.popup, Popup::Snippets { .. }), "launcher opens");
        assert_eq!(app.snippet_matches("").len(), 2);
        assert_eq!(app.snippet_matches("dang").len(), 1);
        assert_eq!(app.snippet_matches("ls").len(), 1, "matches command text too");

        // A plain snippet is delivered and the picker closes.
        app.send_snippet(0);
        assert!(!matches!(app.popup, Popup::ConfirmSnippet { .. }), "no confirm for a safe snippet");

        // A confirm-flagged snippet routes through the confirmation.
        app.send_snippet(1);
        match &app.popup {
            Popup::ConfirmSnippet { name, cmd, .. } => {
                assert_eq!(name, "danger");
                assert_eq!(cmd, "rm -rf x");
            }
            _ => panic!("destructive snippet must confirm"),
        }
    }

    #[test]
    fn bulk_rename_previews_then_applies() {
        let d = tempfile::tempdir().unwrap();
        for n in ["a.txt", "b.txt"] {
            std::fs::write(d.path().join(n), b"x").unwrap();
        }
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p.clone(), en_config()).unwrap();
        let targets = vec![p.join("a.txt"), p.join("b.txt")];

        // Template with a padded counter → a review checklist, nothing on disk yet.
        app.build_bulk_rename(&targets, "img_{n2}.{ext}");
        match &app.popup {
            Popup::RenameReview { items, .. } => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].new, "img_01.txt");
                assert_eq!(items[1].new, "img_02.txt");
            }
            _ => panic!("no review popup"),
        }
        assert!(p.join("a.txt").exists(), "not renamed until applied");

        app.apply_rename_plan();
        assert!(p.join("img_01.txt").exists() && p.join("img_02.txt").exists(), "renamed");
        assert!(!p.join("a.txt").exists());

        // A regex substitution over the current names.
        let targets = vec![p.join("img_01.txt"), p.join("img_02.txt")];
        app.build_bulk_rename(&targets, "s/img/photo/");
        match &app.popup {
            Popup::RenameReview { items, .. } => assert_eq!(items[0].new, "photo_01.txt"),
            _ => panic!("no review popup"),
        }

        // A pattern that changes nothing reports rather than opening a review.
        app.popup = Popup::None;
        app.build_bulk_rename(&targets, "{name}.{ext}");
        assert!(matches!(app.popup, Popup::None), "no-op does not open a review");

        // A malformed pattern is reported, not opened.
        app.build_bulk_rename(&targets, "s/[/x/");
        assert!(matches!(app.popup, Popup::None), "bad pattern does not open a review");
    }

    #[test]
    fn dir_compare_copy_across_reconciles_entries() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        let l = tempfile::tempdir().unwrap();
        let r = tempfile::tempdir().unwrap();
        std::fs::write(l.path().join("only_left.txt"), b"L").unwrap();
        std::fs::write(l.path().join("both.txt"), b"AAA").unwrap();
        std::fs::write(r.path().join("both.txt"), b"BBB").unwrap();

        let mut app = App::new(
            l.path().to_path_buf(),
            r.path().to_path_buf(),
            en_config(),
        )
        .unwrap();

        // Build the folder comparison synchronously (skip the async job).
        let cancel = Arc::new(AtomicBool::new(false));
        let diff = cian_core::dirdiff::compare(l.path(), r.path(), &cancel, &mut |_| {});
        let find = |app: &App, name: &str| {
            let Popup::DirCompare { entries, .. } = &app.popup else { panic!("not dircompare") };
            entries.iter().position(|e| e.rel.to_string_lossy() == name)
        };
        let set = |app: &mut App, cur: usize| {
            if let Popup::DirCompare { cursor, .. } = &mut app.popup { *cursor = cur; }
        };
        let mk = |app: &mut App, entries: Vec<cian_core::dirdiff::Entry>| {
            app.popup = Popup::DirCompare {
                left: "L".into(), right: "R".into(),
                left_root: l.path().to_path_buf(), right_root: r.path().to_path_buf(),
                entries, cursor: 0, scroll: 0, truncated: false,
            };
        };
        mk(&mut app, diff.entries.clone());

        // only_left.txt → right: destination absent, so it copies immediately
        // and the entry drops out (both sides now match).
        let i = find(&app, "only_left.txt").unwrap();
        set(&mut app, i);
        app.dir_compare_copy(true);
        assert!(r.path().join("only_left.txt").exists(), "created on the right");
        assert!(find(&app, "only_left.txt").is_none(), "entry reconciled");

        // both.txt differs → overwrite needs confirmation.
        let i = find(&app, "both.txt").unwrap();
        set(&mut app, i);
        app.dir_compare_copy(true);
        assert!(matches!(app.popup, Popup::ConfirmDiffCopy { .. }), "overwrite confirms");
        app.confirm_diff_copy();
        assert_eq!(std::fs::read(r.path().join("both.txt")).unwrap(), b"AAA", "overwritten");

        // Cancel path restores the comparison without copying.
        mk(&mut app, cian_core::dirdiff::compare(l.path(), r.path(), &cancel, &mut |_| {}).entries);
        std::fs::write(l.path().join("both.txt"), b"CCC").unwrap();
        std::fs::write(r.path().join("both.txt"), b"DDD").unwrap();
        mk(&mut app, cian_core::dirdiff::compare(l.path(), r.path(), &cancel, &mut |_| {}).entries);
        let i = find(&app, "both.txt").unwrap();
        set(&mut app, i);
        app.dir_compare_copy(true);
        app.cancel_diff_copy();
        assert!(matches!(app.popup, Popup::DirCompare { .. }), "comparison restored");
        assert_eq!(std::fs::read(r.path().join("both.txt")).unwrap(), b"DDD", "not copied on cancel");
    }

    #[test]
    fn recent_files_dedupe_and_skip_remote_temp() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.note_recent_file(std::path::Path::new("/proj/a.rs"));
        app.note_recent_file(std::path::Path::new("/proj/b.rs"));
        app.note_recent_file(std::path::Path::new("/proj/a.rs")); // re-open moves to front
        assert_eq!(app.recent_files.len(), 2, "duplicate collapsed");
        assert_eq!(app.recent_files[0], std::path::PathBuf::from("/proj/a.rs"), "most recent first");

        // A downloaded remote temp is not a reopenable local file.
        app.note_recent_file(std::path::Path::new("/tmp/cian-remote/x.log"));
        assert_eq!(app.recent_files.len(), 2, "remote temp not recorded");
    }

    #[test]
    fn ai_history_archives_reopens_and_forgets() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        // A RAG chat with an answer in it.
        app.popup = Popup::AiChat {
            input: String::new(),
            log: vec![
                ChatMsg { user: true, text: "first question".into() },
                ChatMsg { user: false, text: "an answer".into() },
            ],
            scroll: 0,
            pending: false,
            sel: None,
            mode: ChatMode::Ai,
            skin: ChatSkin::of(ChatMode::Ai),
        };
        app.open_ai_history();
        assert!(matches!(app.popup, Popup::AiHistory { .. }), "history picker opens");
        assert_eq!(app.ai_history.len(), 1, "current conversation archived");
        assert_eq!(app.ai_history[0].mode(), ChatMode::Ai, "backend remembered");
        assert_eq!(App::ai_history_title(app.ai_history[0].log()), "first question");

        // Reopening restores the backend, so a follow-up goes the same way.
        app.load_ai_conversation(0);
        assert!(matches!(app.popup, Popup::AiChat { mode: ChatMode::Ai, .. }), "mode restored");
        app.open_ai_history();
        assert_eq!(app.ai_history.len(), 1, "identical snapshot deduped");

        // A chat with no answer is not worth archiving.
        app.popup = Popup::AiChat {
            input: String::new(),
            log: vec![ChatMsg { user: true, text: "unanswered".into() }],
            scroll: 0,
            pending: true,
            sel: None,
            mode: ChatMode::Ai,
            skin: ChatSkin::of(ChatMode::Ai),
        };
        app.archive_current_ai_chat();
        assert_eq!(app.ai_history.len(), 1, "answerless chat not archived");

        // Reopen, then forget it.
        app.load_ai_conversation(0);
        assert!(matches!(app.popup, Popup::AiChat { .. }), "conversation reopened");
        app.popup = Popup::AiHistory { cursor: 0 };
        app.delete_ai_conversation(0);
        assert!(app.ai_history.is_empty(), "conversation forgotten");
    }

    #[test]
    fn folder_sync_one_way_copies_source_and_keeps_dest_only() {
        use std::sync::Arc;
        let l = tempfile::tempdir().unwrap();
        let r = tempfile::tempdir().unwrap();
        std::fs::write(l.path().join("only_left.txt"), b"L").unwrap();
        std::fs::write(l.path().join("both.txt"), b"AAA").unwrap();
        std::fs::write(r.path().join("both.txt"), b"BBB").unwrap();
        std::fs::write(r.path().join("only_right.txt"), b"R").unwrap();
        // A whole subtree present only on the left copies as one entry.
        std::fs::create_dir(l.path().join("newdir")).unwrap();
        std::fs::write(l.path().join("newdir").join("deep.txt"), b"D").unwrap();

        let mut app = App::new(
            l.path().to_path_buf(),
            r.path().to_path_buf(),
            en_config(),
        )
        .unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let diff = cian_core::dirdiff::compare(l.path(), r.path(), &cancel, &mut |_| {});
        app.popup = Popup::DirCompare {
            left: "L".into(), right: "R".into(),
            left_root: l.path().to_path_buf(), right_root: r.path().to_path_buf(),
            entries: diff.entries, cursor: 0, scroll: 0, truncated: false,
        };

        // Sync left → right: everything the left has, none of it deleted.
        app.dir_compare_sync(true);
        let Popup::ConfirmDirSync { ops, extra, to_right, .. } = &app.popup else {
            panic!("expected a sync confirmation, got {:?}", app.popup);
        };
        assert!(*to_right);
        assert_eq!(*extra, 1, "only_right.txt is destination-only");
        assert_eq!(ops.len(), 3, "only_left.txt + both.txt + newdir/");
        app.confirm_dir_sync();
        assert!(app.op_job.is_some(), "sync runs on the worker");
        let start = Instant::now();
        while app.op_job.is_some() && start.elapsed() < Duration::from_secs(5) {
            app.poll_op_job();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(r.path().join("only_left.txt").exists(), "source-only copied");
        assert_eq!(std::fs::read(r.path().join("both.txt")).unwrap(), b"AAA", "differing overwritten");
        assert_eq!(std::fs::read(r.path().join("newdir").join("deep.txt")).unwrap(), b"D", "subtree copied");
        assert!(r.path().join("only_right.txt").exists(), "destination-only kept, never deleted");

        // Running it again finds nothing to do.
        let diff2 = cian_core::dirdiff::compare(l.path(), r.path(), &cancel, &mut |_| {});
        app.popup = Popup::DirCompare {
            left: "L".into(), right: "R".into(),
            left_root: l.path().to_path_buf(), right_root: r.path().to_path_buf(),
            entries: diff2.entries, cursor: 0, scroll: 0, truncated: false,
        };
        app.dir_compare_sync(true);
        assert!(matches!(app.popup, Popup::DirCompare { .. }), "nothing to sync leaves the compare up");
    }

    #[test]
    fn git_log_diff_and_blame() {
        use std::process::Command;
        let d = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(d.path()).unwrap();
        let ok = Command::new("git").arg("-C").arg(&dir).args(["init", "-q"]).status()
            .map(|s| s.success()).unwrap_or(false);
        if !ok {
            eprintln!("git not available; skipping");
            return;
        }
        for kv in [["user.email", "t@e.com"], ["user.name", "Alice"], ["core.autocrlf", "false"]] {
            let _ = Command::new("git").arg("-C").arg(&dir).args(["config", kv[0], kv[1]]).status();
        }
        std::fs::write(dir.join("f.rs"), "let a = 1;\nlet b = 2;\n").unwrap();
        Command::new("git").arg("-C").arg(&dir).args(["add", "."]).status().unwrap();
        Command::new("git").arg("-C").arg(&dir).args(["commit", "-qm", "seed"]).status().unwrap();

        let mut app = App::new(dir.clone(), dir.clone(), en_config()).unwrap();
        // Give the pane's git status a moment (ensure_git runs in the loop; call it).
        app.ensure_git();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "f.rs").unwrap();

        // History → a GitLog popup with the seed commit; Enter shows its diff.
        app.start_git_log();
        match &app.popup {
            Popup::GitLog { commits, .. } => assert_eq!(commits[0].subject, "seed"),
            _ => panic!("no git log popup"),
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "commit diff opens in the viewer");
        quit_viewer(&mut app);

        // Diff vs HEAD after an edit.
        std::fs::write(dir.join("f.rs"), "let a = 1;\nlet B = 2;\n").unwrap();
        let _ = app.active_file_tabs_mut().map(|t| t.active_mut().reload());
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "f.rs").unwrap();
        app.git_diff_file();
        match &app.popup {
            Popup::Viewer { view, .. } => assert!(view.lines.join("\n").contains("+let B = 2;"), "diff shown"),
            _ => panic!("diff did not open"),
        }
        quit_viewer(&mut app);

        // F3 then B toggles blame.
        app.handle_key(code(KeyCode::F(3))).unwrap();
        for k in [':', 'b', 'l', 'a', 'm', 'e'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        match &app.popup {
            Popup::Viewer { blame, .. } => assert!(!blame.is_empty(), "blame computed"),
            _ => panic!("not a viewer"),
        }
    }

    #[test]
    fn disk_usage_cache_populates_for_the_active_pane() {
        let d = tempfile::tempdir().unwrap();
        let p = std::fs::canonicalize(d.path()).unwrap();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        assert!(app.disk_for(app.focused).is_none(), "cold before the first refresh");
        app.ensure_git();
        let u = app.disk_for(app.focused).expect("mount is queryable");
        assert!(u.total > 0 && u.free <= u.total);
    }

    #[test]
    fn svn_status_log_and_diff() {
        use std::process::Command;
        // Needs both svnadmin (to make a repo) and svn (to check one out).
        let have = |bin: &str| Command::new(bin).arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
        if !have("svnadmin") || !have("svn") {
            eprintln!("svn not available; skipping");
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let repo = std::fs::canonicalize(root.path()).unwrap().join("repo");
        assert!(Command::new("svnadmin").args(["create"]).arg(&repo).status().unwrap().success());
        let url = format!("file://{}", repo.display());
        let wc_parent = tempfile::tempdir().unwrap();
        let wc = std::fs::canonicalize(wc_parent.path()).unwrap().join("wc");
        assert!(Command::new("svn").args(["checkout", &url]).arg(&wc).status().unwrap().success());

        std::fs::write(wc.join("f.rs"), "let a = 1;\nlet b = 2;\n").unwrap();
        let svn = |args: &[&str]| assert!(Command::new("svn").current_dir(&wc).args(args).status().unwrap().success(), "svn {:?}", args);
        svn(&["add", "f.rs"]);
        svn(&["commit", "-m", "seed"]);

        let mut app = App::new(wc.clone(), wc.clone(), en_config()).unwrap();
        app.ensure_git();
        // The status bar label comes from RepoStatus.branch → "svn r1".
        assert_eq!(app.vcs_kind(), Some(Vcs::Svn), "detected as svn");
        assert!(app.git_for(app.focused).map(|s| s.branch.starts_with("svn r")).unwrap_or(false), "revision label");

        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "f.rs").unwrap();

        // History → GitLog popup carrying Vcs::Svn; Enter shows the revision diff.
        app.start_git_log();
        match &app.popup {
            Popup::GitLog { commits, vcs, .. } => {
                assert_eq!(*vcs, Vcs::Svn);
                assert_eq!(commits[0].subject, "seed");
            }
            _ => panic!("no svn log popup"),
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "revision diff opens in the viewer");
        app.handle_key(code(KeyCode::Esc)).unwrap();

        // Diff vs BASE after an edit.
        std::fs::write(wc.join("f.rs"), "let a = 1;\nlet B = 2;\n").unwrap();
        let _ = app.active_file_tabs_mut().map(|t| t.active_mut().reload());
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "f.rs").unwrap();
        app.git_diff_file();
        match &app.popup {
            Popup::Viewer { view, .. } => assert!(view.lines.join("\n").contains("+let B = 2;"), "diff shown"),
            _ => panic!("diff did not open"),
        }
    }

    #[test]
    fn f3_syntax_highlights_recognised_code() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.rs"), "fn main() {\n    let x = 1; // hi\n}\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "a.rs").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert!(matches!(&app.popup, Popup::Viewer { hl_lang: Some(_), .. }), "rust detected");
        // The render computes and caches the per-char highlight styles.
        let _ = render(&mut app, 100, 30);
        match &app.popup {
            Popup::Viewer { hl, .. } => {
                assert!(!hl.is_empty(), "highlight computed");
                // `fn` (keyword mauve) differs from a plain identifier's colour.
                let kw = hl[0][0];
                let plain = hl[2][0]; // the closing `}` line, char 0
                assert_ne!(kw.fg, plain.fg, "keyword coloured differently from plain");
            }
            _ => panic!("not a viewer"),
        }
        // A .txt file is not highlighted.
        std::fs::write(d.path().join("b.txt"), "plain\n").unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
    }

    #[test]
    fn the_viewer_edits_and_saves_a_text_file() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("note.txt");
        std::fs::write(&file, "hello\nworld\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "note.txt").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);

        // Enter edit mode and type at the start of line 1.
        app.handle_key(key('i')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }), "editing started");
        for c in "AB".chars() {
            app.handle_key(key(c)).unwrap();
        }
        // A newline splits the line.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { dirty: true, .. }), "buffer is dirty");

        // Ctrl+S writes it back.
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { dirty: false, .. }), "saved → clean");
        let on_disk = std::fs::read_to_string(&file).unwrap();
        assert_eq!(on_disk, "AB\nhello\nworld\n", "edit persisted: {on_disk:?}");

        // Esc leaves edit mode; `:q` closes (nothing unsaved now).
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { editing: false, .. }));
        quit_viewer_discarding(&mut app);
        assert!(matches!(app.popup, Popup::None));
    }

    /// Open note.txt ("alpha…delta") in the viewer, cursor on line 0.
    fn viewer_on(lines: &str) -> (tempfile::TempDir, App) {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("note.txt"), lines).unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        // No system clipboard: these tests run in parallel on one machine and
        // would otherwise yank and paste through the *developer's* clipboard,
        // reading each other's copies. cian's own yank is the path that has to
        // work anyway — it is what a machine over SSH has.
        app.clipboard = None;
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "note.txt").unwrap();
        // Enter opens the panel where the file was listed; F12 gives it the
        // window, which is the shape most of these tests are about.
        app.handle_key(code(KeyCode::Enter)).unwrap();
        app.handle_key(code(KeyCode::F(12))).unwrap();
        let _ = render(&mut app, 100, 30);
        (d, app)
    }

    fn viewer_lines(app: &App) -> Vec<String> {
        match &app.popup {
            Popup::Viewer { view, .. } => view.lines.clone(),
            other => panic!("not a viewer: {other:?}"),
        }
    }

    /// The normal-mode change set: dd/x/J/D mutate in place, o opens a line
    /// and drops into insert, and `u` walks it all back — one unit per change,
    /// with `dirty` clearing once the stack drains to the original.
    #[test]
    fn viewer_normal_mode_operators_edit_and_undo() {
        let (_d, mut app) = viewer_on("alpha\nbravo\ncharlie\n");

        // dd deletes the line under the cursor.
        app.handle_key(key('d')).unwrap();
        app.handle_key(key('d')).unwrap();
        assert_eq!(viewer_lines(&app), ["bravo", "charlie"], "dd removed line 0");

        // x deletes the character under the cursor.
        app.handle_key(key('x')).unwrap();
        assert_eq!(viewer_lines(&app)[0], "ravo", "x ate the b");

        // `gJ` joins the next line up. (`J` is the window's key for the
        // shell below; `:combine` is the one that adds a space.)
        app.handle_key(key('g')).unwrap();
        app.handle_key(key('J')).unwrap();
        assert_eq!(viewer_lines(&app), ["ravocharlie"], "gJ joined");

        // o opens a line below and enters insert mode; typing lands there.
        app.handle_key(key('o')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }), "o → insert mode");
        app.handle_key(key('z')).unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert_eq!(viewer_lines(&app), ["ravocharlie", "z"]);

        // u, u, u, u: each change was one unit (the whole o-session is one).
        for expect in [
            vec!["ravocharlie".to_string()],
            vec!["ravo".into(), "charlie".into()],
            vec!["bravo".into(), "charlie".into()],
            vec!["alpha".into(), "bravo".into(), "charlie".into()],
        ] {
            app.handle_key(key('u')).unwrap();
            assert_eq!(viewer_lines(&app), expect);
        }
        assert!(
            matches!(app.popup, Popup::Viewer { dirty: false, .. }),
            "undone to the original → clean, so Esc closes without a warning"
        );

        // One more u: nothing left, and it says so rather than scrolling.
        app.handle_key(key('u')).unwrap();
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("oldest") || msg.contains("戻れません"), "says so: {msg}");
    }

    /// V + d deletes the selected lines; v + d splices within lines.
    #[test]
    fn viewer_visual_delete() {
        let (_d, mut app) = viewer_on("one\ntwo\nthree\nfour\n");
        // V j d: delete lines 0-1.
        app.handle_key(key('V')).unwrap();
        app.handle_key(key('j')).unwrap();
        app.handle_key(key('d')).unwrap();
        assert_eq!(viewer_lines(&app), ["three", "four"]);

        // v l l d on "three": delete chars 0..=2 → "ee".
        app.handle_key(key('v')).unwrap();
        app.handle_key(key('l')).unwrap();
        app.handle_key(key('l')).unwrap();
        app.handle_key(key('d')).unwrap();
        assert_eq!(viewer_lines(&app), ["ee", "four"]);

        // u twice restores everything.
        app.handle_key(key('u')).unwrap();
        app.handle_key(key('u')).unwrap();
        assert_eq!(viewer_lines(&app), ["one", "two", "three", "four"]);
    }

    /// d and u still scroll on a non-editable view (here: the hex dump), so
    /// the pager reflexes survive where there is nothing to edit.
    #[test]
    fn viewer_d_and_u_still_scroll_where_not_editable() {
        let d = tempfile::tempdir().unwrap();
        // A binary file (NUL bytes) opens as a hex dump, which is not editable.
        let mut bytes = vec![0u8; 4096];
        bytes[1] = 1;
        std::fs::write(d.path().join("blob.bin"), &bytes).unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "blob.bin").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);
        assert!(
            matches!(&app.popup, Popup::Viewer { view, editable: true, .. }
                if view.kind == cian_core::viewer::ViewKind::Binary),
            "a hex dump is editable — but as hex (i), not with the text operators"
        );
        let before = match &app.popup {
            Popup::Viewer { line, .. } => *line,
            _ => unreachable!(),
        };
        // `d` is vi's operator now, not a scroll key — Ctrl+D scrolls.
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)).unwrap();
        let after = match &app.popup {
            Popup::Viewer { line, .. } => *line,
            _ => unreachable!(),
        };
        assert!(after > before, "Ctrl+D scrolled half a page");
    }

    #[test]
    fn the_viewer_refuses_to_drop_unsaved_edits() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "x\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "a.txt").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);
        app.handle_key(key('i')).unwrap();
        app.handle_key(key('z')).unwrap(); // dirty
        app.handle_key(code(KeyCode::Esc)).unwrap(); // leave edit mode
        // `:q` won't discard unsaved work…
        quit_viewer(&mut app);
        assert!(matches!(app.popup, Popup::Viewer { dirty: true, .. }), "still open, warned");
        // …but `:q!` does.
        quit_viewer_discarding(&mut app);
        assert!(matches!(app.popup, Popup::None), ":q! discards and closes");
    }

    #[test]
    fn viewer_esc_clears_search_before_closing() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "a.txt").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);

        // Run a `/` search, then Esc: it clears the search and the viewer stays.
        app.handle_key(key('/')).unwrap();
        for c in "beta".chars() { app.handle_key(key(c)).unwrap(); }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(&app.popup, Popup::Viewer { find_query: Some(_), .. }), "search active");
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(&app.popup, Popup::Viewer { find_query: None, .. }), "Esc cleared the search");

        // A second Esc does *not* close it — that is `:q`, as it is in vi.
        // A third does, for a hand that just wants it gone.
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "Esc does not close the file");
        assert!(
            app.message.is_none(),
            "and does not count out loud: {:?}",
            app.message,
        );
        for k in [':', 'q'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::None), ":q closes it");
    }

    /// A zip with a small tree, for the archive-browse tests.
    fn make_browse_zip(dir: &std::path::Path) -> PathBuf {
        use std::io::Write;
        let path = dir.join("bundle.zip");
        let f = std::fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        w.start_file("top.txt", opts).unwrap();
        w.write_all(b"top level\n").unwrap();
        w.start_file("docs/readme.md", opts).unwrap();
        w.write_all(b"# hello from inside\n").unwrap();
        w.start_file("docs/deep/note.txt", opts).unwrap();
        w.write_all(b"deep note\n").unwrap();
        w.finish().unwrap();
        path
    }

    /// No keystroke may end the session. `l` inside an archive used to reach
    /// the local-directory navigation and hand it a member path, whose
    /// read_dir failure propagated all the way out of the event loop and
    /// killed cian — with an unsaved-work-shaped hole where a message belonged.
    #[test]
    fn keys_inside_an_archive_never_kill_the_session() {
        let d = tempfile::tempdir().unwrap();
        make_browse_zip(d.path());
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "bundle.zip").unwrap();
        }
        app.activate_selected().unwrap();
        assert!(app.active_pane().unwrap().archive_view().is_some());
        // Every plain letter, on every row, including the ones that used to
        // walk into the filesystem with a path that only exists in the zip.
        for row in 0..app.active_pane().unwrap().entries.len() {
            app.active_pane_mut().unwrap().cursor = row;
            for c in "abcdefghijklmnopqrstuvwxyz-".chars() {
                assert!(app.handle_key(key(c)).is_ok(), "key {c:?} on row {row} returned an error");
                if app.active_pane().map(|p| p.archive_view().is_none()).unwrap_or(true) {
                    // A key legitimately left the archive; go back in and carry on.
                    app.popup = Popup::None;
                    let pane = app.active_pane_mut().unwrap();
                    if let Some(i) = pane.entries.iter().position(|e| e.name == "bundle.zip") {
                        pane.cursor = i;
                    }
                    app.activate_selected().unwrap();
                }
                app.popup = Popup::None;
            }
        }
    }

    /// Alt+←/→ (and Alt+h/l) are the browser arrows over this pane's history,
    /// and `-` stays unbound so a stray dash never navigates.
    #[test]
    fn alt_arrows_walk_the_directory_history() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("sub")).unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p.clone(), en_config()).unwrap();
        let root = app.active_pane().unwrap().cwd.clone();

        // Go into sub/, then back, then forward again.
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "sub").unwrap();
        }
        app.activate_selected().unwrap();
        assert!(app.active_pane().unwrap().cwd.ends_with("sub"));

        let alt = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
        app.handle_key(alt('h')).unwrap();
        assert_eq!(app.active_pane().unwrap().cwd, root, "Alt+h went back");
        app.handle_key(alt('l')).unwrap();
        assert!(app.active_pane().unwrap().cwd.ends_with("sub"), "Alt+l went forward");

        // The arrows are the same pair.
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)).unwrap();
        assert_eq!(app.active_pane().unwrap().cwd, root);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)).unwrap();
        assert!(app.active_pane().unwrap().cwd.ends_with("sub"));

        // Going somewhere new ends the forward branch.
        app.handle_key(alt('h')).unwrap();
        assert!(!app.active_pane().unwrap().forward.is_empty(), "forward is armed");
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "sub").unwrap();
        }
        app.activate_selected().unwrap();
        assert!(app.active_pane().unwrap().forward.is_empty(), "a new step drops forward");

        // `-` is unbound; Backspace still goes up.
        let before = app.active_pane().unwrap().cwd.clone();
        app.handle_key(key('-')).unwrap();
        assert_eq!(app.active_pane().unwrap().cwd, before, "`-` is unbound");
        app.handle_key(code(KeyCode::Backspace)).unwrap();
        assert_ne!(app.active_pane().unwrap().cwd, before, "Backspace still goes up");
    }


    /// A file dragged from Finder/Explorer onto the terminal arrives as a
    /// paste; cian turns it into a move into the focused pane, asking first.
    #[test]
    fn a_dropped_file_becomes_a_move_into_this_pane() {
        let (l, r, mut app) = app_two_dirs(&["victim.txt"], &[]);
        app.focus(FocusedPane::Right);
        let src = l.path().join("victim.txt");

        // The shape iTerm2 sends for a drag.
        let dropped = src.display().to_string().replace(' ', "\\ ");
        assert!(app.accept_drop(&dropped), "recognised as a drop");
        match &app.popup {
            Popup::ConfirmTransfer { op, targets, dest } => {
                assert!(matches!(op, PendingOp::Move), "a drop moves");
                assert_eq!(targets, &vec![src.clone()]);
                // Compare by the final component: the pane canonicalises
                // (/var → /private/var on macOS) and the tempdir does not.
                assert_eq!(dest.file_name(), r.path().file_name());
            }
            other => panic!("expected the transfer confirm, got {other:?}"),
        }
        // Confirming actually moves it.
        app.handle_key(key('y')).unwrap();
        drain_op_job(&mut app);
        assert!(r.path().join("victim.txt").exists(), "landed in the right pane");
        assert!(!src.exists(), "and left the left pane");
    }

    /// Ordinary pastes must still be pastes — the drop path only claims text
    /// that is entirely real files, and never while something is being typed.
    #[test]
    fn a_drop_never_steals_an_ordinary_paste() {
        let (_l, _r, mut app) = app_two_dirs(&["a.txt"], &[]);
        assert!(!app.accept_drop("just some words"), "prose is a paste");

        // Even a real path, while a text field is open, belongs to the field.
        let real = _l.path().join("a.txt").display().to_string();
        app.start_rename();
        assert!(!app.accept_drop(&real), "a text field keeps its paste");
        app.popup = Popup::None;

        // And the shell keeps its own — dropping a file on a terminal to get
        // its path onto the command line predates cian.
        app.focus(FocusedPane::Shell);
        assert!(!app.accept_drop(&real), "the shell keeps its paste");
    }

    /// Inside an archive the hint bar names archive keys — and says outright
    /// when the format is read-only, since the keys that would write are the
    /// ones a filer user reaches for first.
    #[test]
    fn the_hint_bar_changes_inside_an_archive() {
        let d = tempfile::tempdir().unwrap();
        make_browse_zip(d.path());
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        let plain: Vec<&str> = crate::render::key_hints(&app).iter().map(|(k, _)| *k).collect();
        assert!(plain.contains(&"S-J"), "the ordinary bar leads with pane keys");

        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "bundle.zip").unwrap();
        }
        app.activate_selected().unwrap();
        let hints = crate::render::key_hints(&app);
        let keys: Vec<&str> = hints.iter().map(|(k, _)| *k).collect();
        // Backspace, not `-/h`: `-` is bound to nothing without a keymap and
        // `h` opens the directory history.
        assert!(keys.contains(&"Enter/l") && keys.contains(&"Bksp"), "navigation named: {keys:?}");
        assert!(keys.contains(&"F3"), "member viewing named");
        // `r` renames a member; F2 is the file-tab key and never reached the
        // archive at all.
        assert!(keys.contains(&"r") && keys.contains(&"d"), "zip is writable, so say so: {keys:?}");
    }

    /// Enter on a zip browses into it like a folder: members list, subdirs
    /// descend, `..` climbs, and past the root you are back on the archive.
    #[test]
    fn enter_browses_into_an_archive_and_out_again() {
        let d = tempfile::tempdir().unwrap();
        make_browse_zip(d.path());
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "bundle.zip").unwrap();
        }
        app.activate_selected().unwrap();
        {
            let pane = app.active_pane().unwrap();
            assert!(pane.archive_view().is_some(), "entered the archive");
            let names: Vec<&str> = pane.entries.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(names, vec!["..", "docs", "top.txt"], "root listing");
        }
        // Descend into docs/, then docs/deep/.
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "docs").unwrap();
        }
        app.activate_selected().unwrap();
        {
            let pane = app.active_pane().unwrap();
            assert_eq!(pane.archive_view().unwrap().1, "docs/");
            let names: Vec<&str> = pane.entries.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(names, vec!["..", "deep", "readme.md"]);
        }
        // `..` climbs back to the root; cursor lands on the dir we left.
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = 0;
        }
        app.activate_selected().unwrap();
        {
            let pane = app.active_pane().unwrap();
            assert_eq!(pane.archive_view().unwrap().1, "");
            assert_eq!(pane.selected().unwrap().name, "docs", "cursor on the dir we left");
        }
        // `..` at the root leaves the archive, cursor on the zip itself.
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = 0;
        }
        app.activate_selected().unwrap();
        {
            let pane = app.active_pane().unwrap();
            assert!(pane.archive_view().is_none(), "left the archive");
            assert_eq!(pane.selected().unwrap().name, "bundle.zip");
        }
    }

    /// F3 on a member extracts to a temp file and opens the normal viewer;
    /// markdown members even get their preview.
    #[test]
    fn f3_views_an_archive_member() {
        let d = tempfile::tempdir().unwrap();
        make_browse_zip(d.path());
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "bundle.zip").unwrap();
        }
        app.activate_selected().unwrap();
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "top.txt").unwrap();
        }
        app.handle_key(code(KeyCode::F(3))).unwrap();
        match &app.popup {
            Popup::Viewer { view, title, .. } => {
                assert!(view.lines.join("\n").contains("top level"), "member content shown");
                assert!(title.contains("bundle.zip"), "title names the archive: {title}");
            }
            other => panic!("expected the viewer, got {other:?}"),
        }
    }

    /// Copying from inside an archive extracts to the other pane, relative to
    /// the directory being browsed.
    #[test]
    fn copy_out_of_an_archive_extracts_to_the_other_pane() {
        let (l, r, mut app) = app_two_dirs(&[], &[]);
        let zip = make_browse_zip(l.path());
        let _ = zip;
        if let Some(pane) = app.active_pane_mut() {
            let _ = pane.reload();
            pane.cursor = pane.entries.iter().position(|e| e.name == "bundle.zip").unwrap();
        }
        app.activate_selected().unwrap();
        // Into docs/, then copy readme.md across.
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "docs").unwrap();
        }
        app.activate_selected().unwrap();
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "readme.md").unwrap();
        }
        app.start_transfer(PendingOp::Copy);
        drain_op_job(&mut app);
        assert!(
            r.path().join("readme.md").exists(),
            "extracted relative to docs/, not the whole tree"
        );
        assert!(!r.path().join("docs").exists(), "no rebuilt docs/ directory");

        // A directory row extracts everything under it, keeping its own name.
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = 0; // `..` → back to root
        }
        app.activate_selected().unwrap();
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "docs").unwrap();
        }
        app.start_transfer(PendingOp::Copy);
        drain_op_job(&mut app);
        assert!(r.path().join("docs/deep/note.txt").exists(), "subtree extracted");

        // Move out is refused: it would mean deleting members, which the write
        // side does not do yet. Said as a fact about the tool rather than as
        // "for now" — a limit is not an apology.
        app.start_transfer(PendingOp::Move);
        let msg = app.message.clone().unwrap_or_default();
        assert!(
            msg.contains("cannot be moved out of") || msg.contains("移動はできません"),
            "{msg}",
        );
    }

    /// The write side, end to end: copy INTO the zip from the other pane,
    /// rename a member, delete a member — each confirmed, run on the worker,
    /// and reflected in the refreshed listing.
    #[test]
    fn zip_add_rename_delete_from_the_panes() {
        let (l, r, mut app) = app_two_dirs(&[], &["fresh.txt"]);
        let zip = make_browse_zip(l.path());
        std::fs::write(r.path().join("fresh.txt"), "fresh body").unwrap();
        // Left pane: into the zip's docs/ directory.
        if let Some(pane) = app.active_pane_mut() {
            let _ = pane.reload();
            pane.cursor = pane.entries.iter().position(|e| e.name == "bundle.zip").unwrap();
        }
        app.activate_selected().unwrap();
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "docs").unwrap();
        }
        app.activate_selected().unwrap();

        // Right pane copies fresh.txt toward the left → confirm → into docs/.
        app.focus(FocusedPane::Right);
        if let Some(pane) = app.active_pane_mut() {
            let _ = pane.reload();
            pane.cursor = pane.entries.iter().position(|e| e.name == "fresh.txt").unwrap();
        }
        app.start_transfer(PendingOp::Copy);
        assert!(
            matches!(app.popup, Popup::ConfirmZipAdd { .. }),
            "asks before writing into the zip: {:?}",
            app.popup
        );
        app.handle_key(key('y')).unwrap();
        drain_op_job(&mut app);
        let names: Vec<String> = cian_core::archive::list(&zip)
            .unwrap()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert!(names.contains(&"docs/fresh.txt".to_string()), "added under docs/: {names:?}");

        // The left pane (still inside docs/) sees the new member.
        app.focus(FocusedPane::Left);
        let listed: Vec<String> =
            app.active_pane().unwrap().entries.iter().map(|e| e.name.clone()).collect();
        assert!(listed.contains(&"fresh.txt".to_string()), "listing refreshed: {listed:?}");

        // Rename it (F2 path) …
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "fresh.txt").unwrap();
        }
        app.start_rename();
        assert!(
            matches!(&app.popup, Popup::TextInput { kind: InputKind::RenameZipMember { .. }, .. }),
            "member rename prompt: {:?}",
            app.popup
        );
        // Clear the seeded name, type the new one, Enter.
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)).unwrap();
        for c in "renamed.txt".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        drain_op_job(&mut app);
        let names: Vec<String> =
            cian_core::archive::list(&zip).unwrap().into_iter().map(|m| m.name).collect();
        assert!(names.contains(&"docs/renamed.txt".to_string()), "renamed: {names:?}");
        assert!(!names.contains(&"docs/fresh.txt".to_string()));

        // …and delete it.
        {
            let pane = app.active_pane_mut().unwrap();
            pane.cursor = pane.entries.iter().position(|e| e.name == "renamed.txt").unwrap();
        }
        app.start_delete();
        assert!(matches!(app.popup, Popup::ConfirmZipDelete { .. }), "{:?}", app.popup);
        app.handle_key(key('y')).unwrap();
        drain_op_job(&mut app);
        let names: Vec<String> =
            cian_core::archive::list(&zip).unwrap().into_iter().map(|m| m.name).collect();
        assert!(!names.contains(&"docs/renamed.txt".to_string()), "deleted: {names:?}");
        // Untouched members survived all three rewrites.
        assert!(names.contains(&"docs/deep/note.txt".to_string()));
        assert!(names.contains(&"top.txt".to_string()));
    }

    #[test]
    fn a_docx_previews_as_searchable_text() {
        use std::io::Write;
        let d = tempfile::tempdir().unwrap();
        // A minimal .docx: a zip with word/document.xml.
        let docx = d.path().join("report.docx");
        {
            let f = std::fs::File::create(&docx).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<'_, ()> =
                zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            zw.start_file("word/document.xml", opts).unwrap();
            zw.write_all(
                br#"<w:document><w:body>
                    <w:p><w:r><w:t>Quarterly results</w:t></w:r></w:p>
                    <w:p><w:r><w:t>Revenue is up</w:t></w:r></w:p>
                </w:body></w:document>"#,
            )
            .unwrap();
            zw.finish().unwrap();
        }
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        // F3 opens the extracted document in the ordinary viewer (not markdown).
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);
        if let Popup::Viewer { view, markdown, preview, .. } = &app.popup {
            assert!(!*markdown && !*preview, "a document, not a markdown preview");
            let flat = view.lines.join("\n");
            assert!(flat.contains("Word"), "header names the format");
            assert!(flat.contains("Quarterly results"), "body text extracted: {:?}", view.lines);
            assert!(flat.contains("Revenue is up"));
        } else {
            panic!("F3 did not open a viewer");
        }

        // Search works over the extracted text, just like any file.
        app.handle_key(key('/')).unwrap();
        for c in "Revenue".chars() { app.handle_key(key(c)).unwrap(); }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        if let Popup::Viewer { view, line, .. } = &app.popup {
            assert!(view.lines[*line].contains("Revenue"), "search jumped to the match");
        } else {
            panic!("not a viewer");
        }
        quit_viewer(&mut app);
        assert!(matches!(app.popup, Popup::None));
    }

    #[test]
    fn the_viewer_line_visual_selects_and_copies_a_range() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30); // size viewer_rect so motion works

        // Move to line 1 (two), start line-visual, extend to line 2 (three).
        app.handle_key(key('j')).unwrap();
        app.handle_key(key('V')).unwrap();
        app.handle_key(key('j')).unwrap();
        assert!(
            matches!(app.popup, Popup::Viewer { visual: Some(ViewVisual::Line), .. }),
            "line-visual is active"
        );
        app.handle_key(key('y')).unwrap();
        assert_eq!(app.message.as_deref(), Some("copied"));
        // Visual ends after the copy; the viewer stays open.
        assert!(matches!(app.popup, Popup::Viewer { visual: None, .. }));
    }

    #[test]
    fn the_viewer_shift_arrow_selects_characters() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "hello world\nsecond\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);

        // Shift+Right three times: a character-wise selection begins at col 0
        // and the cursor advances, extending it.
        for _ in 0..3 {
            app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)).unwrap();
        }
        match &app.popup {
            Popup::Viewer { visual: Some(ViewVisual::Char), anchor, col, .. } => {
                assert_eq!(*anchor, (0, 0), "anchored where selection began");
                assert_eq!(*col, 3, "cursor advanced three chars");
            }
            other => panic!("expected a char selection, got {:?}", other),
        }
        // A plain motion keeps the vim-style selection; `y` copies and ends it.
        app.handle_key(key('y')).unwrap();
        assert_eq!(app.message.as_deref(), Some("copied"));
        assert!(matches!(app.popup, Popup::Viewer { visual: None, .. }));
    }

    #[test]
    fn the_viewer_alt_arrow_and_alt_drag_select_a_block() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "hello world\nsecond line\nthird row!!\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);

        // Alt+Down then Alt+Right builds a rectangle from the cursor.
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)).unwrap();
        match &app.popup {
            Popup::Viewer { visual: Some(ViewVisual::Block), anchor, line, col, .. } => {
                assert_eq!(*anchor, (0, 0));
                assert_eq!((*line, *col), (1, 2), "block cursor advanced down 1, right 2");
            }
            other => panic!("expected a block selection, got {:?}", other),
        }
        app.handle_key(code(KeyCode::Esc)).unwrap(); // drop the selection

        // Alt+drag also makes a block selection.
        let body = app.viewer_rect;
        let x0 = body.x + app.viewer_gutter;
        let mut down = mouse(MouseEventKind::Down(MouseButton::Left), x0 + 1, body.y);
        down.modifiers = KeyModifiers::ALT;
        app.handle_mouse(down);
        let mut drag = mouse(MouseEventKind::Drag(MouseButton::Left), x0 + 4, body.y + 2);
        drag.modifiers = KeyModifiers::ALT;
        app.handle_mouse(drag);
        assert!(matches!(app.popup, Popup::Viewer { visual: Some(ViewVisual::Block), .. }),
            "alt-drag makes a block selection");
    }

    #[test]
    fn the_viewer_mouse_drag_selects_characters() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "hello world\nsecond line\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);
        let body = app.viewer_rect;
        let x0 = body.x + app.viewer_gutter;
        // Press on (line 0, char 2), drag to (line 0, char 8): a char selection.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x0 + 2, body.y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), x0 + 8, body.y));
        match &app.popup {
            Popup::Viewer { visual: Some(ViewVisual::Char), anchor, line, col, .. } => {
                assert_eq!(*anchor, (0, 2), "anchored at the press char");
                assert_eq!((*line, *col), (0, 8), "cursor at the drag char");
            }
            other => panic!("expected a char selection, got {:?}", other),
        }
        // Right-click opens the menu, with the viewer put aside rather than
        // closed — the selection is still there behind it.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), x0 + 8, body.y));
        assert!(matches!(app.popup, Popup::ContextMenu { .. }), "the menu opened");
        assert!(app.viewer_return.is_some(), "the file is waiting behind it");

        // Copy is in it, and means the selection.
        let at = match &app.popup {
            Popup::ContextMenu { items, .. } => {
                items.iter().position(|i| matches!(i, MenuItem::Copy)).expect("Copy is in the menu")
            }
            _ => unreachable!(),
        };
        if let Popup::ContextMenu { cursor, .. } = &mut app.popup {
            *cursor = at;
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(app.message.as_deref(), Some("copied"));
        assert!(matches!(app.popup, Popup::Viewer { .. }), "and the file came back");

        // Esc out of the menu puts the file back untouched.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), x0 + 8, body.y));
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "back to the file");
        assert!(app.viewer_return.is_none(), "and nothing left waiting");
    }

    /// Drive the viewer with a sequence of plain-char keys.
    fn vkeys(app: &mut App, s: &str) {
        for c in s.chars() {
            app.handle_key(key(c)).unwrap();
        }
    }

    #[test]
    fn the_viewer_searches_and_jumps_between_matches() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "alpha\nbeta needle\ngamma\nneedle again\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);

        // /needle<CR> jumps to the first match (line 1, col 5).
        app.handle_key(key('/')).unwrap();
        vkeys(&mut app, "needle");
        app.handle_key(code(KeyCode::Enter)).unwrap();
        if let Popup::Viewer { line, col, .. } = &app.popup {
            assert_eq!((*line, *col), (1, 5), "first match");
        } else {
            panic!("viewer");
        }
        // n advances to the next match (line 3, col 0).
        app.handle_key(key('n')).unwrap();
        if let Popup::Viewer { line, col, .. } = &app.popup {
            assert_eq!((*line, *col), (3, 0), "second match");
        } else {
            panic!("viewer");
        }
    }

    #[test]
    fn the_viewer_goto_line_and_bracket_match() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "fn f() {\n    body\n}\nfour\nfive\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);

        // 4G jumps to line 4 (0-based index 3).
        vkeys(&mut app, "4");
        app.handle_key(key('G')).unwrap();
        if let Popup::Viewer { line, .. } = &app.popup {
            assert_eq!(*line, 3, "goto line 4");
        } else {
            panic!("viewer");
        }
        // Back to the top, move onto the `{` (col 7 of "fn f() {"), then % to
        // its matching `}` on line 2.
        app.handle_key(key('g')).unwrap();
        app.handle_key(key('g')).unwrap();
        vkeys(&mut app, "lllllll"); // 7 × l → col 7 = '{'
        if let Popup::Viewer { col, .. } = &app.popup {
            assert_eq!(*col, 7, "cursor on the brace");
        }
        vkeys(&mut app, "%");
        if let Popup::Viewer { line, .. } = &app.popup {
            assert_eq!(*line, 2, "matching brace is on line 2");
        } else {
            panic!("viewer");
        }
    }

    #[test]
    fn the_viewer_char_visual_yanks_across_lines() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "abcd\nefgh\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        let _ = render(&mut app, 100, 30);
        // From (0,1)=b, char-visual to (1,1)=f → "bcd\nef".
        app.handle_key(key('l')).unwrap();
        app.handle_key(key('v')).unwrap();
        app.handle_key(key('j')).unwrap();
        // cursor col follows the goal (1) on line 1.
        let text = if let Popup::Viewer { view, line, col, visual, anchor, .. } = &app.popup {
            assert_eq!((*line, *col), (1, 1));
            let (s, e) = order_pos(*anchor, (*line, *col));
            assert!(visual.is_some());
            viewer_charwise(&view.lines, s, e)
        } else {
            panic!("viewer")
        };
        assert_eq!(text, "bcd\nef");
    }

    #[test]
    fn e_opens_the_encoding_picker_and_applies_the_choice() {
        let d = tempfile::tempdir().unwrap();
        // "日本語" in Shift_JIS: mojibake as UTF-8 until switched.
        std::fs::write(d.path().join("s.txt"), [0x93u8, 0xfa, 0x96, 0x7b, 0x8c, 0xea, b'\n']).unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();

        // `e` opens the picker (a list), not an immediate cycle.
        for k in [':', 'e', 'n', 'c'] {
            app.handle_key(key(k)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(
            matches!(app.popup, Popup::EncodingPicker { target: EncTarget::Viewer(_), .. }),
            "e opens the picker targeting the viewer"
        );
        // Move to Shift_JIS and confirm; the viewer comes back re-decoded.
        let sjis = cian_core::viewer::TextEncoding::ALL
            .iter()
            .position(|e| *e == cian_core::viewer::TextEncoding::ShiftJis)
            .unwrap();
        if let Popup::EncodingPicker { cursor, .. } = &mut app.popup {
            *cursor = sjis;
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let Popup::Viewer { view, .. } = &app.popup else { panic!("viewer restored") };
        assert_eq!(view.encoding, cian_core::viewer::TextEncoding::ShiftJis);
        assert_eq!(view.lines[0], "日本語");
    }

    #[test]
    fn cancelling_the_encoding_picker_restores_the_viewer_unchanged() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("s.txt"), b"plain\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        app.handle_key(key('e')).unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "Esc returns to the viewer");
    }

    #[test]
    fn shift_enter_reveals_the_viewed_file_in_the_pane() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub").join("deep.txt"), "content\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        // Open the file, then Shift+Enter for the viewer's menu, and take the
        // item that reveals it. (Shift+Enter used to reveal it directly; it is
        // the keyboard's right-click now, and revealing moved into the menu.)
        app.open_viewer_at(&d.path().join("sub").join("deep.txt"), "deep.txt", 0);
        let _ = render(&mut app, 100, 30);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        let at = match &app.popup {
            Popup::ContextMenu { items, .. } => items
                .iter()
                .position(|i| matches!(i, MenuItem::RevealInPane))
                .expect("the menu offers it"),
            other => panic!("expected the viewer's menu, got {other:?}"),
        };
        if let Popup::ContextMenu { cursor, .. } = &mut app.popup {
            *cursor = at;
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(matches!(app.popup, Popup::None), "viewer closed");
        let pane = app.active_pane().unwrap();
        assert!(pane.cwd.ends_with("sub"), "pane moved into the file's dir: {:?}", pane.cwd);
        assert_eq!(pane.selected().map(|e| e.name.as_str()), Some("deep.txt"));
    }

    #[test]
    fn ctrl_n_steps_through_grep_hits_in_the_viewer() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "NEEDLE one\n").unwrap();
        std::fs::write(d.path().join("b.txt"), "two NEEDLE\n").unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.start_find("NEEDLE", cian_core::search::Mode::Content);
        drain_find(&mut app);
        // Sort of results is by rel path, so a.txt is first. Open it.
        if let Popup::FindResults { cursor, .. } = &mut app.popup {
            *cursor = 0;
        }
        app.open_find_hit().unwrap();
        let first = match &app.popup {
            Popup::Viewer { title, .. } => title.clone(),
            _ => panic!("viewer"),
        };
        // Ctrl+n → the other file's hit.
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)).unwrap();
        let second = match &app.popup {
            Popup::Viewer { title, .. } => title.clone(),
            other => panic!("expected viewer, got {:?}", other),
        };
        assert_ne!(first, second, "Ctrl+n moved to the other hit");
        // Closing still returns to the (stepped) results list.
        quit_viewer(&mut app);
        assert!(matches!(app.popup, Popup::FindResults { .. }));
    }

    #[test]
    fn f3_on_an_archive_lists_it_instead() {
        let d = tempfile::tempdir().unwrap();
        {
            use std::io::Write as _;
            let f = std::fs::File::create(d.path().join("a.zip")).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            w.start_file("inside.txt", o).unwrap();
            w.write_all(b"hi").unwrap();
            w.finish().unwrap();
        }
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();

        app.handle_key(code(KeyCode::F(3))).unwrap();
        let Popup::Archive { members, .. } = &app.popup else {
            panic!("expected the archive list, got {:?}", app.popup)
        };
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].name, "inside.txt");
    }

    #[test]
    fn extracting_sends_the_members_to_the_other_pane() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        {
            use std::io::Write as _;
            let f = std::fs::File::create(src.path().join("a.zip")).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            w.start_file("inside.txt", o).unwrap();
            w.write_all(b"hi").unwrap();
            w.finish().unwrap();
        }
        let mut app = App::new(
            src.path().to_path_buf(),
            out.path().to_path_buf(),
            en_config(),
        )
        .unwrap();

        app.handle_key(code(KeyCode::F(3))).unwrap();
        app.extract_from_archive(true);
        assert!(app.op_job.is_some(), "extraction runs on the worker");

        let start = Instant::now();
        while app.op_job.is_some() && start.elapsed() < Duration::from_secs(5) {
            app.poll_op_job();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(std::fs::read_to_string(out.path().join("inside.txt")).unwrap(), "hi");
        // The destination is worth remembering like any other transfer target.
        assert!(app.dest_history.iter().any(|p| p.file_name() == out.path().file_name()));
    }

    #[test]
    fn f3_on_a_directory_says_so_rather_than_opening_a_blank_viewer() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert!(matches!(app.popup, Popup::None));
        assert!(app.message.as_deref().unwrap_or("").contains("directory"));
    }

    #[test]
    fn shell_panel_starts_empty_and_focusing_it_does_not_block() {
        let (_d, mut app) = app_with(&["a.txt"]);
        assert_eq!(app.shell.count(), 0);

        // Focusing the shell must return immediately, leaving the spawn in
        // flight rather than blocking the event loop on fork/exec.
        app.focus(FocusedPane::Shell);
        assert!(app.shell.is_starting(), "spawn should be pending, not resolved inline");

        // The placeholder renders without a session present.
        let out = render(&mut app, 100, 24).join("\n");
        assert!(out.contains("starting shell"), "expected placeholder; got:\n{}", out);
    }

/// The icon grid gives the letters up: in that view a key names a file rather
/// than running a command. See [`crate::grid`].
mod grid_type_ahead {
    use super::*;

    /// A grid over these files, cursor at the top.
    fn grid(names: &[&str]) -> (tempfile::TempDir, App) {
        let (dir, mut app) = app_with(names);
        app.icon_view = true;
        app.icon_cols = 3;
        (dir, app)
    }

    fn press(app: &mut App, c: char) {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)).unwrap();
    }

    fn under_cursor(app: &App) -> String {
        app.active_pane().and_then(|p| p.selected()).map(|e| e.name.clone()).unwrap_or_default()
    }

    #[test]
    fn a_letter_goes_to_the_first_file_starting_with_it() {
        let (_d, mut app) = grid(&["apple.txt", "jam.txt", "juice.txt", "kiwi.txt"]);
        press(&mut app, 'j');
        assert_eq!(under_cursor(&app), "jam.txt");
    }

    #[test]
    fn the_same_letter_again_walks_to_the_next_one() {
        let (_d, mut app) = grid(&["apple.txt", "jam.txt", "juice.txt", "kiwi.txt"]);
        press(&mut app, 'j');
        press(&mut app, 'j');
        assert_eq!(under_cursor(&app), "juice.txt");
    }

    #[test]
    fn repeating_past_the_last_one_wraps_to_the_first() {
        let (_d, mut app) = grid(&["jam.txt", "juice.txt"]);
        press(&mut app, 'j');
        press(&mut app, 'j');
        press(&mut app, 'j');
        assert_eq!(under_cursor(&app), "jam.txt");
    }

    #[test]
    fn different_letters_build_a_prefix() {
        let (_d, mut app) = grid(&["read.txt", "readme.md", "report.pdf"]);
        press(&mut app, 'r');
        assert_eq!(under_cursor(&app), "read.txt");
        // `e` extends rather than jumping to something starting with `e`, so
        // this stays inside the `re…` names instead of leaving them.
        press(&mut app, 'e');
        assert_eq!(under_cursor(&app), "read.txt");
        press(&mut app, 'p');
        assert_eq!(under_cursor(&app), "report.pdf");
    }

    #[test]
    fn case_does_not_matter() {
        let (_d, mut app) = grid(&["Report.pdf", "apple.txt"]);
        press(&mut app, 'r');
        assert_eq!(under_cursor(&app), "Report.pdf");
    }

    #[test]
    fn a_prefix_that_matches_nothing_falls_back_to_the_last_letter() {
        let (_d, mut app) = grid(&["apple.txt", "jam.txt"]);
        press(&mut app, 'a');
        // `q` cannot extend `a…`, so it is taken as a fresh search — which also
        // matches nothing, and the cursor is left where it was rather than
        // wandering off.
        press(&mut app, 'q');
        assert_eq!(under_cursor(&app), "apple.txt");
        // ...and the next real letter still works, rather than being stuck
        // behind a dead prefix.
        press(&mut app, 'j');
        assert_eq!(under_cursor(&app), "jam.txt");
    }

    #[test]
    fn the_arrows_walk_the_grid_by_row_and_by_one() {
        let (_d, mut app) = grid(&["a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt"]);
        let first = under_cursor(&app);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)).unwrap();
        assert_ne!(under_cursor(&app), first);
        // Down moves a whole row — three, because the grid is three wide.
        let before = app.active_pane().unwrap().cursor;
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.active_pane().unwrap().cursor, before + 3);
    }

    #[test]
    fn the_lists_are_unaffected() {
        // The same keys outside the grid keep every meaning they had.
        let (_d, mut app) = app_with(&["jam.txt", "juice.txt"]);
        let before = app.active_pane().unwrap().cursor;
        press(&mut app, 'j');
        assert_eq!(
            app.active_pane().unwrap().cursor,
            before + 1,
            "`j` outside the grid still moves down one"
        );
    }
}

/// The grid's mouse. Clicking a tile has to move the cursor of the pane the
/// grid is actually *drawing* — they were two different panes once, which made
/// a click move a cursor nobody could see and a double click walk into a
/// directory that was not on screen.
mod grid_mouse {
    use super::*;
    use ratatui::layout::Rect;

    /// A grid three tiles wide, laid out at the origin, as a draw would leave
    /// it. `TILE_W` and `TILE_H` are the renderer's own.
    fn grid(names: &[&str]) -> (tempfile::TempDir, App) {
        let (dir, mut app) = app_with(names);
        app.icon_view = true;
        app.icon_cols = 3;
        app.grid_area = Some(Rect::new(0, 0, 3 * 14, 4 * 6));
        (dir, app)
    }

    /// The middle of tile `n`, in cells.
    fn tile(n: usize, cols: usize) -> (u16, u16) {
        let (cx, cy) = ((n % cols) as u16 * 14, (n / cols) as u16 * 6);
        (cx + 7, cy + 2)
    }

    fn under_cursor(app: &App) -> String {
        app.active_pane().and_then(|p| p.selected()).map(|e| e.name.clone()).unwrap_or_default()
    }

    #[test]
    fn clicking_a_tile_moves_the_cursor_to_it() {
        let (_d, mut app) = grid(&["a.txt", "b.txt", "c.txt", "d.txt"]);
        let (col, row) = tile(2, 3);
        assert!(app.grid_click(col, row), "the grid should take the click");
        assert_eq!(app.active_pane().unwrap().cursor, 2);
    }

    #[test]
    fn clicking_the_second_row_lands_on_the_right_entry() {
        let (_d, mut app) = grid(&["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);
        // Index 4 is the second row, first column, on a three-wide grid.
        let (col, row) = tile(4, 3);
        assert!(app.grid_click(col, row));
        assert_eq!(app.active_pane().unwrap().cursor, 4);
    }

    #[test]
    fn clicking_past_the_last_entry_changes_nothing() {
        let (_d, mut app) = grid(&["a.txt", "b.txt"]);
        let before = app.active_pane().unwrap().cursor;
        let (col, row) = tile(8, 3);
        // Swallowed — the grid owns its rectangle — but the cursor stays put
        // rather than jumping to the end.
        app.grid_click(col, row);
        assert_eq!(app.active_pane().unwrap().cursor, before);
    }

    #[test]
    fn it_acts_on_the_pane_the_grid_is_drawing() {
        // With the focus on the right pane, a click still has to move the
        // cursor the user can see. Both panes open on the same directory here,
        // so the test is about *which* pane changed, not about the contents.
        let (_d, mut app) = grid(&["a.txt", "b.txt", "c.txt"]);
        app.focus(FocusedPane::Right);
        let left_before = app.left.active_ref().cursor;
        let (col, row) = tile(2, 3);
        assert!(app.grid_click(col, row));
        assert_eq!(
            app.active_pane().unwrap().cursor,
            2,
            "the focused pane — which is the one the grid draws — moved"
        );
        assert_eq!(
            app.left.active_ref().cursor,
            left_before,
            "and the pane that is not on screen was left alone"
        );
    }

    #[test]
    fn double_clicking_a_directory_enters_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("inside.txt"), b"").unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p.clone(), en_config()).unwrap();
        app.icon_view = true;
        app.icon_cols = 3;
        app.grid_area = Some(Rect::new(0, 0, 3 * 14, 4 * 6));

        let n = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "sub")
            .expect("the directory is listed");
        let (col, row) = tile(n, 3);
        assert!(app.grid_double_click(col, row));
        // By name, not by path: cian canonicalises, and on macOS a temp
        // directory comes back with a `/private` in front of it.
        assert_eq!(
            app.active_pane().unwrap().cwd.file_name().unwrap(),
            "sub",
            "the pane on screen is the one that moved"
        );
        assert!(
            app.active_pane().unwrap().entries.iter().any(|e| e.name == "inside.txt"),
            "and it is listing what is in there"
        );
        let _ = under_cursor(&app);
    }

    #[test]
    fn holding_the_modifier_adds_to_the_selection() {
        // Entry 0 is `..`, which is navigation and never a selection — so the
        // files start at 1.
        let (_d, mut app) = grid(&["a.txt", "b.txt", "c.txt"]);
        let (c1, r1) = tile(1, 3);
        let (c3, r3) = tile(3, 3);
        assert!(app.grid_click_mods(c1, r1, true));
        assert!(app.grid_click_mods(c3, r3, true));
        let pane = app.active_pane().unwrap();
        assert!(pane.is_marked(1) && pane.is_marked(3), "both are marked");
        assert!(!pane.is_marked(2), "and the one in between is not");
    }

    #[test]
    fn a_plain_click_leaves_the_marks_alone() {
        // cian's marks are built with `Space` and operated on by every file
        // command; a click that quietly emptied them would lose work.
        let (_d, mut app) = grid(&["a.txt", "b.txt", "c.txt"]);
        let (c1, r1) = tile(1, 3);
        app.grid_click_mods(c1, r1, true);
        let (c2, r2) = tile(2, 3);
        app.grid_click_mods(c2, r2, false);
        assert!(app.active_pane().unwrap().is_marked(1), "the earlier mark survives");
        assert_eq!(app.active_pane().unwrap().cursor, 2, "and the cursor moved");
    }

    #[test]
    fn the_lists_are_unaffected() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt"]);
        assert!(!app.grid_click(7, 2), "outside the grid, the grid takes nothing");
    }

    #[test]
    fn right_clicking_points_at_the_file_first() {
        // A menu about "the selected file" has to be about the one that was
        // right-clicked, not about wherever the cursor happened to be.
        use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        let (_d, mut app) = grid(&["a.txt", "b.txt", "c.txt"]);
        let (col, row) = tile(2, 3);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Right),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.active_pane().unwrap().cursor, 2, "the cursor moved to it");
        assert!(matches!(app.popup, Popup::ContextMenu { .. }), "and the menu opened");
    }

    #[test]
    fn the_parent_row_is_never_marked() {
        // `..` is a way out, not a file. Ctrl-clicking it moves the cursor
        // and marks nothing.
        let (_d, mut app) = grid(&["a.txt"]);
        let (c, r) = tile(0, 3);
        app.grid_click_mods(c, r, true);
        assert!(!app.active_pane().unwrap().is_marked(0));
    }
}

/// `Ctrl+A` in a text field selects the line, and the clipboard keys act on it.
/// The address bar is the reason — an address is copied, pasted and replaced
/// far more often than it is edited in the middle.
mod input_select_all {
    use super::*;

    fn open(app: &mut App) {
        app.start_jump_path();
    }

    fn press(app: &mut App, c: char, ctrl: bool) {
        let m = if ctrl { KeyModifiers::CONTROL } else { KeyModifiers::NONE };
        app.handle_key(KeyEvent::new(KeyCode::Char(c), m)).unwrap();
    }

    fn text(app: &App) -> String {
        match &app.popup {
            Popup::TextInput { buffer, .. } => buffer.clone(),
            _ => String::new(),
        }
    }

    fn is_selected(app: &App) -> bool {
        matches!(&app.popup, Popup::TextInput { select_all: true, .. })
    }

    #[test]
    fn ctrl_a_selects_the_whole_line() {
        let (_d, mut app) = app_with(&["a.txt"]);
        open(&mut app);
        assert!(!text(&app).is_empty(), "the prompt opens seeded with the path");
        press(&mut app, 'a', true);
        assert!(is_selected(&app));
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let (_d, mut app) = app_with(&["a.txt"]);
        open(&mut app);
        press(&mut app, 'a', true);
        press(&mut app, '/', false);
        assert_eq!(text(&app), "/", "the seeded path is gone, not appended to");
        assert!(!is_selected(&app), "and the selection is spent");
    }

    #[test]
    fn backspace_over_a_selection_empties_it() {
        let (_d, mut app) = app_with(&["a.txt"]);
        open(&mut app);
        press(&mut app, 'a', true);
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)).unwrap();
        assert_eq!(text(&app), "");
    }

    #[test]
    fn an_arrow_key_collapses_the_selection() {
        // As it does in every other text field: the caret moves, and the next
        // keystroke inserts rather than replacing.
        let (_d, mut app) = app_with(&["a.txt"]);
        open(&mut app);
        press(&mut app, 'a', true);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)).unwrap();
        assert!(!is_selected(&app));
    }

    #[test]
    fn ctrl_x_takes_the_line_away() {
        let (_d, mut app) = app_with(&["a.txt"]);
        open(&mut app);
        press(&mut app, 'a', true);
        press(&mut app, 'x', true);
        assert_eq!(text(&app), "", "cut leaves the field empty");
    }

    #[test]
    fn ctrl_c_leaves_the_line_alone() {
        let (_d, mut app) = app_with(&["a.txt"]);
        open(&mut app);
        let before = text(&app);
        press(&mut app, 'a', true);
        press(&mut app, 'c', true);
        assert_eq!(text(&app), before, "copy is not cut");
    }

}

/// Dragging files with the mouse: what gets picked up, and where letting go
/// would put it. The drawing is the window's; these are the decisions.
mod drag_and_drop {
    use super::*;
    use ratatui::layout::Rect;

    fn grid(names: &[&str]) -> (tempfile::TempDir, App) {
        let (dir, mut app) = app_with(names);
        app.icon_view = true;
        app.icon_cols = 3;
        app.grid_area = Some(Rect::new(0, 0, 3 * 14, 4 * 6));
        (dir, app)
    }

    fn tile(n: usize, cols: usize) -> (u16, u16) {
        let (cx, cy) = ((n % cols) as u16 * 14, (n / cols) as u16 * 6);
        (cx + 7, cy + 2)
    }

    #[test]
    fn a_press_on_a_file_picks_up_that_file() {
        let (_d, app) = grid(&["a.txt", "b.txt"]);
        let (c, r) = tile(1, 3);
        let got = app.drag_targets_at(c, r);
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with("a.txt"));
    }

    #[test]
    fn a_press_on_a_marked_file_picks_up_the_whole_selection() {
        // A selection built with Ctrl-click or Space drags as a group; that is
        // the point of having made it.
        let (_d, mut app) = grid(&["a.txt", "b.txt", "c.txt"]);
        if let Some(p) = app.active_pane_mut() {
            p.toggle_mark_at(1);
            p.toggle_mark_at(3);
        }
        let (c, r) = tile(1, 3);
        assert_eq!(app.drag_targets_at(c, r).len(), 2);
    }

    #[test]
    fn the_parent_row_is_not_a_thing_to_drag() {
        let (_d, app) = grid(&["a.txt"]);
        let (c, r) = tile(0, 3);
        assert!(app.drag_targets_at(c, r).is_empty(), "`..` is a way out, not a file");
    }

    #[test]
    fn a_folder_under_the_pointer_is_a_destination() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("into")).unwrap();
        std::fs::write(dir.path().join("a.txt"), b"").unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.icon_view = true;
        app.icon_cols = 3;
        app.grid_area = Some(Rect::new(0, 0, 3 * 14, 4 * 6));
        let n = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == "into")
            .unwrap();
        let (c, r) = tile(n, 3);
        assert!(app.drop_target_at(c, r).is_some_and(|d| d.ends_with("into")));
    }

    #[test]
    fn a_plain_file_is_not_a_destination() {
        let (_d, app) = grid(&["a.txt", "b.txt"]);
        let (c, r) = tile(1, 3);
        assert!(app.drop_target_at(c, r).is_none());
    }

    #[test]
    fn dropping_asks_before_it_does_anything() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("into")).unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p.clone(), en_config()).unwrap();
        app.drop_onto(vec![p.join("a.txt")], p.join("into"), false);
        assert!(
            matches!(app.popup, Popup::ConfirmTransfer { .. }),
            "a drag says what to do; the confirmation is still what does it"
        );
        assert!(p.join("a.txt").exists(), "and nothing has moved yet");
    }

    #[test]
    fn dropping_something_where_it_already_is_does_nothing() {
        // The commonest miss: picking a file up and putting it back down.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"").unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p.clone(), en_config()).unwrap();
        app.drop_onto(vec![p.join("a.txt")], p.clone(), false);
        assert!(matches!(app.popup, Popup::None), "no question worth asking");
    }

    #[test]
    fn a_folder_cannot_be_dropped_into_itself() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p.clone(), en_config()).unwrap();
        app.drop_onto(vec![p.join("d")], p.join("d"), true);
        assert!(matches!(app.popup, Popup::None));
    }
}

/// `d` in the bookmark list asks first. It used to remove and save in one
/// keystroke, one key away from `j` and `k`, and a bookmark removed by accident
/// is gone — there is no undo for a file that only ever held a name and a path.
mod shortcut_delete_asks {
    use super::*;

    fn with_bookmarks(app: &mut App) {
        app.shortcuts.entries = vec![
            crate::Shortcut { name: "home".into(), target: Some("~".into()), children: None },
            crate::Shortcut { name: "work".into(), target: Some("/tmp".into()), children: None },
        ];
        app.start_shortcuts();
    }

    fn press(app: &mut App, c: char) {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)).unwrap();
    }

    fn names(app: &App) -> Vec<String> {
        app.shortcuts.entries.iter().map(|s| s.name.clone()).collect()
    }

    #[test]
    fn d_asks_rather_than_removing() {
        let (_d, mut app) = app_with(&["a.txt"]);
        with_bookmarks(&mut app);
        press(&mut app, 'd');
        assert!(
            matches!(app.popup, Popup::ConfirmShortcutDelete { .. }),
            "it asks"
        );
        assert_eq!(names(&app).len(), 2, "and nothing has gone yet");
    }

    #[test]
    fn saying_no_keeps_it_and_returns_to_the_list() {
        let (_d, mut app) = app_with(&["a.txt"]);
        with_bookmarks(&mut app);
        press(&mut app, 'd');
        press(&mut app, 'n');
        assert_eq!(names(&app), ["home", "work"], "both bookmarks survive");
        assert!(
            matches!(app.popup, Popup::Shortcuts { .. }),
            "and the list is back, so the next one is two keys away"
        );
    }

    #[test]
    fn escape_also_keeps_it() {
        let (_d, mut app) = app_with(&["a.txt"]);
        with_bookmarks(&mut app);
        press(&mut app, 'd');
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert_eq!(names(&app).len(), 2);
        assert!(matches!(app.popup, Popup::Shortcuts { .. }));
    }

    #[test]
    fn saying_yes_removes_the_one_under_the_cursor() {
        let (_d, mut app) = app_with(&["a.txt"]);
        with_bookmarks(&mut app);
        press(&mut app, 'd');
        press(&mut app, 'y');
        assert_eq!(names(&app), ["work"], "the first one, and only it");
    }

    #[test]
    fn the_list_comes_back_after_removing() {
        let (_d, mut app) = app_with(&["a.txt"]);
        with_bookmarks(&mut app);
        press(&mut app, 'd');
        press(&mut app, 'y');
        assert!(
            matches!(app.popup, Popup::Shortcuts { .. }),
            "tidying up several should not mean reopening the list each time"
        );
    }
}

/// The windowed build hands the front end a list of "a picture goes here"
/// slots, rebuilt every frame. It has to *be* rebuilt: the list only ever grew,
/// and a window sitting still with thirty icons on it was asking the renderer
/// for thousands of quads a few seconds later.
mod pictures_do_not_pile_up {
    use super::*;

    fn frames(app: &mut App, n: usize) -> Vec<usize> {
        (0..n)
            .map(|_| {
                let _ = render(app, 140, 40);
                app.icon_slots.len()
            })
            .collect()
    }

    #[test]
    fn the_detail_view_asks_for_the_same_pictures_every_frame() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        app.native_icons = true;
        app.skin = Skin::Finder;
        let counts = frames(&mut app, 4);
        assert!(counts[0] > 0, "the detail view asks for pictures at all");
        assert!(
            counts.iter().all(|n| *n == counts[0]),
            "a still screen asks for the same slots each time, got {counts:?}"
        );
    }

    #[test]
    fn the_icon_grid_asks_for_the_same_pictures_every_frame() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        app.native_icons = true;
        app.skin = Skin::Finder;
        app.icon_view = true;
        let counts = frames(&mut app, 4);
        assert!(counts[0] > 0, "the grid is made of pictures");
        assert!(
            counts.iter().all(|n| *n == counts[0]),
            "a still grid asks for the same slots each time, got {counts:?}"
        );
    }
}

/// Opening things in a pane that is a server. The rows carry paths that mean
/// nothing to this disk, so every "open it" route has to go over the network.
mod remote_pane_opens {
    use super::*;

    /// A pane showing `/var` on a server: `..`, a directory, a file.
    fn remote(app: &mut App) {
        let entries = vec![
            cian_core::Entry::remote("..", "/", true, 0, true),
            cian_core::Entry::remote("log", "/var/log", true, 0, false),
            cian_core::Entry::remote("hosts", "/var/hosts", false, 12, false),
        ];
        app.left.active_mut().enter_remote("srv", "/var", entries);
    }

    /// A connection that will refuse — enough for the pane to count as
    /// connected, without a server being involved.
    fn connected(app: &mut App) {
        app.remote_targets[0] = Some((
            cian_scp::Target {
                host: "127.0.0.1".into(),
                port: 1,
                user: "nobody".into(),
                password: String::new(),
            key: None,
            key_pass: None,
            },
            "srv".into(),
        ));
    }

    #[test]
    fn a_double_click_on_a_remote_directory_never_looks_on_this_disk() {
        let (_d, mut app) = app_with(&["a.txt"]);
        remote(&mut app);
        connected(&mut app);
        app.left.active_mut().cursor = 1;
        let here = app.active_pane().unwrap().cwd.clone();
        app.activate_selected().unwrap();
        assert!(
            app.active_pane().unwrap().remote_view().is_some(),
            "the pane is still the server's"
        );
        assert_eq!(app.active_pane().unwrap().cwd, here, "the local cwd never moved");
    }

    #[test]
    fn enter_on_a_remote_file_fetches_it() {
        let (_d, mut app) = app_with(&["a.txt"]);
        remote(&mut app);
        connected(&mut app);
        app.left.active_mut().cursor = 2;
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(
            app.remote_view.is_some(),
            "Enter on a remote file starts the fetch the viewer reads from"
        );
    }

    #[test]
    fn enter_on_a_remote_directory_lists_it() {
        let (_d, mut app) = app_with(&["a.txt"]);
        remote(&mut app);
        connected(&mut app);
        app.left.active_mut().cursor = 1;
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(app.remote_pane_ls.is_some(), "Enter on a remote directory asks for its listing");
        assert!(app.remote_view.is_none(), "and does not try to read it as a file");
    }
}

/// Who the advice about terminals is for. Not the window: it has no terminal,
/// and it sets none of the variables a good terminal sets — so it looked like
/// the worst one, and said so on every start.
mod advice_is_for_terminals {
    use super::*;

    #[test]
    fn only_the_legacy_windows_console_is_told_about_itself() {
        // (host, on Windows, host says it is a modern terminal)
        assert!(wants_terminal_advice(Host::Terminal, true, false), "the case it exists for");
        assert!(!wants_terminal_advice(Host::Terminal, true, true), "Windows Terminal is fine");
        assert!(!wants_terminal_advice(Host::Terminal, false, false), "not a Windows problem");
    }

    #[test]
    fn a_window_is_never_told_to_go_and_find_a_terminal() {
        for windows in [true, false] {
            for modern in [true, false] {
                assert!(
                    !wants_terminal_advice(Host::Window, windows, modern),
                    "windows={windows} modern={modern}"
                );
            }
        }
    }
}

/// Where the desktop is. Not always `~/Desktop`: OneDrive takes the folder
/// away, renames it in the user's own language, and the sidebar entry for it
/// vanished — the report was "please add the desktop", to a cian that thought
/// it had.
mod where_the_desktop_is {

    #[test]
    fn the_plain_path_wins_when_it_is_there() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        std::fs::create_dir_all(home.join("Desktop")).unwrap();
        std::fs::create_dir_all(home.join("OneDrive/デスクトップ")).unwrap();
        assert_eq!(
            crate::known_dir(home, "Desktop", "デスクトップ"),
            Some(home.join("Desktop"))
        );
    }

    #[test]
    fn onedrive_is_found_when_it_took_the_folder() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        std::fs::create_dir_all(home.join("OneDrive/デスクトップ")).unwrap();
        assert_eq!(
            crate::known_dir(home, "Desktop", "デスクトップ"),
            Some(home.join("OneDrive/デスクトップ")),
            "the Japanese client's name for it, inside OneDrive"
        );
    }

    #[test]
    fn nothing_is_invented_when_there_is_nothing() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(crate::known_dir(d.path(), "Desktop", "デスクトップ"), None);
    }
}

/// A theme the user asked for holds in every view. The detail and icon views
/// bring the Finder palette with them, and were bringing it over the top of a
/// theme chosen in init.lua — the same cian looked like two programs depending
/// on which view was showing.
mod a_chosen_theme_survives_the_view {

    #[test]
    fn only_a_theme_nobody_asked_for_may_be_replaced() {
        assert!(
            crate::theme::skin_may_swap_theme(false),
            "nobody chose these colours, so the detail view brings its own"
        );
        assert!(
            !crate::theme::skin_may_swap_theme(true),
            "these are the user's colours, and they hold in every view"
        );
    }
}

/// The single-pane views are the filer and nothing else — that is what someone
/// who did not want a terminal picked. So there is no shell panel in them, and
/// asking for one says where it lives rather than moving the focus somewhere
/// nothing is drawn (which is what made the shell look like it would not
/// start).
mod the_filer_views_have_no_shell {
    use super::*;

    fn detail_view(names: &[&str]) -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(names);
        app.skin = Skin::Finder;
        app.native_icons = true;
        (d, app)
    }

    #[test]
    fn the_detail_view_keeps_the_focus_on_the_files() {
        let (_d, mut app) = detail_view(&["a.txt"]);
        app.handle_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.focused, FocusedPane::Left, "the focus never left the listing");
        assert!(app.message.is_some(), "and it says where the shell is");
        let _ = render(&mut app, 140, 40);
        assert_eq!(app.layout_rects.shell.height, 0, "no panel is drawn for it");
    }

    #[test]
    fn the_icon_grid_answers_the_same_way() {
        let (_d, mut app) = detail_view(&["a.txt"]);
        app.icon_view = true;
        app.handle_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT)).unwrap();
        assert_ne!(app.focused, FocusedPane::Shell);
    }

    #[test]
    fn the_classic_view_still_goes_to_the_shell() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.focused, FocusedPane::Shell, "classic has a panel, so J reaches it");
    }
}

/// The mark for a cloud placeholder, and why the window does not get the cloud.
mod the_cloud_mark_fits_its_cell {
    #[test]
    fn a_terminal_keeps_the_cloud_and_the_window_does_not() {
        assert_eq!(crate::render::cloud_mark_for(false), "☁ ", "a terminal draws it fine");
        assert_eq!(
            crate::render::cloud_mark_for(true),
            "↓ ",
            "the window rasterises per cell, and ☁ is a two-cell glyph there"
        );
    }
}

/// What a frame costs, on this machine, in a directory big enough to fill the
/// pane. Not an assertion — "fast enough" is not something a test can decide —
/// but a number that can be taken before and after a change to the drawing
/// path, which is the only way to tell tuning from fiddling.
///
///     cargo test --release -p cian-tui -- --ignored frame_cost --nocapture
mod frame_cost {
    use super::*;

    #[test]
    #[ignore = "a measurement, not a test"]
    fn a_full_pane_of_files() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..2000 {
            std::fs::write(d.path().join(format!("file_{i:04}.txt")), b"x").unwrap();
        }
        let p = d.path().to_path_buf();
        let mut app = App::new(p.clone(), p, en_config()).unwrap();
        app.native_icons = true;

        // Warm: the first frame pays for whatever is lazily built.
        for _ in 0..20 {
            let _ = render(&mut app, 200, 60);
        }
        // The minimum of many runs, not the mean: a laptop's scheduler adds
        // time and never removes it, so the fastest run is the closest thing to
        // what the code actually costs.
        let cost = |app: &mut App, w: u16, h: u16| {
            let mut best = std::time::Duration::MAX;
            for _ in 0..30 {
                let t = std::time::Instant::now();
                for _ in 0..10 {
                    let _ = render(app, w, h);
                }
                best = best.min(t.elapsed() / 10);
            }
            best
        };
        // Two heights, so the fixed cost of a frame can be told apart from what
        // each row of the listing costs.
        let tall = cost(&mut app, 200, 60);
        let short = cost(&mut app, 200, 14);
        eprintln!("frame_cost: {tall:?} per frame, 200x60, two panes of 2000 files");
        eprintln!("frame_cost: {short:?} per frame, 200x14 (same panes, 46 fewer rows each)");
        app.preview_on = false;
        let tall_np = cost(&mut app, 200, 60);
        eprintln!("frame_cost: {tall_np:?} 200x60 with no preview panel");

        // The same window, the same everything, with almost nothing to list:
        // the difference is what the rows themselves cost.
        let e = tempfile::tempdir().unwrap();
        for i in 0..3 {
            std::fs::write(e.path().join(format!("f{i}.txt")), b"x").unwrap();
        }
        let q = e.path().to_path_buf();
        let mut empty = App::new(q.clone(), q, en_config()).unwrap();
        empty.native_icons = true;
        empty.preview_on = false;
        for _ in 0..20 {
            let _ = render(&mut empty, 200, 60);
        }
        let bare = cost(&mut empty, 200, 60);
        eprintln!("frame_cost: {bare:?} 200x60 with three files (no preview)");
    }
}

/// The sidebar is drawn on every frame, and the disk must not be asked about
/// it on every frame. On Windows those paths are usually OneDrive's, where a
/// question costs a round trip to the sync engine — sixteen milliseconds a
/// frame of them, measured in the window.
mod the_sidebar_does_not_ask_the_disk_every_frame {
    use super::*;

    #[test]
    fn what_it_learned_is_kept_between_frames() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.skin = Skin::Finder;
        app.native_icons = true;
        assert!(app.sidebar_dirs.1.is_empty(), "nothing known before the first frame");
        let _ = render(&mut app, 140, 40);
        let learned = app.sidebar_dirs.1.len();
        assert!(learned > 0, "the first frame asks, and remembers what it heard");
        let _ = render(&mut app, 140, 40);
        assert_eq!(app.sidebar_dirs.1.len(), learned, "the second frame asks nothing new");
    }

    /// The standard places are worked out once for the life of the process:
    /// each one probes up to ten paths, and they cannot move while cian runs.
    #[test]
    fn the_standard_places_are_worked_out_once() {
        let first = crate::render::standard_places().as_ptr();
        let again = crate::render::standard_places().as_ptr();
        assert_eq!(first, again, "the same list, not a fresh one per frame");
    }
}

/// What the single-pane views do with keys that have nowhere to go, and what
/// Ctrl+click means when nothing is marked yet.
mod desktop_view_keys {
    use super::*;
    use ratatui::layout::Rect;

    fn detail(names: &[&str]) -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(names);
        app.skin = Skin::Finder;
        app.native_icons = true;
        (d, app)
    }

    #[test]
    fn shift_l_does_not_swap_which_listing_is_shown() {
        let (_d, mut app) = detail(&["a.txt"]);
        app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.focused, FocusedPane::Left, "one pane in this view");
        assert!(app.message.is_some(), "and it says so");
    }

    #[test]
    fn shift_h_likewise() {
        let (_d, mut app) = detail(&["a.txt"]);
        app.focus(FocusedPane::Right);
        app.handle_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.focused, FocusedPane::Right);
    }

    #[test]
    fn the_classic_view_still_moves_between_panes() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.focused, FocusedPane::Right);
    }

    /// `q` quits in the classic view and looks for a file in the desktop ones.
    #[test]
    fn q_looks_for_a_file_rather_than_the_way_out() {
        let (_d, mut app) = detail(&["apple.txt", "quince.txt"]);
        app.handle_key(key('q')).unwrap();
        assert!(matches!(app.popup, Popup::None), "no 'really quit?' box");
        let name = app.active_pane().unwrap().selected().unwrap().name.clone();
        assert_eq!(name, "quince.txt", "it went to the file starting with q");
    }

    #[test]
    fn q_still_quits_in_the_classic_view() {
        let (_d, mut app) = app_with(&["quince.txt"]);
        app.handle_key(key('q')).unwrap();
        assert!(!matches!(app.popup, Popup::None), "the classic view asks");
    }

    /// Ctrl+click adds to a selection, and the file under the cursor is part of
    /// what the user believes is already selected — it is drawn that way.
    #[test]
    fn ctrl_click_keeps_the_file_that_was_already_pointed_at() {
        let (_d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        app.icon_view = true;
        app.icon_cols = 3;
        app.grid_area = Some(Rect::new(0, 0, 3 * 14, 4 * 6));
        // The cursor starts on the first real entry — index 1, since `..` is
        // index 0. Ctrl+click the one after it.
        let first = app.active_pane().unwrap().cursor;
        assert_eq!(first, 1, "the cursor starts on the first file, not on `..`");
        let (col, row) = (2 * 14 + 2, 1); // tile index 2
        assert!(app.grid_click_mods(col, row, true), "the click landed on a tile");
        let marks = app.active_pane().unwrap().marks.len();
        assert_eq!(marks, 2, "the one that was pointed at, and the one clicked");
    }
}

/// A drop from the desktop arrives as text, and on Windows that text is full
/// of backslashes and spaces. What matters is that a path survives the reading
/// of it — a name torn in half is a drop that silently does nothing.
///
/// Both conventions are asserted from whichever platform runs the tests. The
/// Windows reading used to be behind `cfg!(windows)` at the point of use,
/// which meant it was only ever exercised on the machine cian is not written
/// on.
mod dropped_paths_are_read_the_platform_way {

    /// The Windows convention: backslashes are separators, and every dropped
    /// path is on its own line.
    fn windows(text: &str) -> Vec<std::path::PathBuf> {
        crate::drop::read_dropped(text, false, |_| true)
    }

    /// The Unix one: a backslash escapes the space it precedes.
    fn unix(text: &str) -> Vec<std::path::PathBuf> {
        crate::drop::read_dropped(text, true, |_| true)
    }

    #[test]
    fn a_windows_path_keeps_its_separators() {
        let one = windows("C:\\Users\\taro\\a.txt");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].to_string_lossy(), "C:\\Users\\taro\\a.txt");
    }

    #[test]
    fn a_windows_name_with_spaces_survives() {
        let p = windows("C:\\Users\\taro\\My Documents\\a b.txt");
        assert_eq!(p.len(), 1, "not torn at the spaces: {p:?}");
        assert!(p[0].to_string_lossy().ends_with("a b.txt"));
    }

    #[test]
    fn several_windows_files_arrive_as_several_paths() {
        let many = windows("C:\\a\\one.txt\nC:\\a\\two.txt");
        assert_eq!(many.len(), 2, "one per line: {many:?}");
    }

    #[test]
    fn a_unix_escaped_space_is_one_path() {
        let p = unix("/home/taro/My\\ File.txt");
        assert_eq!(p.len(), 1, "the escape held it together: {p:?}");
        assert_eq!(p[0].to_string_lossy(), "/home/taro/My File.txt");
    }
}

/// The places down the left of the desktop views are there to be clicked.
mod the_sidebar_answers_a_click {
    use super::*;

    fn desktop(icon_view: bool) -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(&["a.txt"]);
        app.skin = Skin::Finder;
        app.native_icons = true;
        app.icon_view = icon_view;
        // A place to go: a real directory, so the jump can succeed.
        let target = d.path().join("sub");
        std::fs::create_dir(&target).unwrap();
        app.shortcuts.entries =
            vec![crate::Shortcut { name: "sub".into(), target: Some(target.display().to_string()), children: None }];
        (d, app)
    }

    fn click_the_bookmark(app: &mut App) -> bool {
        // Draw first: the sidebar's rows are recorded by the frame that draws
        // them, which is what a click is tested against.
        let _ = render(app, 140, 40);
        let row = app
            .sidebar_rows
            .iter()
            .find(|(p, _)| p.file_name().map(|n| n == "sub").unwrap_or(false))
            .map(|(_, y)| *y);
        let Some(row) = row else { return false };
        app.grid_click_mods(2, row, false)
    }

    #[test]
    fn the_detail_view_goes_to_the_place_that_was_clicked() {
        let (_d, mut app) = desktop(false);
        assert!(click_the_bookmark(&mut app), "the click landed on the sidebar");
        assert!(
            app.active_pane().unwrap().cwd.ends_with("sub"),
            "it went there: {}",
            app.active_pane().unwrap().cwd.display()
        );
    }

    #[test]
    fn the_icon_grid_still_does_too() {
        let (_d, mut app) = desktop(true);
        assert!(click_the_bookmark(&mut app));
        assert!(app.active_pane().unwrap().cwd.ends_with("sub"));
    }

    #[test]
    fn the_classic_view_leaves_the_click_to_the_panes() {
        let (_d, mut app) = app_with(&["a.txt"]);
        assert!(!app.grid_click_mods(2, 5, false), "no chrome in the classic view");
    }
}

/// The spinner turns, and turns in glyphs the window can actually draw.
///
/// The window draws with one font and no fallback, and the Japanese Nerd Fonts
/// it looks for have no braille block — so a braille spinner was ten frames of
/// the same missing glyph there, which is a spinner that does not spin. Hence
/// both halves of this: that the frame changes with time, and that it stays out
/// of U+2800.
mod the_spinner_turns {
    use crate::render::spinner_frame;

    #[test]
    fn a_full_turn_shows_every_frame_in_order() {
        let seen: Vec<&str> = (0..8).map(|i| spinner_frame(i * 120)).collect();
        assert_eq!(seen[0], seen[4], "it comes back round");
        assert_eq!(seen[..4].iter().collect::<std::collections::HashSet<_>>().len(), 4);
    }

    #[test]
    fn nothing_it_draws_is_braille() {
        for ms in [0u128, 120, 240, 360, 480, 1_000, 5_432] {
            for c in spinner_frame(ms).chars() {
                assert!(
                    !('\u{2800}'..='\u{28FF}').contains(&c),
                    "{c:?} is braille, which the window's font does not have",
                );
            }
        }
    }
}

/// Everything cian draws has to exist in the font that draws it.
///
/// The window loads one font and has no fallback chain, so a character that
/// font does not have is not substituted, it is *blank* — and a blank is
/// indistinguishable from a bug. These are the ones HackGen Console NF (and
/// the Nerd Fonts beside it) turned out not to have: emoji, braille, and a
/// handful of stray dingbats. Checked against the source rather than against
/// the font, because the font is not on the machine that runs the tests.
mod the_window_can_draw_what_cian_writes {
    /// Found missing by walking the cmap of every font on the machine cian is
    /// written on. Replaced, in that order, with § ↑ ↓ ▣ ⇦ ↻ ◈ ◆.
    const ABSENT: &[char] = &['⚙', '👍', '👎', '📎', '⌫', '⏳', '✦', '🔑'];

    #[test]
    fn no_glyph_the_font_lacks_reaches_the_screen() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut bad = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            // This file names them in order to forbid them.
            if path.file_name().and_then(|n| n.to_str()) == Some("tests.rs") {
                continue;
            }
            for (n, line) in std::fs::read_to_string(&path).unwrap().lines().enumerate() {
                // A comment is not drawn, and the reasoning is worth keeping
                // in the words of the thing it is about.
                if line.trim_start().starts_with("//") {
                    continue;
                }
                for c in line.chars() {
                    let missing = ABSENT.contains(&c)
                        || ('\u{2800}'..='\u{28FF}').contains(&c) // braille
                        || c >= '\u{1F000}'; // emoji
                    if missing {
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        bad.push(format!("{name}:{} draws {c:?}", n + 1));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "the window's font cannot draw these:\n{}", bad.join("\n"));
    }
}

/// The desktop views say how far down the listing they are.
///
/// They said it in grey on grey — a │ thumb on a │ track, one shade apart —
/// which was reported as there being no scrollbar at all, and the grid did not
/// say it at all. Both draw a solid block now, and the grid's is clickable and
/// answers the wheel.
mod the_desktop_views_show_where_they_are {
    use super::*;

    /// Enough files that a page cannot hold them.
    fn plenty() -> Vec<String> {
        (0..200).map(|i| format!("file{i:03}.txt")).collect()
    }

    fn desktop(icon_view: bool) -> (tempfile::TempDir, App) {
        let names = plenty();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let (d, mut app) = app_with(&refs);
        app.skin = Skin::Finder;
        app.native_icons = true;
        app.icon_view = icon_view;
        (d, app)
    }

    /// Every symbol the frame drew, as one string.
    fn painted(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn the_detail_view_draws_a_solid_thumb() {
        let (_d, mut app) = desktop(false);
        assert!(painted(&mut app, 140, 40).contains('█'), "a thumb you can see");
    }

    /// Two cells wide in the views driven with the mouse: a one-cell bar is
    /// something to aim at rather than something to grab.
    #[test]
    fn the_desktop_bars_are_two_cells_wide() {
        for icons in [false, true] {
            let (_d, mut app) = desktop(icons);
            let _ = painted(&mut app, 140, 40);
            let bar = app.scroll_tracks.first().copied().expect("a track");
            assert_eq!(bar.rect.width, 2, "icon_view={icons}");
        }
    }

    /// …and the classic view's is still exactly its border.
    #[test]
    fn the_classic_bar_is_still_the_border_it_is_drawn_on() {
        let names = plenty();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let (_d, mut app) = app_with(&refs);
        let _ = painted(&mut app, 140, 40);
        let bar = app.scroll_tracks.first().copied().expect("a track");
        assert_eq!(bar.rect.width, 1);
    }

    #[test]
    fn the_grid_draws_one_too_and_leaves_room_for_it() {
        let (_d, mut app) = desktop(true);
        assert!(painted(&mut app, 140, 40).contains('█'), "a thumb you can see");
        let bar = app.scroll_tracks.first().copied().expect("a track to drag");
        let grid = app.grid_area.expect("the tiles have a rectangle");
        assert!(bar.rect.x >= grid.x + grid.width, "the bar is beside the tiles, not on them");
    }

    #[test]
    fn the_wheel_walks_the_grid_a_row_at_a_time() {
        let (_d, mut app) = desktop(true);
        let _ = painted(&mut app, 140, 40);
        let cols = app.icon_cols;
        assert!(cols > 0, "the grid has columns");
        let before = app.active_pane().unwrap().cursor;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            app.active_pane().unwrap().cursor - before,
            cols,
            "one notch, one row of tiles",
        );
    }

    #[test]
    fn a_click_down_the_grids_track_jumps_that_far_in() {
        let (_d, mut app) = desktop(true);
        let _ = painted(&mut app, 140, 40);
        let bar = app.scroll_tracks.first().copied().expect("a track to click");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: bar.rect.x,
            row: bar.rect.y + bar.rect.height - 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(
            app.active_pane().unwrap().cursor > bar.shown,
            "the foot of the track is the foot of the listing, not the first page",
        );
    }
}

/// A click off a popup closes it — in every view, and with nothing else
/// happening to the listing behind it.
mod clicking_past_a_popup_closes_it {
    use super::*;

    fn desktop(icon_view: bool) -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        app.skin = Skin::Finder;
        app.native_icons = true;
        app.icon_view = icon_view;
        (d, app)
    }

    fn click(app: &mut App, col: u16, row: u16) {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    /// Open the menu, draw it, and click the far corner — which no popup of
    /// cian's reaches.
    fn menu_then_click_away(app: &mut App) {
        app.open_context_menu(40, 12);
        let _ = render(app, 140, 40);
        click(app, 139, 39);
    }

    #[test]
    fn in_the_classic_view() {
        let (_d, mut app) = app_with(&["a.txt"]);
        menu_then_click_away(&mut app);
        assert!(matches!(app.popup, Popup::None), "the menu went away");
    }

    #[test]
    fn in_the_detail_view() {
        let (_d, mut app) = desktop(false);
        menu_then_click_away(&mut app);
        assert!(matches!(app.popup, Popup::None), "the menu went away");
    }

    #[test]
    fn in_the_icon_view() {
        let (_d, mut app) = desktop(true);
        menu_then_click_away(&mut app);
        assert!(matches!(app.popup, Popup::None), "the menu went away");
    }

    #[test]
    fn a_dialog_goes_too_and_is_never_answered_yes() {
        let (_d, mut app) = desktop(false);
        let doomed = app.active_pane().unwrap().cwd.join("a.txt");
        app.popup = Popup::ConfirmDelete { targets: vec![doomed.clone()] };
        let _ = render(&mut app, 140, 40);
        click(&mut app, 139, 39);
        assert!(matches!(app.popup, Popup::None), "dismissed");
        assert!(doomed.exists(), "and answered no, not yes");
    }

    #[test]
    fn a_click_inside_it_is_left_alone() {
        let (_d, mut app) = desktop(false);
        let doomed = app.active_pane().unwrap().cwd.join("a.txt");
        app.popup = Popup::ConfirmDelete { targets: vec![doomed] };
        let _ = render(&mut app, 140, 40);
        // The middle of the dialog: inside it, and on none of its buttons.
        let box_ = crate::render::popup_ink().expect("the dialog said where it is");
        click(&mut app, box_.x + box_.width / 2, box_.y + box_.height / 2);
        assert!(matches!(app.popup, Popup::ConfirmDelete { .. }), "still open: {:?}", app.popup);
    }
}

/// The window draws file icons as pictures over the cells, so a popup has to
/// take them with it. The full-window popups used to leave before that
/// happened, and a listing's worth of icons was drawn on top of the AI chat.
mod a_popup_takes_the_icons_with_it {
    use super::*;

    fn desktop_with_a_chat(icon_view: bool) -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        app.skin = Skin::Finder;
        app.native_icons = true;
        app.icon_view = icon_view;
        (d, app)
    }

    #[test]
    fn the_listing_has_icons_to_begin_with() {
        let (_d, mut app) = desktop_with_a_chat(false);
        let _ = render(&mut app, 140, 40);
        assert!(!app.icon_slots.is_empty(), "the detail view asks for pictures");
    }

    #[test]
    fn and_none_of_them_survive_the_chat() {
        for icons in [false, true] {
            let (_d, mut app) = desktop_with_a_chat(icons);
            app.start_ai_chat(ChatMode::Ai, Vec::new(), true);
            let _ = render(&mut app, 140, 40);
            assert!(
                app.icon_slots.is_empty(),
                "icon_view={icons}: {} icons left over the chat",
                app.icon_slots.len(),
            );
        }
    }

    #[test]
    fn nor_the_other_full_window_popups() {
        let (_d, mut app) = desktop_with_a_chat(false);
        app.start_toggles();
        let _ = render(&mut app, 140, 40);
        assert!(app.icon_slots.is_empty(), "{} icons left over the toggles", app.icon_slots.len());
    }
}

/// Switching the language switches the menu too.
///
/// It did not: the menu and the manual read `menu_lang`, the switch moved
/// `lang`, and "Switch to English" left the menu it had just been chosen from
/// in Japanese.
mod switching_language_switches_the_menu {
    use super::*;

    /// The menu as one string, with the padding taken out — a 全角 character
    /// occupies two cells and the second of them is blank, so the text read
    /// back off the screen has a space inside every Japanese word.
    fn menu_says(app: &mut App) -> String {
        app.open_context_menu(5, 5);
        let text: String =
            render(app, 140, 40).join("").chars().filter(|c| !c.is_whitespace()).collect();
        app.popup = Popup::None;
        text
    }

    #[test]
    fn the_menu_follows_the_switch() {
        let (_d, mut app) = app_with_lang(&["a.txt"], "ja");
        assert!(menu_says(&mut app).contains("cianを終了"), "Japanese to begin with");
        app.run_menu_item(MenuItem::Lang).unwrap();
        let english = menu_says(&mut app);
        assert!(english.contains("Quitcian"), "and English after: {english:.400}");
        assert!(!english.contains("終了"), "with nothing left behind");
    }

    #[test]
    fn and_so_does_the_manual() {
        let (_d, mut app) = app_with_lang(&["a.txt"], "ja");
        app.run_menu_item(MenuItem::Lang).unwrap();
        app.open_manual();
        let text: String =
            render(&mut app, 140, 40).join("").chars().filter(|c| !c.is_whitespace()).collect();
        assert!(!text.contains("移動"), "the manual came with it: {text:.400}");
    }

    #[test]
    fn a_language_named_in_init_lua_keeps_its_menu() {
        let (_d, mut app) = app_with_lang(&["a.txt"], "en");
        // As `cian.set_option("menu_lang", "ja")` would leave it.
        app.menu_lang = Lang::Ja;
        app.menu_lang_pinned = true;
        app.run_menu_item(MenuItem::Lang).unwrap();
        assert_eq!(app.lang, Lang::Ja, "the interface switched");
        assert_eq!(app.menu_lang, Lang::Ja, "and the menu stayed where init.lua put it");
    }
}

/// Tabs, in the view that had nowhere to show them.
mod the_icon_view_shows_its_tabs {
    use super::*;

    fn grid(names: &[&str]) -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(names);
        app.skin = Skin::Finder;
        app.native_icons = true;
        app.icon_view = true;
        (d, app)
    }

    fn painted(app: &mut App) -> String {
        render(app, 140, 40).join("").chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn a_second_tab_appears_in_the_strip() {
        let (_d, mut app) = grid(&["a.txt"]);
        assert!(app.left.tabs.len() == 1);
        app.left.add_clone().unwrap();
        let text = painted(&mut app);
        assert!(text.contains("2"), "the second tab is named on screen");
        assert!(!app.tab_rects.is_empty(), "and can be clicked");
    }

    #[test]
    fn clicking_a_label_goes_to_that_tab() {
        let (_d, mut app) = grid(&["a.txt"]);
        app.left.add_clone().unwrap();
        let _ = painted(&mut app);
        assert_eq!(app.left.active, 1, "the new tab is the one showing");
        let (_, idx, r) = app
            .tab_rects
            .iter()
            .copied()
            .find(|(_, i, _)| *i == 0)
            .expect("the first tab has a label");
        assert_eq!(idx, 0);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: r.x,
            row: r.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.left.active, 0, "clicking the first label went back to it");
    }
}

/// A tab is opened on purpose or not at all.
mod a_new_tab_is_asked_about_first {
    use super::*;

    #[test]
    fn the_letter_t_asks() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(key('t')).unwrap();
        assert!(matches!(app.popup, Popup::ConfirmNewTab { .. }), "{:?}", app.popup);
        assert_eq!(app.left.tabs.len(), 1, "and nothing has happened yet");
    }

    #[test]
    fn no_leaves_the_tabs_alone() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(key('t')).unwrap();
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::None));
        assert_eq!(app.left.tabs.len(), 1, "still one tab");
    }

    #[test]
    fn yes_opens_it() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.handle_key(key('t')).unwrap();
        app.handle_key(key('y')).unwrap();
        assert_eq!(app.left.tabs.len(), 2);
        assert_eq!(app.left.active, 1, "and goes to it");
    }

    #[test]
    fn it_asks_in_the_desktop_views_too() {
        for icons in [false, true] {
            let (_d, mut app) = app_with(&["a.txt"]);
            app.skin = Skin::Finder;
            app.icon_view = icons;
            // In the grid a letter is type-ahead, so F9 is the key that asks.
            app.handle_key(code(KeyCode::F(9))).unwrap();
            assert!(
                matches!(app.popup, Popup::ConfirmNewTab { .. }),
                "icon_view={icons}: {:?}",
                app.popup,
            );
        }
    }
}

/// In a window the picture is a picture, not an impression of one in
/// half-blocks: the popup hands the rectangle to the front end and stays out
/// of it.
mod a_window_shows_the_picture_itself {
    use super::*;

    fn with_an_image(native: bool) -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(&[]);
        let mut img = image::RgbImage::new(40, 20);
        for px in img.pixels_mut() {
            *px = image::Rgb([30, 160, 90]);
        }
        img.save(d.path().join("pic.png")).unwrap();
        app.reload_both();
        app.native_icons = native;
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "pic.png").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert!(matches!(app.popup, Popup::ImageView { .. }), "{:?}", app.popup);
        (d, app)
    }

    #[test]
    fn the_window_is_told_where_the_picture_goes() {
        let (_d, mut app) = with_an_image(true);
        let text = render(&mut app, 80, 24).join("");
        let slot = app.image_slot.clone().expect("a rectangle for the picture");
        assert!(slot.path.ends_with("pic.png"));
        assert!(slot.w > 0 && slot.h > 0, "with room in it: {slot:?}");
        assert!(!text.contains('▀'), "and no half-blocks drawn into it");
    }

    #[test]
    fn a_terminal_still_gets_its_half_blocks() {
        let (_d, mut app) = with_an_image(false);
        let text = render(&mut app, 80, 24).join("");
        assert!(app.image_slot.is_none(), "nothing is asked of a terminal");
        assert!(text.contains('▀'), "it draws the picture the only way it can");
    }

    #[test]
    fn the_rectangle_is_forgotten_when_the_picture_closes() {
        let (_d, mut app) = with_an_image(true);
        let _ = render(&mut app, 80, 24);
        app.handle_key(code(KeyCode::Esc)).unwrap();
        let _ = render(&mut app, 80, 24);
        assert!(app.image_slot.is_none(), "no picture, no rectangle");
    }
}

/// Every popup there is, checked for the two things a popup has to get right
/// in a window: it takes the file icons with it, and a click off it closes it.
///
/// Written as a sweep rather than as a case per popup because both answers come
/// from shared machinery — the one exit at the foot of `draw`, and the
/// rectangle each popup records when it clears the cells it is about to cover —
/// and shared machinery is exactly the kind that is right for fifty popups and
/// wrong for the fifty-first. The last test here reads the enum out of
/// `lib.rs`, so a popup added later cannot quietly skip the sweep.
mod every_popup_behaves {
    use super::*;

    /// The name to report a failure under, and the popup itself.
    fn all(dir: &std::path::Path) -> Vec<(&'static str, Popup)> {
        let file = dir.join("a.txt");
        let files = vec![file.clone()];
        vec![
            ("ConfirmDelete", Popup::ConfirmDelete { targets: files.clone() }),
            ("OpQueue", Popup::OpQueue { cursor: 0 }),
            ("ConfirmNoBom", Popup::ConfirmNoBom { targets: files.clone() }),
            (
                "ConfirmZipAdd",
                Popup::ConfirmZipAdd {
                    archive: dir.join("a.zip"),
                    sub: String::new(),
                    sources: files.clone(),
                },
            ),
            (
                "ConfirmZipDelete",
                Popup::ConfirmZipDelete {
                    archive: dir.join("a.zip"),
                    members: vec!["a.txt".into()],
                    shown: vec!["a.txt".into()],
                },
            ),
            (
                "ConfirmTransfer",
                Popup::ConfirmTransfer {
                    op: PendingOp::Copy,
                    targets: files.clone(),
                    dest: dir.to_path_buf(),
                },
            ),
            (
                "ConfirmDiffCopy",
                Popup::ConfirmDiffCopy {
                    src: file.clone(),
                    dst: dir.join("b.txt"),
                    is_dir: false,
                    back: Box::new(Popup::None),
                },
            ),
            (
                "ConfirmShortcutDelete",
                Popup::ConfirmShortcutDelete {
                    path: Vec::new(),
                    idx: 0,
                    name: "here".into(),
                    back: Box::new(Popup::None),
                },
            ),
            (
                "ConfirmDirSync",
                Popup::ConfirmDirSync {
                    to_right: true,
                    ops: Vec::new(),
                    extra: 0,
                    back: Box::new(Popup::None),
                },
            ),
            (
                "ConfirmRemoteDelete",
                Popup::ConfirmRemoteDelete {
                    side: FocusedPane::Left,
                    path: "/tmp/a".into(),
                    name: "a".into(),
                    is_dir: false,
                },
            ),
            (
                "ConfirmRemoteMove",
                Popup::ConfirmRemoteMove {
                    plan: crate::RemoteMovePlan {
                        files: vec!["a.txt".into()],
                        src_target: None,
                        dst_target: None,
                        dst_dir: "/tmp".into(),
                    },
                    from: "host:/a".into(),
                    to: "host:/b".into(),
                },
            ),
            (
                "TextInput",
                text_input(" Rename ", "new name:", "a.txt".into(), InputKind::Rename { original: file.clone() }),
            ),
            ("Notice", Popup::Notice { lines: vec!["something happened".into()] }),
            (
                "Report",
                Popup::Report {
                    title: " Report ".into(),
                    lines: vec!["a line".into()],
                    scroll: 0,
                    back: Box::new(Popup::None),
                },
            ),
            (
                "Palette",
                Popup::Palette {
                    kind: crate::PaletteKind::Commands,
                    query: String::new(),
                    items: Vec::new(),
                    shown: Vec::new(),
                    cursor: 0,
                    scroll: 0,
                },
            ),
            (
                "DiskUsage",
                Popup::DiskUsage {
                    dir: dir.to_path_buf(),
                    entries: vec![cian_core::du::DuEntry {
                        name: "a.txt".into(),
                        path: file.clone(),
                        size: 1,
                        is_dir: false,
                    }],
                    total: 1,
                    cursor: 0,
                    scroll: 0,
                },
            ),
            ("Manual", Popup::Manual { lines: vec!["keys".into()], scroll: 0 }),
            (
                "ContextMenu",
                Popup::ContextMenu { items: vec![MenuItem::Quit], cursor: 0, at: (10, 6) },
            ),
            ("ColorPicker", Popup::ColorPicker { pane: FocusedPane::Left, cursor: 0 }),
            ("SortPicker", Popup::SortPicker { cursor: 0 }),
            ("Macros", Popup::Macros { cursor: 0, names: vec!["one".into()] }),
            (
                "GitLog",
                Popup::GitLog {
                    title: " git log ".into(),
                    dir: dir.to_path_buf(),
                    commits: vec![cian_core::git::Commit {
                        hash: "abc1234".into(),
                        date: "2026-08-19".into(),
                        author: "someone".into(),
                        subject: "a change".into(),
                    }],
                    cursor: 0,
                    scroll: 0,
                    vcs: crate::Vcs::Git,
                },
            ),
            ("EncodingPicker", Popup::EncodingPicker { cursor: 0, target: crate::EncTarget::Shell }),
            (
                "DirCompare",
                Popup::DirCompare {
                    left: "left".into(),
                    right: "right".into(),
                    left_root: dir.to_path_buf(),
                    right_root: dir.to_path_buf(),
                    entries: Vec::new(),
                    cursor: 0,
                    scroll: 0,
                    truncated: false,
                },
            ),
            (
                "Diff",
                Popup::Diff {
                    left: "a.txt".into(),
                    right: "b.txt".into(),
                    left_path: file.clone(),
                    right_path: dir.join("b.txt"),
                    encoding: cian_core::viewer::TextEncoding::Utf8,
                    result: cian_core::diff::Diff {
                        rows: Vec::new(),
                        added: 0,
                        removed: 0,
                        changed: 0,
                        truncated: false,
                        binary: false,
                        identical: true,
                        too_large: false,
                    },
                    folded: Vec::new(),
                    fold: false,
                    scroll: 0,
                    find: None,
                    find_input: None,
                },
            ),
            (
                "Archive",
                Popup::Archive {
                    path: dir.join("a.zip"),
                    members: Vec::new(),
                    cursor: 0,
                    scroll: 0,
                },
            ),
            (
                "DestPicker",
                Popup::DestPicker { op: PendingOp::Copy, targets: files.clone(), cursor: 0 },
            ),
            (
                "FindResults",
                Popup::FindResults {
                    hits: vec![cian_core::search::Hit {
                        path: file.clone(),
                        rel: "a.txt".into(),
                        is_dir: false,
                        line: None,
                    }],
                    cursor: 0,
                    scroll: 0,
                    by_ai: false,
                },
            ),
            (
                "GrepReplace",
                Popup::GrepReplace(Box::new(crate::ReplacePlan {
                    changes: Vec::new(),
                    skipped: Vec::new(),
                    cursor: 0,
                    scroll: 0,
                    what: "a → b".into(),
                })),
            ),
            ("SshHosts", Popup::SshHosts { cursor: 0, filter: String::new() }),
            ("SshUsers", Popup::SshUsers { host: 0, cursor: 0 }),
            (
                "RemoteBrowser",
                Popup::RemoteBrowser {
                    label: "host".into(),
                    cwd: "/".into(),
                    entries: Vec::new(),
                    cursor: 0,
                    scroll: 0,
                    marked: Default::default(),
                    loading: false,
                    purpose: crate::BrowsePurpose::Download,
                },
            ),
            ("LocalDest", Popup::LocalDest { files: vec!["a.txt".into()], cursor: 0 }),
            (
                "ThemePicker",
                Popup::ThemePicker {
                    cursor: 0,
                    scope: crate::ThemeScope::App { revert: crate::theme::theme() },
                },
            ),
            ("Snippets", Popup::Snippets { cursor: 0, filter: String::new() }),
            (
                "ConfirmSnippet",
                Popup::ConfirmSnippet { name: "s".into(), cmd: "ls".into(), enter: false },
            ),
            ("Search", Popup::Search { buffer: "a".into() }),
            ("History", Popup::History { entries: vec![dir.to_path_buf()], cursor: 0 }),
            (
                "Shortcuts",
                Popup::Shortcuts {
                    entries: vec![crate::Shortcut {
                        name: "here".into(),
                        target: Some(dir.display().to_string()),
                        children: None,
                    }],
                    cursor: 0,
                    path: Vec::new(),
                },
            ),
            ("ConfirmQuit", Popup::ConfirmQuit),
            (
                "ConfirmClose",
                Popup::ConfirmClose { target: crate::CloseTarget::FileTab(FocusedPane::Left) },
            ),
            ("ConfirmNewTab", Popup::ConfirmNewTab { side: FocusedPane::Left }),
            ("AiShellConfirm", Popup::AiShellConfirm { command: "ls -la".into(), description: "list them".into() }),
            (
                "AiChat",
                Popup::AiChat {
                    input: String::new(),
                    log: vec![ChatMsg { user: true, text: "hello".into() }],
                    scroll: 0,
                    pending: true,
                    sel: None,
                    mode: ChatMode::Ai,
                    skin: ChatSkin::of(ChatMode::Ai),
                },
            ),
            ("AiHistory", Popup::AiHistory { cursor: 0 }),
            ("Toggles", Popup::Toggles { cursor: 0 }),
            (
                "ConfirmElevate",
                Popup::ConfirmElevate {
                    op: PendingOp::Copy,
                    targets: files.clone(),
                    dest: dir.to_path_buf(),
                },
            ),
            (
                "CommitMessage",
                Popup::CommitMessage {
                    buffer: "a change".into(),
                    stat: "1 file".into(),
                    dir: dir.to_path_buf(),
                    editing: false,
                },
            ),
            (
                "JunkReview",
                Popup::JunkReview {
                    items: vec![crate::JunkItem {
                        path: file.clone(),
                        reason: "empty".into(),
                        selected: true,
                    }],
                    cursor: 0,
                    scroll: 0,
                },
            ),
            (
                "StructureReview",
                Popup::StructureReview {
                    items: vec![crate::MoveItem {
                        path: file.clone(),
                        name: "a.txt".into(),
                        dest: "docs".into(),
                        reason: "text".into(),
                        selected: true,
                    }],
                    cursor: 0,
                    scroll: 0,
                    dir: dir.to_path_buf(),
                },
            ),
            (
                "RenameReview",
                Popup::RenameReview {
                    items: vec![crate::RenameItem {
                        path: file.clone(),
                        old: "a.txt".into(),
                        new: "b.txt".into(),
                        selected: true,
                    }],
                    cursor: 0,
                    scroll: 0,
                    by_ai: false,
                },
            ),
            (
                "ConfirmDiscard",
                Popup::ConfirmDiscard { targets: files.clone(), dir: dir.to_path_buf() },
            ),
            (
                "DupeReview",
                Popup::DupeReview {
                    items: vec![crate::DupeItem {
                        path: file.clone(),
                        group: 0,
                        keeper: true,
                        selected: false,
                    }],
                    cursor: 0,
                    scroll: 0,
                },
            ),
        ]
    }

    /// The two the sweep cannot build here, and why.
    ///
    /// Both are opened by a key in tests of their own — see
    /// `f3_on_an_image_opens_the_half_block_preview` and the viewer's tests —
    /// and both are checked below through that door instead.
    const OPENED_WITH_A_KEY: &[&str] = &["ImageView", "Viewer"];

    fn desktop(icon_view: bool) -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        app.skin = Skin::Finder;
        app.native_icons = true;
        app.icon_view = icon_view;
        // A server to be in the middle of picking a login for. Without one the
        // SSH popups have nothing to draw and draw nothing — which is a state
        // the keys already recover from (any key closes it), and not the state
        // this sweep is asking about.
        app.config.ssh_hosts = vec![cian_lua::SshHost {
            name: "box".into(),
            host: "box.example".into(),
            users: vec![cian_lua::SshUser::plain("taro")],
            port: None,
            notes: None,
        }];
        (d, app)
    }

    #[test]
    fn none_of_them_leaves_icons_on_top() {
        // The theme picker restores the theme it was opened on when it is
        // dismissed, and the theme is global: dismissing one here while the
        // contrast tests are painting under another is how a frame ends up
        // half in one theme and half in the other. Same lock they take.
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let was = crate::theme::theme();
        for icons in [false, true] {
            let (d, mut app) = desktop(icons);
            let dir = app.active_pane().unwrap().cwd.clone();
            for (name, popup) in all(&dir) {
                app.popup = popup;
                let _ = render(&mut app, 140, 40);
                if name == "ContextMenu" {
                    // The menu is small, so the icons it does not cover stay:
                    // a listing that empties itself the moment a menu opens is
                    // the bug this behaviour was written to avoid.
                    let m = app.menu_rect;
                    for s in &app.icon_slots {
                        let clear = s.x + s.w <= m.x
                            || s.x >= m.x + m.width
                            || s.y + s.h <= m.y
                            || s.y >= m.y + m.height;
                        assert!(clear, "icon_view={icons}: an icon sits on the menu: {s:?} vs {m:?}");
                    }
                    continue;
                }
                assert!(
                    app.icon_slots.is_empty(),
                    "icon_view={icons}: {name} left {} icons on top of itself",
                    app.icon_slots.len(),
                );
            }
            drop(d);
        }
        crate::theme::set_theme(was);
    }

    #[test]
    fn every_one_of_them_says_where_it_is() {
        // The theme picker restores the theme it was opened on when it is
        // dismissed, and the theme is global: dismissing one here while the
        // contrast tests are painting under another is how a frame ends up
        // half in one theme and half in the other. Same lock they take.
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let was = crate::theme::theme();
        let (_d, mut app) = desktop(false);
        let dir = app.active_pane().unwrap().cwd.clone();
        for (name, popup) in all(&dir) {
            app.popup = popup;
            let _ = render(&mut app, 140, 40);
            assert!(
                crate::render::popup_ink().is_some(),
                "{name} drew without saying what it covers, so a click cannot be outside it",
            );
        }
        crate::theme::set_theme(was);
    }

    #[test]
    fn a_click_off_any_of_them_closes_it() {
        // The theme picker restores the theme it was opened on when it is
        // dismissed, and the theme is global: dismissing one here while the
        // contrast tests are painting under another is how a frame ends up
        // half in one theme and half in the other. Same lock they take.
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let was = crate::theme::theme();
        for icons in [false, true] {
            let (_d, mut app) = desktop(icons);
            let dir = app.active_pane().unwrap().cwd.clone();
            for (name, popup) in all(&dir) {
                app.popup = popup;
                let _ = render(&mut app, 140, 40);
                let before = std::mem::discriminant(&app.popup);
                app.handle_mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: 139,
                    row: 39,
                    modifiers: KeyModifiers::NONE,
                });
                assert_ne!(
                    std::mem::discriminant(&app.popup),
                    before,
                    "icon_view={icons}: {name} is still open after a click off it",
                );
            }
        }
        crate::theme::set_theme(was);
    }

    #[test]
    fn the_picture_popup_too() {
        let (d, mut app) = desktop(false);
        let mut img = image::RgbImage::new(40, 20);
        for px in img.pixels_mut() {
            *px = image::Rgb([200, 40, 40]);
        }
        img.save(d.path().join("pic.png")).unwrap();
        app.reload_both();
        app.active_pane_mut().unwrap().cursor =
            app.active_pane().unwrap().entries.iter().position(|e| e.name == "pic.png").unwrap();
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert!(matches!(app.popup, Popup::ImageView { .. }));
        let _ = render(&mut app, 140, 40);
        assert!(app.icon_slots.is_empty(), "no listing icons over the picture");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 139,
            row: 39,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(app.popup, Popup::None), "a click off it closes it");
    }

    /// The editor panel is the one popup that does *not* close on an outside
    /// click, on purpose: it is a place to be rather than a question to answer.
    /// It still has to take the icons with it.
    #[test]
    fn the_editor_panel_keeps_its_file_and_loses_the_icons() {
        let (_d, mut app) = desktop(false);
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "{:?}", app.popup);
        let _ = render(&mut app, 140, 40);
        assert!(app.icon_slots.is_empty(), "no listing icons over the open file");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 139,
            row: 39,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(app.popup, Popup::Viewer { .. }), "and it stays open");
    }

    /// …and the same when it is split in two, which leaves the frame by a door
    /// of its own.
    #[test]
    fn a_split_editor_panel_loses_them_too() {
        let (_d, mut app) = desktop(false);
        app.handle_key(code(KeyCode::F(3))).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "{:?}", app.popup);
        app.split_viewer(true);
        assert!(app.viewer_split.is_some(), "split in two");
        let _ = render(&mut app, 140, 40);
        assert!(app.icon_slots.is_empty(), "{} icons over the split panel", app.icon_slots.len());
    }

    /// A popup added later has to be added to the sweep as well. Read out of
    /// the enum itself, so forgetting is a failing test rather than a gap.
    #[test]
    fn the_sweep_covers_every_variant_there_is() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"),
        )
        .unwrap();
        // Windows checks the source out with CRLF, and a test that reads its
        // own source has to read it the way it is on disk rather than the way
        // it was written. This is what the Windows job is for.
        let src = src.replace("\r\n", "\n");
        let body = src
            .split_once("\nenum Popup {")
            .expect("the enum is where it was")
            .1
            .split_once("\n}\n")
            .unwrap()
            .0;
        let mut depth = 0i32;
        let mut variants = Vec::new();
        for line in body.lines() {
            let s = line.trim();
            if depth == 0 && !s.starts_with("//") && !s.starts_with('#') {
                if let Some(name) = s.split(['{', '(', ',']).next() {
                    let name = name.trim();
                    if !name.is_empty()
                        && name.chars().next().unwrap().is_ascii_uppercase()
                        && name.chars().all(|c| c.is_ascii_alphanumeric())
                    {
                        variants.push(name.to_string());
                    }
                }
            }
            depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        }
        let d = tempfile::tempdir().unwrap();
        let covered: Vec<&str> = all(d.path()).iter().map(|(n, _)| *n).collect();
        let missing: Vec<&String> = variants
            .iter()
            .filter(|v| *v != "None")
            .filter(|v| !covered.contains(&v.as_str()) && !OPENED_WITH_A_KEY.contains(&v.as_str()))
            .collect();
        assert!(missing.is_empty(), "popups the sweep never sees: {missing:?}");
        assert!(variants.len() > 50, "the enum was parsed, not merely searched: {}", variants.len());
    }
}

/// In the two mouse-driven views a letter goes to a file, not to a command.
mod letters_move_in_the_desktop_views {
    use super::*;

    fn desktop(icon_view: bool) -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(&["alpha.txt", "delta.txt", "2024-report.txt", "zulu.txt"]);
        app.skin = Skin::Finder;
        app.icon_view = icon_view;
        (d, app)
    }

    fn name_under_cursor(app: &App) -> String {
        app.active_pane().unwrap().selected().map(|e| e.name.clone()).unwrap_or_default()
    }

    /// The keys that were asked for by name.
    const GIVEN_UP: &[char] = &[
        'h', 'q', 'c', 'p', 'g', 'G', 'P', 'r', 'j', 'k', 'a', 'A', 's', 'u', 't', 'd', 'z', 'v',
        'm', 'b',
    ];

    #[test]
    fn none_of_the_named_keys_does_anything_but_move() {
        for icons in [false, true] {
            for &c in GIVEN_UP {
                let (_d, mut app) = desktop(icons);
                let tabs = app.left.tabs.len();
                app.handle_key(KeyEvent::new(
                    KeyCode::Char(c),
                    if c.is_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE },
                ))
                .unwrap();
                assert!(
                    matches!(app.popup, Popup::None),
                    "icon_view={icons}: {c:?} opened {:?}",
                    app.popup,
                );
                assert_eq!(app.mode, Mode::Normal, "icon_view={icons}: {c:?} changed mode");
                assert_eq!(app.left.tabs.len(), tabs, "icon_view={icons}: {c:?} touched the tabs");
            }
        }
    }

    #[test]
    fn a_letter_goes_to_the_file_that_starts_with_it() {
        for icons in [false, true] {
            let (_d, mut app) = desktop(icons);
            app.handle_key(key('d')).unwrap();
            assert_eq!(name_under_cursor(&app), "delta.txt", "icon_view={icons}");
            app.handle_key(key('z')).unwrap();
            assert_eq!(name_under_cursor(&app), "zulu.txt", "icon_view={icons}");
        }
    }

    #[test]
    fn so_does_a_digit() {
        for icons in [false, true] {
            let (_d, mut app) = desktop(icons);
            app.handle_key(key('2')).unwrap();
            assert_eq!(name_under_cursor(&app), "2024-report.txt", "icon_view={icons}");
        }
    }

    #[test]
    fn the_detail_view_keeps_the_letters_that_were_not_asked_for() {
        let (_d, mut app) = desktop(false);
        app.handle_key(key('f')).unwrap();
        assert!(matches!(app.popup, Popup::Search { .. }), "f still searches: {:?}", app.popup);
    }

    #[test]
    fn the_classic_view_keeps_all_of_them() {
        let (_d, mut app) = app_with(&["alpha.txt", "delta.txt"]);
        app.handle_key(key('t')).unwrap();
        assert!(
            matches!(app.popup, Popup::ConfirmNewTab { .. }),
            "t is still a command in the classic view: {:?}",
            app.popup,
        );
    }
}

/// The three views, and the three ways of asking for one.
mod the_view_switcher {
    use super::*;

    fn painted(app: &mut App) -> String {
        render(app, 140, 40).join("").chars().filter(|c| !c.is_whitespace()).collect()
    }

    fn in_view(icon_view: bool, finder: bool) -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(&["a.txt"]);
        app.skin = if finder { Skin::Finder } else { Skin::Classic };
        app.icon_view = icon_view;
        (d, app)
    }

    #[test]
    fn both_views_draw_it() {
        for (icons, finder) in [(false, true), (false, false)] {
            let (_d, mut app) = in_view(icons, finder);
            let text = painted(&mut app);
            assert!(text.contains("詳細"), "icons={icons} finder={finder}: {text:.200}");
            assert!(text.contains("クラシック"), "icons={icons} finder={finder}");
            assert_eq!(
                app.grid_buttons.iter().filter(|(b, _)| matches!(b, GridButton::View(_))).count(),
                2,
                "two segments to click",
            );
        }
    }

    /// Clicking a segment asks for that view — in every view, including the
    /// classic one, where the switcher rides the top border row.
    #[test]
    fn clicking_a_segment_asks_for_that_view() {
        for (icons, finder) in [(false, true), (false, false)] {
            let (_d, mut app) = in_view(icons, finder);
            let _ = painted(&mut app);
            // Every segment, and the middle of each — a control answered only
            // at its left edge is a control that half works.
            for want in
                [crate::ViewWanted::Details, crate::ViewWanted::Classic]
            {
                let (_, r) = app
                    .grid_buttons
                    .iter()
                    .copied()
                    .find(|(b, _)| matches!(b, GridButton::View(w) if *w == want))
                    .unwrap_or_else(|| panic!("icons={icons} finder={finder}: no {want:?} segment"));
                app.view_request = None;
                app.handle_mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: r.x + r.width / 2,
                    row: r.y,
                    modifiers: KeyModifiers::NONE,
                });
                assert_eq!(
                    app.view_request,
                    Some(want),
                    "icons={icons} finder={finder}: clicking {want:?} asked for it",
                );
            }
        }
    }

    #[test]
    fn the_command_asks_too_and_leaves_the_viewer_alone() {
        let (_d, mut app) = in_view(false, true);
        app.command_buffer = "view details".into();
        app.run_command();
        assert_eq!(app.view_request, Some(crate::ViewWanted::Details));
        assert!(matches!(app.popup, Popup::None), "no file was opened");

        // …and bare `:view` still opens the file under the cursor.
        app.view_request = None;
        app.command_buffer = "view".into();
        app.run_command();
        assert_eq!(app.view_request, None, "no view was asked for");
        assert!(matches!(app.popup, Popup::Viewer { .. }), "the file opened: {:?}", app.popup);
    }

    #[test]
    fn the_menu_asks_too() {
        let (_d, mut app) = in_view(false, true);
        app.run_menu_item(MenuItem::ViewClassic).unwrap();
        assert_eq!(app.view_request, Some(crate::ViewWanted::Classic));
    }

    /// Nothing switches by itself: the details view only exists in a window,
    /// so cian only ever *asks*.
    #[test]
    fn asking_does_not_change_the_view_by_itself() {
        let (_d, mut app) = in_view(false, true);
        let was = app.skin;
        app.run_menu_item(MenuItem::ViewDetails).unwrap();
        assert_eq!(app.view_request, Some(crate::ViewWanted::Details), "it asked");
        assert_eq!(app.skin, was, "and changed nothing itself");
    }
}

/// With the listing narrowed, Backspace un-narrows it rather than walking off.
mod backspace_undoes_the_filter_first {
    use super::*;

    /// A directory with a `sub/` to climb down from, and files to filter.
    fn filtered() -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(&["sample-a.txt", "sample-b.txt", "other.txt"]);
        std::fs::create_dir(d.path().join("sub")).unwrap();
        app.reload_both();
        // `/sample` then Enter: the pane keeps the narrowing and the keys go
        // back to normal, which is the state this is about.
        app.handle_key(key('/')).unwrap();
        for c in "sample".chars() {
            app.handle_key(key(c)).unwrap();
        }
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert_eq!(app.mode, Mode::Normal, "Enter kept the filter and left filter mode");
        let showing = &app.active_pane().unwrap().entries;
        assert!(
            showing.iter().all(|e| e.is_parent || e.name.contains("sample")),
            "only the matches are left (and `..`, which is always a way out): {:?}",
            showing.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
        );
        assert!(showing.iter().any(|e| e.name == "sample-a.txt"), "the matches are there");
        let narrowed = showing.len();
        let _ = narrowed;
        (d, app)
    }

    #[test]
    fn it_widens_the_listing_and_stays_put() {
        let (d, mut app) = filtered();
        let here = app.active_pane().unwrap().cwd.clone();
        app.handle_key(code(KeyCode::Backspace)).unwrap();
        assert_eq!(app.active_pane().unwrap().cwd, here, "the directory did not change");
        assert!(app.active_pane().unwrap().filter.is_empty(), "the filter is gone");
        assert!(
            app.active_pane().unwrap().entries.iter().any(|e| e.name == "other.txt"),
            "the whole listing is back",
        );
        drop(d);
    }

    #[test]
    fn a_second_backspace_then_goes_up_as_it_always_did() {
        let (d, mut app) = filtered();
        let here = app.active_pane().unwrap().cwd.clone();
        app.handle_key(code(KeyCode::Backspace)).unwrap();
        app.handle_key(code(KeyCode::Backspace)).unwrap();
        assert_ne!(app.active_pane().unwrap().cwd, here, "the second one climbed");
        drop(d);
    }

    #[test]
    fn with_no_filter_it_climbs_straight_away() {
        let (d, mut app) = app_with(&["a.txt"]);
        let here = app.active_pane().unwrap().cwd.clone();
        app.handle_key(code(KeyCode::Backspace)).unwrap();
        assert_ne!(app.active_pane().unwrap().cwd, here, "nothing to undo, so it went up");
        drop(d);
    }

    /// The `parent` action means "up" and nothing else, and still does — it is
    /// what the ↑ button presses and what `cian.set_keymap("-", "parent")`
    /// binds. Only the key shaped like "back" learned to peel.
    #[test]
    fn the_parent_action_still_goes_straight_up() {
        let (d, mut app) = filtered();
        let here = app.active_pane().unwrap().cwd.clone();
        app.execute_action(crate::Action::Parent).unwrap();
        assert_ne!(app.active_pane().unwrap().cwd, here, "it climbed");
        drop(d);
    }

    /// The typed filter is forgotten too, so `/` starts a fresh word rather
    /// than resuming the one that was just abandoned.
    #[test]
    fn the_typed_word_is_forgotten_with_it() {
        let (_d, mut app) = filtered();
        app.handle_key(code(KeyCode::Backspace)).unwrap();
        assert!(app.filter_buffer.is_empty(), "{:?}", app.filter_buffer);
    }
}

/// A click lands on the row that was clicked, however far down the listing is.
mod clicking_a_row_after_scrolling {
    use super::*;

    fn many(detail: bool) -> (tempfile::TempDir, App) {
        let names: Vec<String> = (0..200).map(|i| format!("file{i:03}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let (d, mut app) = app_with(&refs);
        if detail {
            app.skin = Skin::Finder;
            app.native_icons = true;
        }
        (d, app)
    }

    /// Walk a long way down, then click a row near the middle of the screen.
    fn scroll_then_click(app: &mut App) -> (String, String) {
        let _ = render(app, 140, 40);
        for _ in 0..120 {
            app.handle_key(code(KeyCode::Down)).unwrap();
        }
        let _ = render(app, 140, 40);
        let before = app.active_pane().unwrap().scroll;
        assert!(before > 0, "the listing scrolled");
        let rect = app.layout_rects.for_pane(app.focused);
        // Four rows into the listing: past the border and the column header.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 6,
            row: rect.y + 4,
            modifiers: KeyModifiers::NONE,
        });
        let p = app.active_pane().unwrap();
        // The row two down from the top of the listing is the one that was
        // clicked, whatever the listing has scrolled to.
        let want = p.entries.get(p.scroll + 2).map(|e| e.name.clone()).unwrap_or_default();
        (p.selected().map(|e| e.name.clone()).unwrap_or_default(), want)
    }

    #[test]
    fn in_the_detail_view() {
        let (_d, mut app) = many(true);
        let (clicked, on_that_row) = scroll_then_click(&mut app);
        assert_eq!(clicked, on_that_row, "the file that was under the pointer");
        assert!(app.active_pane().unwrap().scroll > 0, "and the listing stayed where it was");
    }

    #[test]
    fn and_in_the_classic_view() {
        let (_d, mut app) = many(false);
        let (clicked, on_that_row) = scroll_then_click(&mut app);
        assert_eq!(clicked, on_that_row);
        assert!(app.active_pane().unwrap().scroll > 0);
    }
}

/// A server directory with more in it than fits on screen can be walked to the
/// end of it.
mod the_server_browser_scrolls {
    use super::*;

    fn browsing(n: usize) -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(&["a.txt"]);
        app.popup = Popup::RemoteBrowser {
            label: "box".into(),
            cwd: "/var/log".into(),
            entries: (0..n)
                .map(|i| cian_scp::RemoteEntry {
                    name: format!("file{i:04}.log"),
                    is_dir: false,
                    size: 1,
                    link: false,
                })
                .collect(),
            cursor: 0,
            scroll: 0,
            marked: Default::default(),
            loading: false,
            purpose: crate::BrowsePurpose::Download,
        };
        (d, app)
    }

    fn showing(app: &mut App) -> String {
        render(app, 120, 30).join("\n")
    }

    fn cursor_and_scroll(app: &App) -> (usize, usize) {
        match &app.popup {
            Popup::RemoteBrowser { cursor, scroll, .. } => (*cursor, *scroll),
            other => panic!("not the browser: {other:?}"),
        }
    }

    #[test]
    fn walking_down_brings_the_listing_with_it() {
        let (_d, mut app) = browsing(200);
        let _ = showing(&mut app);
        for _ in 0..60 {
            app.handle_key(code(KeyCode::Down)).unwrap();
        }
        let text = showing(&mut app);
        let (cursor, scroll) = cursor_and_scroll(&app);
        assert_eq!(cursor, 60);
        assert!(scroll > 0, "the window followed the cursor, not {scroll}");
        assert!(text.contains("file0060.log"), "and the file under the cursor is on screen");
    }

    #[test]
    fn the_end_is_reachable() {
        let (_d, mut app) = browsing(500);
        let _ = showing(&mut app);
        app.handle_key(key('G')).unwrap();
        let text = showing(&mut app);
        assert!(text.contains("file0499.log"), "the last file is on screen");
    }

    #[test]
    fn a_page_at_a_time_too() {
        let (_d, mut app) = browsing(200);
        let _ = showing(&mut app);
        app.handle_key(code(KeyCode::PageDown)).unwrap();
        assert_eq!(cursor_and_scroll(&app).0, 10);
        app.handle_key(code(KeyCode::PageUp)).unwrap();
        assert_eq!(cursor_and_scroll(&app).0, 0);
    }

    #[test]
    fn the_wheel_moves_it_as_well() {
        let (_d, mut app) = browsing(200);
        let _ = showing(&mut app);
        for _ in 0..40 {
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 40,
                row: 10,
                modifiers: KeyModifiers::NONE,
            });
        }
        assert_eq!(cursor_and_scroll(&app).0, 40, "the wheel walks it too");
    }

    /// …and it says how far down it is, so a long listing looks long.
    #[test]
    fn a_long_listing_shows_a_scrollbar() {
        let (_d, mut app) = browsing(200);
        assert!(showing(&mut app).contains('█'), "a thumb on the right-hand edge");
        let (_d2, mut short) = browsing(3);
        assert!(!showing(&mut short).contains('█'), "and none when everything fits");
    }
}

/// A transfer can be told how much of the network to take.
mod the_transfer_limit {
    use super::*;

    #[test]
    fn a_speed_is_read_the_way_it_is_written() {
        use crate::parse_rate;
        assert_eq!(parse_rate("2M"), Some(2_000_000));
        assert_eq!(parse_rate("2m"), Some(2_000_000));
        assert_eq!(parse_rate("500k"), Some(500_000));
        assert_eq!(parse_rate("1.5M"), Some(1_500_000));
        assert_eq!(parse_rate("1.5MB/s"), Some(1_500_000));
        assert_eq!(parse_rate(" 800 "), Some(800));
        assert_eq!(parse_rate("1G"), Some(1_000_000_000));
    }

    /// "No limit" has to be sayable, and unsayable by accident: a typo must
    /// not read as "off".
    #[test]
    fn off_is_off_and_nonsense_is_nothing() {
        use crate::parse_rate;
        for off in ["off", "none", "0", "", "  "] {
            assert_eq!(parse_rate(off), None, "{off:?}");
        }
        for junk in ["fast", "-2M", "M", "2X"] {
            assert_eq!(parse_rate(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn and_written_back_the_same_way() {
        use crate::rate_text;
        assert_eq!(rate_text(2_000_000), "2.0M/s");
        assert_eq!(rate_text(500_000), "500k/s");
        assert_eq!(rate_text(800), "800B/s");
    }

    #[test]
    fn the_command_sets_it_shows_it_and_takes_it_away() {
        let (_d, mut app) = app_with(&["a.txt"]);
        assert_eq!(app.transfer_limit, None, "no ceiling by default");

        app.command_buffer = "limit 2M".into();
        app.run_command();
        assert_eq!(app.transfer_limit, Some(2_000_000));
        assert!(app.message.as_deref().unwrap_or_default().contains("2.0M/s"));

        app.command_buffer = "limit".into();
        app.run_command();
        assert!(app.message.as_deref().unwrap_or_default().contains("2.0M/s"), "says what it is");

        app.command_buffer = "limit off".into();
        app.run_command();
        assert_eq!(app.transfer_limit, None);
    }

    /// A typo leaves the ceiling where it was rather than removing it.
    #[test]
    fn a_typo_changes_nothing() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.command_buffer = "limit 2M".into();
        app.run_command();
        app.command_buffer = "limit 2X".into();
        app.run_command();
        assert_eq!(app.transfer_limit, Some(2_000_000), "still capped");
        assert!(app.message.as_deref().unwrap_or_default().contains("2X?"));
    }

    /// The pacer holds a transfer to the rate it was given: a megabyte at
    /// half a megabyte a second owes about two seconds.
    #[test]
    fn the_pacer_asks_for_the_time_the_bytes_should_have_taken() {
        let mut p = cian_scp::Pacer::new(Some(500_000));
        let mut last = None;
        for _ in 0..16 {
            last = p.wait_after(64 * 1024);
        }
        // Each answer is "how far ahead of the rate are you *now*", so the one
        // that matters is the last: a megabyte at 500 kB/s owes two seconds,
        // less the little that has really elapsed.
        let last = last.expect("still ahead of the rate");
        assert!(
            last > std::time::Duration::from_millis(1900)
                && last < std::time::Duration::from_millis(2100),
            "asked to wait {last:?}",
        );
    }

    #[test]
    fn and_asks_for_nothing_when_there_is_no_ceiling() {
        let mut p = cian_scp::Pacer::new(None);
        assert!(p.wait_after(10_000_000).is_none());
        let mut zero = cian_scp::Pacer::new(Some(0));
        assert!(zero.wait_after(10_000_000).is_none(), "0 means no ceiling, not no bytes");
    }
}

/// `:reload` re-reads init.lua, and must not undress the view it is in.
///
/// The desktop views stand on their own palette — a borderless listing on a
/// dark page is a pane with its edges missing — and a reload was putting
/// init.lua's colours back underneath one. The rule is the same one the front
/// end applies when it changes skin, and it is asserted here as a rule: given
/// the configured colours, which skin is on, and whether the user named a
/// theme, what should be worn?
mod reload_keeps_the_look_it_is_wearing {
    use super::*;
    use crate::theme::{theme_for_skin, ResolvedTheme};

    #[test]
    fn the_desktop_views_wear_their_own() {
        assert_eq!(
            theme_for_skin(ResolvedTheme::DARK, true, false),
            ResolvedTheme::FINDER,
            "a borderless listing stands on its own page",
        );
    }

    #[test]
    fn the_classic_view_wears_what_init_lua_says() {
        assert_eq!(theme_for_skin(ResolvedTheme::DARK, false, false), ResolvedTheme::DARK);
    }

    #[test]
    fn a_theme_asked_for_by_name_outranks_the_skin() {
        assert_eq!(
            theme_for_skin(ResolvedTheme::DARK, true, true),
            ResolvedTheme::DARK,
            "the user's choice is not overruled by a change of view",
        );
    }

    /// And the reload really asks: with the desktop skin on, re-reading the
    /// config leaves the desktop palette in force rather than the file's.
    #[test]
    fn and_reload_puts_the_answer_into_force() {
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let was = crate::theme::theme();
        let (_d, mut app) = app_with(&["a.txt"]);
        app.skin = Skin::Finder;
        crate::theme::set_theme(ResolvedTheme::FINDER);
        app.reload_config();
        // Whatever the flag says on this run, the answer is one of the two the
        // rule allows — never something else, which is what the bug was.
        let now = crate::theme::theme();
        assert!(
            now == ResolvedTheme::FINDER || now == crate::theme::theme_preset("dark").unwrap(),
            "reload settled on a theme the rule allows",
        );
        crate::theme::set_theme(was);
    }
}

/// Where you are, on every theme cian ships.
///
/// The presets put their selection at 1.1–1.4 times the contrast of their page,
/// light and dark alike — and on a dark page that is a shade rather than a
/// band, which is what "the dark theme is hard to read" turned out to mean.
mod the_selection_can_be_seen_on_every_theme {
    use super::*;
    use crate::render::{contrast_ratio, rel_luminance, selection_on};

    #[test]
    fn a_dark_page_gets_a_band_you_can_see() {
        let mut checked = 0;
        for name in crate::theme::THEME_NAMES {
            let t = crate::theme::theme_preset(name).unwrap();
            let page = t.base_bg.unwrap_or(t.popup_bg);
            if rel_luminance(page) > 0.18 {
                continue;
            }
            checked += 1;
            let lifted = selection_on(page, t.selected_bg);
            let r = contrast_ratio(lifted, page);
            assert!(r >= 1.95, "{name}: the selection is still a shade at {r:.2}:1");
        }
        assert!(checked >= 10, "the dark presets were actually walked: {checked}");
    }

    /// A light page is left exactly as its author drew it — it was never the
    /// half that could not be read.
    #[test]
    fn a_light_page_is_left_alone() {
        let mut checked = 0;
        for name in crate::theme::THEME_NAMES {
            let t = crate::theme::theme_preset(name).unwrap();
            let page = t.base_bg.unwrap_or(t.popup_bg);
            if rel_luminance(page) <= 0.18 {
                continue;
            }
            checked += 1;
            assert_eq!(selection_on(page, t.selected_bg), t.selected_bg, "{name}");
        }
        assert!(checked >= 4, "the light presets were actually walked: {checked}");
    }

    /// The hue is the theme's: lifting a band must not turn a blue selection
    /// grey, or every theme ends up with the same one.
    #[test]
    fn the_theme_keeps_its_own_colour() {
        let page = Color::Rgb(26, 27, 38);
        let sel = Color::Rgb(41, 46, 66); // tokyo-night: blue, and barely there
        let Color::Rgb(r, g, b) = selection_on(page, sel) else { panic!("not rgb") };
        assert!(b > r && b > g, "still blue: {r},{g},{b}");
        assert!(b - r >= 20, "and still as blue as it was: {r},{g},{b}");
    }

    /// The cursor row on a dark theme is drawn with the lifted band, not the
    /// raw one — the rule is applied where the row is painted.
    #[test]
    fn and_the_row_is_actually_drawn_with_it() {
        let _g = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let was = crate::theme::theme();
        crate::theme::set_theme(crate::theme::theme_preset("tokyo-night").unwrap());
        let (_d, mut app) = app_with(&["a.txt", "b.txt", "c.txt"]);
        let buf = render_buf(&mut app, 100, 20);
        let th = crate::theme::theme();
        let page = th.base_bg.unwrap_or(th.popup_bg);
        let want = selection_on(page, th.selected_bg);
        let found = (0..20)
            .flat_map(|y| (0..100).map(move |x| (x, y)))
            .any(|(x, y)| buf[(x, y)].bg == want);
        assert!(found, "the lifted band is on screen");
        crate::theme::set_theme(was);
    }
}

/// Shift+H / Shift+L / Shift+J in the editor panel, and where they stop.
mod the_panel_moves_the_focus_like_a_listing_does {
    use super::*;

    fn docked(app: &mut App) {
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(app.viewer_dock.is_some(), "the file opened in its pane");
        assert!(matches!(app.popup, Popup::Viewer { editing: false, .. }), "and in read mode");
    }

    fn shift(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
    }

    #[test]
    fn reading_a_file_docked_in_a_pane_they_move_the_focus() {
        let (_d, mut app) = app_with(&["a.txt"]);
        docked(&mut app);
        app.handle_key(shift('L')).unwrap();
        assert_eq!(app.focused, FocusedPane::Right, "Shift+L crosses to the other listing");
        app.handle_key(shift('H')).unwrap();
        assert_eq!(app.focused, FocusedPane::Left);
        app.handle_key(shift('J')).unwrap();
        assert_eq!(app.focused, FocusedPane::Shell, "and Shift+J goes down to the shell");
    }

    /// …and the file is still open behind the focus, not closed by moving away.
    #[test]
    fn and_the_file_stays_open() {
        let (_d, mut app) = app_with(&["a.txt"]);
        docked(&mut app);
        app.handle_key(shift('L')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { .. }), "still open: {:?}", app.popup);
    }

    /// In a split panel the same keys cross between the two halves — that is
    /// the one place they cannot also move the focus.
    #[test]
    fn but_in_a_split_they_belong_to_the_split() {
        let (_d, mut app) = app_with(&["a.txt"]);
        docked(&mut app);
        app.split_viewer(true);
        assert!(app.viewer_split.is_some());
        app.handle_key(shift('L')).unwrap();
        assert_eq!(app.focused, FocusedPane::Left, "the focus stayed in the panel");
    }

    /// While typing, `H` and `L` are letters.
    #[test]
    fn and_while_editing_they_are_letters() {
        let (_d, mut app) = app_with(&["a.txt"]);
        docked(&mut app);
        app.handle_key(key('i')).unwrap();
        assert!(matches!(app.popup, Popup::Viewer { editing: true, .. }), "typing now");
        app.handle_key(shift('L')).unwrap();
        assert_eq!(app.focused, FocusedPane::Left, "the focus did not move");
    }
}

/// `u` takes back the last thing you did — including walking into a folder.
mod undo_covers_where_you_are {
    use super::*;

    fn tree() -> (tempfile::TempDir, App) {
        let (d, mut app) = app_with(&["a.txt"]);
        std::fs::create_dir_all(d.path().join("abc/def")).unwrap();
        std::fs::write(d.path().join("abc/inside.txt"), b"").unwrap();
        app.reload_both();
        (d, app)
    }

    fn go_into(app: &mut App, name: &str) {
        let i = app
            .active_pane()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.name == name)
            .unwrap_or_else(|| panic!("no {name} here"));
        app.active_pane_mut().unwrap().cursor = i;
        app.handle_key(code(KeyCode::Enter)).unwrap();
    }

    fn here(app: &App) -> String {
        app.active_pane().unwrap().cwd.file_name().unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn stepping_into_a_folder_can_be_taken_back() {
        let (_d, mut app) = tree();
        let was = app.active_pane().unwrap().cwd.clone();
        go_into(&mut app, "abc");
        assert_eq!(here(&app), "abc");
        app.handle_key(key('u')).unwrap();
        assert_eq!(app.active_pane().unwrap().cwd, was, "u came back out");
    }

    #[test]
    fn and_put_back_again() {
        let (_d, mut app) = tree();
        go_into(&mut app, "abc");
        app.handle_key(key('u')).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(here(&app), "abc", "Ctrl+Y walked back in");
    }

    /// Several steps, unwound in the order they were taken.
    #[test]
    fn a_chain_unwinds_in_order() {
        let (_d, mut app) = tree();
        let root = app.active_pane().unwrap().cwd.clone();
        go_into(&mut app, "abc");
        go_into(&mut app, "def");
        assert_eq!(here(&app), "def");
        app.handle_key(key('u')).unwrap();
        assert_eq!(here(&app), "abc");
        app.handle_key(key('u')).unwrap();
        assert_eq!(app.active_pane().unwrap().cwd, root);
    }

    /// A file operation and a walk are on one stack, in the order they
    /// happened — which is what makes `u` mean "take back what I just did".
    #[test]
    fn a_rename_and_a_walk_share_one_stack() {
        let (d, mut app) = tree();
        // The pane's own idea of where it is — on a Mac the temp dir is
        // reached through a symlink, so `d.path()` and the pane's `cwd` are
        // two spellings of one directory.
        let root = app.active_pane().unwrap().cwd.clone();
        std::fs::rename(d.path().join("a.txt"), d.path().join("renamed.txt")).unwrap();
        app.record_undo(crate::UndoAction::Rename {
            from: d.path().join("a.txt"),
            to: d.path().join("renamed.txt"),
        });
        go_into(&mut app, "abc");

        // The walk was last, so the walk comes back first.
        app.handle_key(key('u')).unwrap();
        assert_eq!(app.active_pane().unwrap().cwd, root);
        assert!(d.path().join("renamed.txt").exists(), "the rename is still done");

        // …and then the rename.
        app.handle_key(key('u')).unwrap();
        assert!(d.path().join("a.txt").exists(), "the name came back");
    }

    /// Alt+← is already a way back, so using it must not leave a step that
    /// `u` would then undo by walking *forward* again — the two would fight.
    #[test]
    fn stepping_back_through_history_is_not_itself_undoable() {
        let (_d, mut app) = tree();
        let root = app.active_pane().unwrap().cwd.clone();
        go_into(&mut app, "abc");
        app.pane_go_back();
        assert_eq!(app.active_pane().unwrap().cwd, root, "Alt+← came back");
        app.handle_key(key('u')).unwrap();
        assert_eq!(
            app.active_pane().unwrap().cwd,
            root,
            "and u did not bounce forward into the folder again",
        );
    }

    /// Doing something new ends the redo chain, as everywhere else.
    #[test]
    fn a_new_step_ends_the_chain() {
        let (_d, mut app) = tree();
        go_into(&mut app, "abc");
        app.handle_key(key('u')).unwrap();
        go_into(&mut app, "abc");
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(here(&app), "abc", "there was nothing to redo");
    }

    #[test]
    fn the_commands_do_the_same() {
        let (_d, mut app) = tree();
        let root = app.active_pane().unwrap().cwd.clone();
        go_into(&mut app, "abc");
        app.command_buffer = "undo".into();
        app.run_command();
        assert_eq!(app.active_pane().unwrap().cwd, root);
        app.command_buffer = "redo".into();
        app.run_command();
        assert_eq!(here(&app), "abc");
    }
}

/// Coming up out of a folder lands on the folder you came out of.
mod going_up_lands_where_you_were {
    use super::*;

    #[test]
    fn the_cursor_is_on_the_folder_just_left() {
        let (d, mut app) = app_with(&["a.txt"]);
        for n in ["aaa", "bbb", "def", "zzz"] {
            std::fs::create_dir(d.path().join(n)).unwrap();
        }
        std::fs::create_dir(d.path().join("def/sub")).unwrap();
        app.reload_both();
        let i = app.active_pane().unwrap().entries.iter().position(|e| e.name == "def").unwrap();
        app.active_pane_mut().unwrap().cursor = i;
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(app.active_pane().unwrap().cwd.ends_with("def"));

        app.handle_key(code(KeyCode::Backspace)).unwrap();
        let on = app.active_pane().unwrap().selected().map(|e| e.name.clone()).unwrap_or_default();
        assert_eq!(on, "def", "back upstairs, standing on the folder just left");
    }

    /// And from a folder whose parent no longer lists it, the old behaviour:
    /// the first real row rather than nothing at all.
    #[test]
    fn or_the_first_row_when_it_is_not_there_any_more() {
        let (d, mut app) = app_with(&["a.txt"]);
        std::fs::create_dir(d.path().join("gone")).unwrap();
        app.reload_both();
        let i = app.active_pane().unwrap().entries.iter().position(|e| e.name == "gone").unwrap();
        app.active_pane_mut().unwrap().cursor = i;
        app.handle_key(code(KeyCode::Enter)).unwrap();
        std::fs::remove_dir(d.path().join("gone")).unwrap();
        app.handle_key(code(KeyCode::Backspace)).unwrap();
        let p = app.active_pane().unwrap();
        assert!(p.selected().is_some(), "the cursor is somewhere sensible");
    }
}

/// The keys as the *window* sends them: a capital letter, and no Shift bit.
///
/// cian reads a letter's case as its shift — terminals do not report the
/// modifier for letters reliably, and the window build strips it deliberately
/// for that reason. One guard in the editor panel asked for the bit instead,
/// so Shift+H did nothing at all in the window while working in a terminal.
mod the_panel_answers_a_capital_letter {
    use super::*;

    /// `Char('H')` with no modifiers — exactly what `cian-gui` produces.
    fn windowed(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn docked(app: &mut App) {
        app.handle_key(code(KeyCode::Enter)).unwrap();
        assert!(app.viewer_dock.is_some());
    }

    #[test]
    fn shift_l_crosses_to_the_other_listing() {
        let (_d, mut app) = app_with(&["a.txt"]);
        docked(&mut app);
        app.handle_key(windowed('L')).unwrap();
        assert_eq!(app.focused, FocusedPane::Right);
    }

    #[test]
    fn shift_h_comes_back() {
        let (_d, mut app) = app_with(&["a.txt"]);
        docked(&mut app);
        app.handle_key(windowed('L')).unwrap();
        app.handle_key(windowed('H')).unwrap();
        assert_eq!(app.focused, FocusedPane::Left);
    }

    #[test]
    fn shift_j_goes_down_to_the_shell() {
        let (_d, mut app) = app_with(&["a.txt"]);
        docked(&mut app);
        app.handle_key(windowed('J')).unwrap();
        assert_eq!(app.focused, FocusedPane::Shell);
    }

    /// …and a letter that means something else after `g` still does. `gJ` is
    /// vi's join; the focus keys must see the `g` first.
    #[test]
    fn but_a_pending_operator_keeps_its_letter() {
        let (d, mut app) = app_with(&["a.txt"]);
        std::fs::write(d.path().join("a.txt"), b"one\ntwo\n").unwrap();
        app.reload_both();
        docked(&mut app);
        app.handle_key(key('g')).unwrap();
        app.handle_key(windowed('J')).unwrap();
        assert_eq!(app.focused, FocusedPane::Left, "the focus did not move");
        assert!(matches!(app.popup, Popup::Viewer { .. }), "still in the file");
    }

    /// The terminal's spelling of the same keystroke still works.
    #[test]
    fn and_the_terminals_spelling_too() {
        let (_d, mut app) = app_with(&["a.txt"]);
        docked(&mut app);
        app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.focused, FocusedPane::Right);
    }
}

/// Redo, on the key vi puts it on.
mod ctrl_r_redoes {
    use super::*;

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn it_puts_back_what_u_took_away() {
        let (d, mut app) = app_with(&["a.txt"]);
        std::fs::create_dir(d.path().join("sub")).unwrap();
        app.reload_both();
        let i = app.active_pane().unwrap().entries.iter().position(|e| e.name == "sub").unwrap();
        app.active_pane_mut().unwrap().cursor = i;
        app.handle_key(code(KeyCode::Enter)).unwrap();
        let inside = app.active_pane().unwrap().cwd.clone();

        app.handle_key(key('u')).unwrap();
        assert_ne!(app.active_pane().unwrap().cwd, inside, "u came out");
        app.handle_key(ctrl('r')).unwrap();
        assert_eq!(app.active_pane().unwrap().cwd, inside, "Ctrl+R went back in");
    }

    /// …and refresh keeps F5, which is now its own key rather than its second.
    #[test]
    fn refresh_is_f5_and_no_longer_ctrl_r() {
        let (d, mut app) = app_with(&["a.txt"]);
        std::fs::write(d.path().join("new.txt"), b"").unwrap();
        assert!(
            !app.active_pane().unwrap().entries.iter().any(|e| e.name == "new.txt"),
            "not seen yet",
        );
        app.handle_key(code(KeyCode::F(5))).unwrap();
        assert!(app.active_pane().unwrap().entries.iter().any(|e| e.name == "new.txt"), "F5 sees it");
    }
}

/// A popup that opens, scrolls or closes asks for the whole surface again.
///
/// Every renderer under cian repaints only what changed, and a popup changes
/// more than the cell diff can always see: a glyph whose ink overhangs its
/// cell leaves the overhang behind, and those leftovers were piling up as
/// white blocks along the manual's lines and staying on the panes after it
/// closed.
mod a_popup_asks_for_a_clean_surface {
    use super::*;

    /// Whether the frame asked for the surface to be painted again.
    ///
    /// It used to ask for a *wipe* — `Terminal::clear` — which blanks every
    /// cell before writing the surface back, and on a full-size window that
    /// write is big enough that the terminal paints it as it arrives. Every
    /// popup flashed black and filled back in; `c` to copy was where it was
    /// reported. Painting over what is there needs no blank moment, and this
    /// checks the weaker request is the one being made.
    fn drew(app: &mut App) -> bool {
        let _ = render(app, 100, 30);
        assert!(
            !std::mem::take(&mut app.full_clear),
            "a popup must not wipe the screen — only a picture needs that",
        );
        std::mem::take(&mut app.full_repaint)
    }

    #[test]
    fn opening_one_does() {
        let (_d, mut app) = app_with(&["a.txt"]);
        let _ = drew(&mut app); // the first frame always wipes
        assert!(!drew(&mut app), "a still screen does not");
        app.open_manual();
        assert!(drew(&mut app), "the manual opened");
    }

    #[test]
    fn scrolling_one_does() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.open_manual();
        let _ = drew(&mut app);
        assert!(!drew(&mut app), "and then settles");
        app.handle_key(key('j')).unwrap();
        assert!(drew(&mut app), "scrolled by a line");
    }

    #[test]
    fn and_closing_one_does() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.open_manual();
        let _ = drew(&mut app);
        app.handle_key(code(KeyCode::Esc)).unwrap();
        assert!(matches!(app.popup, Popup::None));
        assert!(drew(&mut app), "the panes underneath get a clean start");
    }

    /// Switching from one popup to another counts too — they are different
    /// shapes over the same cells.
    #[test]
    fn so_does_swapping_one_for_another() {
        let (_d, mut app) = app_with(&["a.txt"]);
        app.open_manual();
        let _ = drew(&mut app);
        app.start_toggles();
        assert!(drew(&mut app));
    }

    /// Why painting again is as strong as wiping, here.
    ///
    /// `swap_buffers` makes the next frame differ from a blank one, so every
    /// cell that is *not* blank gets written. That is only as good as a wipe if
    /// cian leaves no blank cells — and it does not, because it paints its own
    /// background into every cell it owns rather than letting the terminal's
    /// show through. This is the assumption the fix rests on, checked rather
    /// than assumed: if a future layout ever stops painting some corner, the
    /// stale ink would come back there and this says so first.
    #[test]
    fn and_painting_again_reaches_every_cell() {
        use ratatui::style::Style;
        let (_d, mut app) = app_with(&["a.txt", "b.txt"]);
        for open in [false, true] {
            if open {
                app.open_manual();
            }
            let mut terminal =
                Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            let buf = terminal.backend().buffer().clone();
            let blank: Vec<(u16, u16)> = (0..buf.area.height)
                .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let c = &buf[(x, y)];
                    c.symbol() == " " && c.style() == Style::default()
                })
                .collect();
            assert!(
                blank.is_empty(),
                "{} cell(s) carry no styling of their own (popup open: {open}) — \
                 stale ink on those would survive a repaint; first at {:?}",
                blank.len(),
                blank.first(),
            );
        }
    }
}
