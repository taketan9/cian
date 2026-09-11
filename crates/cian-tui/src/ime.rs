//! Switching the system's input method with cian's mode.
//!
//! A Japanese IME sits between the keyboard and the terminal: while it is
//! composing, a letter never reaches cian at all, so `j`, `d`, `y` — every
//! single-key command — are dead until it is switched off by hand. Nothing
//! cian does to the keys it receives can fix that, because it does not receive
//! them.
//!
//! So cian switches the input method itself, on one rule:
//!
//! * **Driving cian** — the file panes, the viewer being read, a selection —
//!   is *always* the off source. A command key must never be eaten.
//! * **Typing into cian** — `:`, `/`, a rename, the chat, the shell, the
//!   viewer's editor — gets back **whatever the user was last typing with**.
//!
//! The second half is the point. cian has no business deciding that text means
//! Japanese: the user knows, and they say so by leaving the input method where
//! they want it. Every time cian takes the keyboard back for commands it first
//! *reads* the source and remembers it, so the next prompt opens the way the
//! last one was left. Turn the IME off mid-rename and the next rename starts
//! off; turn it on and the next one starts on.
//!
//! Until something is remembered, nothing is restored — text simply opens with
//! the input method off, and the user turns it on when they want it. That is
//! also what happens when the helper cannot be read, so the degraded case is
//! the honest default rather than a surprise.
//!
//! ```lua
//! cian.ime{
//!   helper = "cian-ime",                 -- prints the current source; sets the one named
//!   off    = "com.apple.keylayout.ABC",  -- the source that means "no IME"
//! }
//! ```
//!
//! `macism`, `im-select` and cian's own `examples/cian-ime.swift` all have that
//! shape. Commands run on one worker thread, in the order they were asked for
//! — a read-then-switch pair landing out of order would remember the source
//! cian had just set — and never on the UI thread, so a slow or missing helper
//! cannot wedge the keyboard.

use super::*;

impl App {
    /// Is cian taking text right now, rather than being driven by commands?
    ///
    /// This is the whole rule. Everything that reads a typed string says yes;
    /// the file panes in normal mode and the viewer being read say no.
    pub(crate) fn wants_text_input(&self) -> bool {
        match &self.popup {
            Popup::None => {
                matches!(self.mode, Mode::Command | Mode::Filter | Mode::Search)
                    || self.focused == FocusedPane::Shell
            }
            // The viewer is vim: reading is commands, and only the prompts and
            // the editor take text.
            Popup::Viewer { find_input, sub_input, block_input, editing, .. } => {
                find_input.is_some()
                    || sub_input.is_some()
                    || block_input.is_some()
                    || *editing
            }
            Popup::TextInput { .. }
            | Popup::Search { .. }
            | Popup::Palette { .. }
            | Popup::AiChat { .. }
            | Popup::CommitMessage { .. }
            | Popup::Snippets { .. }
            | Popup::GrepReplace(_) => true,
            // Everything else is a list to steer or a question to answer with
            // one key.
            _ => false,
        }
    }

    /// Put the input method where this moment wants it. Cheap to call — it
    /// compares one bool and does nothing until the answer changes — so it
    /// runs once per turn round the event loop rather than being remembered at
    /// every place that opens a prompt.
    pub(crate) fn sync_ime(&mut self) {
        let Some(cfg) = self.config.ime.clone() else { return };
        self.report_ime_failure();
        let want = self.wants_text_input();
        if self.ime_on == Some(want) {
            return;
        }
        self.ime_on = Some(want);
        queue(if want { Job::EnterText } else { Job::LeaveText }, &cfg);
    }

    /// Say so when a switch failed. It happens on a worker thread, so this is
    /// where the news is collected — once per failure.
    fn report_ime_failure(&mut self) {
        let Ok(mut slot) = last_switch().lock() else { return };
        let Some(log) = slot.as_mut() else { return };
        if log.reported {
            return;
        }
        log.reported = true;
        if let Some(e) = &log.error {
            self.message = Some(format!("ime: {} — {e}", log.cmd));
        }
    }

    /// Hand the keyboard back on the way out, as it was found: whatever the
    /// user was last typing with, not whatever cian last set.
    pub(crate) fn release_ime(&self) {
        let Some(cfg) = &self.config.ime else { return };
        if !cfg.restore || self.ime_on != Some(false) {
            return;
        }
        // Through the same queue, so anything still in flight lands first —
        // then waited for, briefly: the process is about to end, and a switch
        // that never ran is worse than a few milliseconds at exit.
        let (tx, rx) = std::sync::mpsc::channel();
        queue(Job::Restore(tx), cfg);
        let _ = rx.recv_timeout(std::time::Duration::from_secs(3));
    }

    /// `:ime` — what is configured, what cian remembers, and what the last
    /// switch actually did. The answer to "it is set up and nothing happens".
    /// Say that the input method is on, when a keystroke has just proved it.
    ///
    /// Once per run of them, not once per key: a committed composition arrives
    /// as a burst, and five identical complaints in a row is noise rather than
    /// news. The flag clears as soon as a key arrives that an input method
    /// could not have produced, so the next time it happens it is said again.
    ///
    /// It names the fix it can offer. Where `cian.ime{}` is configured this
    /// should never fire — the input method is switched off before a command
    /// key is ever waited for — so the message doubles as a sign that the
    /// helper is not set up or is not working, which is the other half of what
    /// a person in this situation needs to know.
    pub(crate) fn note_ime_is_on(&mut self) {
        if self.ime_warned {
            return;
        }
        self.ime_warned = true;
        self.message = Some(if let Some(cfg) = self.config.ime.clone() {
            // Configured, and on anyway — so it was turned on *after* cian last
            // threw the switch. cian only switches on transitions: it sets the
            // input method off on the way into command mode and never looks
            // again, so a hand (or the OS, or another window's focus) that
            // turns it back on afterwards is invisible. This keystroke is that
            // event finally becoming visible, and the switch is worth throwing
            // again rather than only complaining about.
            self.ime_on = Some(false);
            queue(Job::LeaveText, &cfg);
            tr(
                self.lang,
                "IME was on — switched off so the keys work again",
                "IME が ON でした — コマンドが効くよう OFF に戻しました",
            )
            .into()
        } else {
            tr(
                self.lang,
                "IME is on — commands need it off. cian can switch it for you: see cian.ime{} (:ime)",
                "IME が ON です — コマンドは OFF で。cian が自動で切替できます: cian.ime{}（:ime）",
            )
            .into()
        });
    }

    pub(crate) fn ime_report(&mut self, arg: &str) {
        let Some(cfg) = self.config.ime.clone() else {
            self.open_popup(Popup::Notice { lines: not_configured(self.lang) });
            return;
        };
        // `:ime on` / `:ime off` switch now, so a helper can be tested without
        // hunting for the mode that would trigger it.
        match arg.trim() {
            "off" => {
                self.ime_on = Some(false);
                queue(Job::LeaveText, &cfg);
                self.message = Some(tr(self.lang, "ime: off", "ime: オフ").into());
                return;
            }
            "on" => {
                self.ime_on = Some(true);
                queue(Job::EnterText, &cfg);
                self.message = Some(
                    tr(
                        self.lang,
                        "ime: back to what you were typing with",
                        "ime: 直前の入力状態に復帰",
                    )
                    .into(),
                );
                return;
            }
            _ => {}
        }
        let state = match self.ime_on {
            Some(true) => tr(self.lang, "typing (your own input source)", "入力中（記憶した入力ソース）"),
            Some(false) => tr(self.lang, "driving cian (off)", "操作中（オフ）"),
            None => tr(self.lang, "not set yet", "未適用"),
        };
        let remembered = remembered_source().unwrap_or_else(|| {
            tr(self.lang, "(nothing yet — text opens off)", "（まだ覚えていません — 入力のときもオフのまま）").into()
        });
        let last = match last_switch()
            .lock()
            .ok()
            .and_then(|s| s.as_ref().map(|l| (l.cmd.clone(), l.error.clone())))
        {
            Some((cmd, None)) => format!("{cmd}  ✔"),
            Some((cmd, Some(e))) => format!("{cmd}  ⚠ {e}"),
            None => tr(self.lang, "(none run yet)", "（まだ実行されていません）").to_string(),
        };
        let dash = "—".to_string();
        let lines = vec![
            tr(self.lang, "input method switching", "日本語入力の自動切替").to_string(),
            String::new(),
            format!("{:<10}: {}", "state", state),
            format!("{:<10}: {}", "remembered", remembered),
            String::new(),
            format!("{:<10}: {}", "query", cfg.query_cmd().unwrap_or_else(|| dash.clone())),
            format!("{:<10}: {}", "set", cfg.set_cmd("<id>").unwrap_or_else(|| dash.clone())),
            format!("{:<10}: {}", "off", cfg.off.clone().unwrap_or(dash)),
            format!("{:<10}: {}", "restore", cfg.restore),
            format!("{:<10}: {}", "last", last),
            String::new(),
            tr(
                self.lang,
                "Commands are always off. Typing gets back whatever you last",
                "コマンド操作中は常にオフ。入力するときは、直前に自分が使って",
            )
            .to_string(),
            tr(
                self.lang,
                "typed with — cian reads it each time it takes the keyboard back",
                "いた入力ソースに戻します（毎回読み取って覚えています）",
            )
            .to_string(),
            tr(
                self.lang,
                ":ime on / :ime off switch now, to check the helper works",
                ":ime on / :ime off でその場で切り替えて動作確認できます",
            )
            .to_string(),
        ];
        self.open_popup(Popup::Notice { lines });
    }
}

fn not_configured(lang: Lang) -> Vec<String> {
    vec![
        tr(lang, "the input-method switch is not configured", "日本語入力の切替は未設定です").to_string(),
        String::new(),
        tr(
            lang,
            "Add this to init.lua, with a helper that prints the current input",
            "init.lua に次を書いてください。現在の入力ソースを表示し、引数で",
        )
        .to_string(),
        tr(lang, "source and switches to the one it is given:", "切り替えるヘルパーが必要です:")
            .to_string(),
        String::new(),
        "  cian.ime{".to_string(),
        "    helper = \"cian-ime\",".to_string(),
        "    off    = \"com.apple.keylayout.ABC\",".to_string(),
        "  }".to_string(),
        String::new(),
        tr(
            lang,
            "macism, im-select, or cian's own examples/cian-ime.swift",
            "macism / im-select、または同梱の examples/cian-ime.swift",
        )
        .to_string(),
    ]
}

/// What cian is asked to do, in the order it was asked.
enum Job {
    /// Going back to commands: read what the user is typing with, remember it,
    /// then switch the input method off.
    LeaveText,
    /// Taking text: switch back to what was remembered (nothing to do until
    /// something has been).
    EnterText,
    /// On the way out: put back what the user was typing with. The channel is
    /// signalled — by being dropped — once it is done.
    Restore(std::sync::mpsc::Sender<()>),
}

/// The input source the user was last typing with. Written on the worker
/// thread, when cian takes the keyboard back for commands.
fn remembered() -> &'static std::sync::Mutex<Option<String>> {
    static R: std::sync::OnceLock<std::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(None))
}

pub(crate) fn remembered_source() -> Option<String> {
    remembered().lock().ok().and_then(|r| r.clone())
}

/// What the last switch cian ran did — the command, and what it said if it
/// failed.
///
/// A switch runs on a worker thread, so its failure has nowhere to be seen at
/// the moment it happens. Without this, a helper that is missing or renamed is
/// perfectly silent: cian goes on believing the input method is where it asked
/// for it to be, and the only symptom is that Japanese input is still on.
struct SwitchLog {
    cmd: String,
    error: Option<String>,
    /// Set once the failure has been put on screen, so it is said once.
    reported: bool,
}

fn last_switch() -> &'static std::sync::Mutex<Option<SwitchLog>> {
    static L: std::sync::OnceLock<std::sync::Mutex<Option<SwitchLog>>> = std::sync::OnceLock::new();
    L.get_or_init(|| std::sync::Mutex::new(None))
}

/// One worker thread, fed in order.
///
/// Order is why there is a queue at all. `LeaveText` reads the current source
/// and `EnterText` writes it back; the two landing out of order would have
/// cian remember the source it had just set — the off one — and then "restore"
/// the user into no input method at all, permanently.
fn queue(job: Job, cfg: &cian_lua::ImeOptions) {
    struct Work {
        job: Job,
        cfg: cian_lua::ImeOptions,
    }
    static Q: std::sync::OnceLock<std::sync::mpsc::Sender<Work>> = std::sync::OnceLock::new();
    let tx = Q.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Work>();
        std::thread::spawn(move || {
            for w in rx {
                run(w.job, &w.cfg);
            }
        });
        tx
    });
    let _ = tx.send(Work { job, cfg: cfg.clone() });
}

fn run(job: Job, cfg: &cian_lua::ImeOptions) {
    let Some(off) = cfg.off.clone() else { return };
    match job {
        Job::LeaveText => {
            // Read first: this is the one moment cian knows what the user
            // chose, because they have just finished typing with it.
            if let Some(current) = read_source(cfg) {
                if let Ok(mut r) = remembered().lock() {
                    *r = Some(current);
                }
            }
            switch_to(cfg, &off);
        }
        Job::EnterText => {
            // Nothing remembered yet: leave the keyboard off rather than
            // guess. The user turns their input method on, and *that* is what
            // gets remembered for next time.
            if let Some(want) = remembered_source() {
                switch_to(cfg, &want);
            }
        }
        Job::Restore(done) => {
            if let Some(want) = remembered_source() {
                switch_to(cfg, &want);
            }
            drop(done);
        }
    }
}

/// Run the query command and take its first line as the current source id.
fn read_source(cfg: &cian_lua::ImeOptions) -> Option<String> {
    let cmd = cfg.query_cmd()?;
    match shell_command(&cmd).output() {
        Ok(o) if o.status.success() => {
            note(&cmd, None);
            let id = String::from_utf8_lossy(&o.stdout).lines().next()?.trim().to_string();
            (!id.is_empty()).then_some(id)
        }
        Ok(o) => {
            note(&cmd, Some(failure_text(&o)));
            None
        }
        Err(e) => {
            note(&cmd, Some(e.to_string()));
            None
        }
    }
}

fn switch_to(cfg: &cian_lua::ImeOptions, id: &str) {
    let Some(cmd) = cfg.set_cmd(id) else { return };
    match shell_command(&cmd).output() {
        Ok(o) if o.status.success() => note(&cmd, None),
        Ok(o) => note(&cmd, Some(failure_text(&o))),
        Err(e) => note(&cmd, Some(e.to_string())),
    }
}

fn failure_text(o: &std::process::Output) -> String {
    let said = String::from_utf8_lossy(&o.stderr);
    let said = said.trim();
    if said.is_empty() {
        format!("exit {}", o.status.code().unwrap_or(-1))
    } else {
        said.chars().take(120).collect()
    }
}

fn note(cmd: &str, error: Option<String>) {
    if let Ok(mut slot) = last_switch().lock() {
        *slot = Some(SwitchLog { cmd: cmd.to_string(), error, reported: false });
    }
}

/// A command line as the platform's shell would run it, so the config can hold
/// exactly what the user would type.
fn shell_command(cmd: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = cian_core::proc::quiet("cmd");
        c.args(["/C", cmd]);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = cian_core::proc::quiet("sh");
        c.args(["-c", cmd]);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue, what it remembers and its last result are process-wide — one
    /// keyboard, one switch — so the tests that watch them take turns.
    static SWITCH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Start from a known state: wait for the worker to finish anything an
    /// earlier test queued (it outlives that test's lock), then forget.
    fn clear_state() {
        let (tx, rx) = std::sync::mpsc::channel();
        queue(
            Job::Restore(tx),
            &cian_lua::ImeOptions { off: Some("x".into()), ..Default::default() },
        );
        let _ = rx.recv_timeout(std::time::Duration::from_secs(10));
        *remembered().lock().unwrap_or_else(|e| e.into_inner()) = None;
        *last_switch().lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// The rule, stated once: a keystroke is either text or a command, and the
    /// input method follows that and nothing else.
    #[test]
    fn text_and_command_states_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, cian_lua::Config::default()).unwrap();

        assert!(!app.wants_text_input(), "the panes in normal mode are commands");
        app.mode = Mode::Command;
        assert!(app.wants_text_input(), "the `:` line is text");
        app.mode = Mode::Filter;
        assert!(app.wants_text_input(), "so is a filter");
        app.mode = Mode::Normal;
        app.focused = FocusedPane::Shell;
        assert!(app.wants_text_input(), "and so is the shell");
        app.focused = FocusedPane::Left;

        app.popup = Popup::Notice { lines: Vec::new() };
        assert!(!app.wants_text_input(), "a notice is answered with one key");
        app.start_rename();
        assert!(app.wants_text_input(), "a rename is text");
    }

    /// The switch is thrown on the way into text and on the way out, and not
    /// otherwise — a helper run on every keystroke would be a subprocess per
    /// keystroke.
    #[test]
    fn the_switch_moves_only_when_the_answer_changes() {
        // It queues onto the one shared worker, so it takes its turn too.
        let _g = SWITCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_state();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_path_buf();
        let mut config = cian_lua::Config::default();
        // No helper: this is about the decision, not about running anything.
        config.ime = Some(cian_lua::ImeOptions { restore: true, ..Default::default() });
        let mut app = App::new(p.clone(), p, config).unwrap();

        assert_eq!(app.ime_on, None, "nothing decided before the first sync");
        app.sync_ime();
        assert_eq!(app.ime_on, Some(false), "driving cian — input method off");
        app.sync_ime();
        assert_eq!(app.ime_on, Some(false), "still off, and nothing to re-run");

        app.mode = Mode::Command;
        app.sync_ime();
        assert_eq!(app.ime_on, Some(true), "the `:` line takes text");
        app.mode = Mode::Normal;
        app.sync_ime();
        assert_eq!(app.ime_on, Some(false), "and back off on the way out");
    }

    /// A fake input method in a file: the helper prints it when read and
    /// writes it when set, which is the shape of macism / im-select /
    /// cian-ime.
    #[cfg(unix)]
    fn fake_helper(dir: &std::path::Path, start: &str) -> (std::path::PathBuf, cian_lua::ImeOptions) {
        let state = dir.join("source");
        std::fs::write(&state, start).unwrap();
        let s = state.display().to_string();
        (
            state.clone(),
            cian_lua::ImeOptions {
                query: Some(format!("cat {s}")),
                set: Some(format!("sh -c 'printf %s \"$1\" > {s}' --")),
                off: Some("ABC".into()),
                restore: true,
                ..Default::default()
            },
        )
    }

    /// Wait for everything queued so far to have run — for the assertions that
    /// a switch did *not* happen, which otherwise pass by being early.
    #[cfg(unix)]
    fn drain(app: &mut App) {
        app.sync_ime();
        let (tx, rx) = std::sync::mpsc::channel();
        queue(
            Job::Restore(tx),
            &cian_lua::ImeOptions { off: Some("x".into()), ..Default::default() },
        );
        let _ = rx.recv_timeout(std::time::Duration::from_secs(10));
    }

    /// Sync, then wait for the worker to have finished what that queued.
    ///
    /// Waiting for the *file* to change instead would pass by being early:
    /// when the job is about to write the value that is already there, there
    /// is nothing to wait for and the assertion runs before the job does.
    #[cfg(unix)]
    fn sync_and_settle(app: &mut App, state: &std::path::Path) -> String {
        app.sync_ime();
        drain(app);
        std::fs::read_to_string(state).unwrap_or_default()
    }

    /// The heart of it: cian must not decide that typing means Japanese. It
    /// remembers what the user was actually typing with and puts *that* back —
    /// including when they turned the IME off themselves mid-edit.
    #[cfg(unix)]
    #[test]
    fn typing_gets_back_the_source_the_user_left_it_in() {
        let _g = SWITCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_state();
        let dir = tempfile::tempdir().unwrap();
        let (state, ime) = fake_helper(dir.path(), "Japanese");
        let mut config = cian_lua::Config::default();
        config.ime = Some(ime);
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, config).unwrap();

        // Startup: the user had Japanese on, so that is remembered, and the
        // keyboard goes off for commands.
        assert_eq!(sync_and_settle(&mut app, &state), "ABC", "commands are always off");
        assert_eq!(
            remembered_source().as_deref(),
            Some("Japanese"),
            "…and it kept what was there",
        );

        // Typing: back to what the user had.
        app.mode = Mode::Command;
        assert_eq!(sync_and_settle(&mut app, &state), "Japanese", "typing restores it");

        // The user turns the IME off themselves, mid-prompt…
        std::fs::write(&state, "ABC").unwrap();
        app.mode = Mode::Normal;
        sync_and_settle(&mut app, &state);
        assert_eq!(remembered_source().as_deref(), Some("ABC"), "cian noticed the choice");

        // …so the next prompt opens off, not Japanese.
        app.mode = Mode::Command;
        assert_eq!(
            sync_and_settle(&mut app, &state),
            "ABC",
            "cian does not decide that text means Japanese",
        );

        // And the way out hands back what the user was typing with.
        std::fs::write(&state, "Japanese").unwrap();
        app.mode = Mode::Normal;
        assert_eq!(sync_and_settle(&mut app, &state), "ABC");
        app.release_ime();
        assert_eq!(
            std::fs::read_to_string(&state).unwrap(),
            "Japanese",
            "the keyboard is handed back as it was found",
        );
    }

    /// With nothing remembered, text opens with the input method off — the
    /// user turns it on if they want it, and that choice is what sticks. This
    /// is also what a helper that cannot be *read* degrades to.
    #[cfg(unix)]
    #[test]
    fn nothing_remembered_means_text_opens_off() {
        let _g = SWITCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_state();
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("source");
        std::fs::write(&state, "ABC").unwrap();
        let s = state.display().to_string();
        let mut config = cian_lua::Config::default();
        config.ime = Some(cian_lua::ImeOptions {
            // No query command at all: cian can set, but never read.
            set: Some(format!("sh -c 'printf %s \"$1\" > {s}' --")),
            off: Some("ABC".into()),
            restore: true,
            ..Default::default()
        });
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, config).unwrap();

        sync_and_settle(&mut app, &state);
        app.mode = Mode::Command;
        assert_eq!(
            sync_and_settle(&mut app, &state),
            "ABC",
            "nothing to restore, so nothing is restored",
        );
        assert!(remembered_source().is_none());
    }

    /// A helper that is missing, renamed or broken must say so. Before this,
    /// the only symptom of a bad `cian.ime{}` was that nothing happened — the
    /// switch failed on a worker thread with no one listening.
    #[cfg(unix)]
    #[test]
    fn a_failed_switch_is_reported_once() {
        let _g = SWITCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_state();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_path_buf();
        let mut config = cian_lua::Config::default();
        config.ime = Some(cian_lua::ImeOptions {
            helper: Some("exit 3 #".into()),
            off: Some("ABC".into()),
            restore: false,
            ..Default::default()
        });
        let mut app = App::new(p.clone(), p, config).unwrap();
        app.sync_ime();
        let mut said = None;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.sync_ime();
            if let Some(m) = app.message.clone() {
                said = Some(m);
                break;
            }
        }
        let said = said.expect("a failed switch is reported");
        assert!(said.contains("ime:") && said.contains("exit 3"), "{said}");
        // …and only once: the next sync does not repeat it.
        app.message = None;
        app.sync_ime();
        assert!(app.message.is_none(), "a failure is said once, not every frame");
    }

    /// One field is the whole setup for the helpers people actually have.
    #[test]
    fn one_helper_field_gives_both_commands() {
        let c = cian_lua::ImeOptions { helper: Some("macism".into()), ..Default::default() };
        assert_eq!(c.query_cmd().as_deref(), Some("macism"));
        assert_eq!(c.set_cmd("x.y").as_deref(), Some("macism x.y"));
        // An explicit template places the id wherever it belongs.
        let c =
            cian_lua::ImeOptions { set: Some("switch --to {} --now".into()), ..Default::default() };
        assert_eq!(c.set_cmd("x.y").as_deref(), Some("switch --to x.y --now"));
        // And with neither, there is nothing to run.
        assert!(cian_lua::ImeOptions::default().set_cmd("x").is_none());
    }
}

