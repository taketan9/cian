//! Running a layout macro: build a set of shell panes over several ticks.
//!
//! Splits spawn PTYs on a background thread (see [`crate::panes`]), so a macro
//! cannot build its layout in one shot — it would race the spawns. Instead it
//! runs as a tiny state machine advanced from the main loop once per tick:
//! wait for the shell to be idle, apply the current pane (colour, log, the
//! command to run), then split off the next one and wait again. The result is
//! that "split, ssh here, split, ssh there, start logging" collapses to one key.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cian_lua::macros::{Macro, PaneStep, Split, Step};

use crate::{tr, App, FocusedPane, Popup, SplitDir};

/// Nobody wants a mistyped `wait = 100000` to wedge cian; cap any single pause
/// or prompt-wait here.
const MAX_WAIT_SECS: f64 = 600.0;

/// A macro in the middle of building itself.
pub(crate) struct MacroRun {
    /// For the "macro done" message.
    name: String,
    /// Panes not yet started. The very first applies to the current pane; each
    /// later one is split off the previous.
    queue: VecDeque<PaneStep>,
    /// A pane that now exists (its spawn has landed) and still needs its colour,
    /// log and scripted steps applied.
    apply: Option<PaneStep>,
    /// True until the first pane has been consumed (it is the shell pane you are
    /// on, so it is applied without a split).
    first: bool,
    /// The current pane's remaining scripted steps (send / wait / expect).
    steps: VecDeque<Step>,
    /// When a timed `wait` ends, if one is in progress.
    wait_until: Option<Instant>,
    /// An `expect` in progress: the (lower-cased) text to watch for and the
    /// deadline after which we give up and move on.
    expect: Option<(String, Instant)>,
    /// Turn on input broadcast once the whole layout is built.
    sync: bool,
    /// The shell-leaf id of each pane created so far, in `panes` order, so a
    /// later pane's `from = N` can split off pane N rather than the previous.
    leaf_ids: Vec<usize>,
}

/// Load macros. The rule — `macro.lua` then `macro/*.lua`, portable-first —
/// is `cian_lua::macros::load_all`, shared with the window build, which used
/// to read only the first of the two and showed an empty launcher to anybody
/// who had split their macros up.
pub(crate) fn load_macros() -> (Vec<Macro>, Option<String>) {
    cian_lua::macros::load_all()
}

impl App {
    /// Open the macro launcher, or explain why there is nothing to launch.
    pub(crate) fn start_macros(&mut self) {
        if self.macros.is_empty() {
            self.message = Some(match &self.macro_error {
                Some(e) => e.clone(),
                None => tr(
                    self.lang,
                    "no macros. define them in macro.lua",
                    "マクロがありません。macro.lua で定義してください",
                )
                .to_string(),
            });
            return;
        }
        self.open_popup(Popup::Macros { cursor: 0, names: self.macro_names() });
    }

    /// Begin running macro `idx` from the loaded set. The build proceeds in
    /// [`App::tick_macro`].
    pub(crate) fn run_macro(&mut self, idx: usize) {
        let Some(m) = self.macros.get(idx).cloned() else { return };
        self.popup = Popup::None;
        self.begin_macro(&m);
    }

    /// Start running macro named `name` (from the loaded set). Returns false if
    /// there is no such macro — used by the `--macro-name` startup option.
    pub(crate) fn start_macro_by_name(&mut self, name: &str) -> bool {
        let Some(m) = self.macros.iter().find(|m| m.name == name).cloned() else {
            return false;
        };
        self.begin_macro(&m);
        true
    }

    /// Kick off building `m`'s layout, focusing the shell it builds into.
    pub(crate) fn begin_macro(&mut self, m: &Macro) {
        // A script macro is not a layout — it automates file operations and runs
        // synchronously right here, then the panes are refreshed.
        if m.is_script() {
            self.run_script_macro(m);
            return;
        }
        self.macro_run = Some(MacroRun {
            name: m.name.clone(),
            queue: m.panes.iter().cloned().collect(),
            apply: None,
            first: true,
            steps: VecDeque::new(),
            wait_until: None,
            expect: None,
            sync: m.sync,
            leaf_ids: Vec::new(),
        });
        self.focus(FocusedPane::Shell);
        // Maximize the shell panel so a grid has the whole window.
        if m.zoom && !self.zoomed {
            self.toggle_zoom();
        }
        self.message = Some(tr(self.lang, "running macro: ", "マクロ実行中: ").to_string() + &m.name);
    }

    /// Run a script macro synchronously: snapshot the panes, invoke its `run`
    /// function via `cian_lua::macro_script`, then apply the result — refresh the
    /// listings the ops touched and surface any messages or error.
    pub(crate) fn run_script_macro(&mut self, m: &Macro) {
        let Some(src) = m.script.clone() else { return };
        let (Some(dir), Some(other)) =
            (self.cwd(), self.opposite_pane_cwd())
        else {
            self.message = Some(
                tr(self.lang, "no active pane for the macro", "マクロ対象のペインがありません").into(),
            );
            return;
        };
        let (marked, cursor) = match self.active_pane() {
            Some(p) => (
                p.target_paths(),
                p.selected().filter(|e| !e.is_parent).map(|e| e.path.clone()),
            ),
            None => (Vec::new(), None),
        };
        let ctx = cian_lua::macro_script::Ctx { dir, other, marked, cursor };
        let outcome = cian_lua::macro_script::run(&src, &m.name, ctx);

        if outcome.touched {
            self.reload_both();
            self.invalidate_git();
        }
        self.popup = Popup::None;
        if let Some(err) = outcome.error {
            let mut lines = vec![format!("{}: {}", m.name, err)];
            if !outcome.messages.is_empty() {
                lines.push(String::new());
                lines.extend(outcome.messages);
            }
            self.open_popup(Popup::Notice { lines });
        } else if outcome.messages.len() > 1 {
            self.open_popup(Popup::Notice { lines: outcome.messages });
        } else if let Some(one) = outcome.messages.into_iter().next() {
            self.message = Some(one);
        } else {
            self.message =
                Some(tr(self.lang, "macro done: ", "マクロ完了: ").to_string() + &m.name);
        }
    }

    /// Advance a running macro. Returns true if anything changed (so the caller
    /// repaints). Called each tick from the main loop. It processes one thing
    /// per tick — apply a landed pane, run/observe one scripted step, or start
    /// the next pane — so waits and prompt-waits are honoured without blocking.
    pub(crate) fn tick_macro(&mut self) -> bool {
        if self.macro_run.is_none() {
            return false;
        }
        // Wait for calm: a spawn in flight means the previous split has not
        // landed yet, so its pane is not there to apply to or split from.
        if self.shell.busy() {
            return false;
        }
        let cwd = self.shell_cwd();
        self.shell.ensure(&cwd);
        if self.shell.active_session().is_none() {
            return true; // the first shell is still starting; try again next tick
        }

        // Own the runner for the tick so we can freely call other `self` methods.
        let mut run = self.macro_run.take().unwrap();

        // 1. A pane whose spawn just landed: colour/log it and load its steps.
        if let Some(step) = run.apply.take() {
            self.apply_pane(&step, &mut run);
        }

        // 2. A timed pause still in progress.
        if let Some(t) = run.wait_until {
            if Instant::now() < t {
                self.macro_run = Some(run);
                return true;
            }
            run.wait_until = None;
        }

        // 3. Waiting for a prompt/text to appear.
        if let Some((needle, deadline)) = run.expect.clone() {
            if self.shell_screen_has(&needle) {
                run.expect = None;
            } else if Instant::now() >= deadline {
                run.expect = None;
                self.message = Some(if self.lang == crate::theme::Lang::Ja {
                    format!("マクロ: 「{needle}」を待ちましたが現れませんでした")
                } else {
                    format!("macro: gave up waiting for “{needle}”")
                });
            } else {
                self.macro_run = Some(run);
                return true;
            }
        }

        // 4. Run the current pane's next scripted step.
        if let Some(step) = run.steps.pop_front() {
            if cian_core::log::enabled() {
                cian_core::log::log(&format!("macro: step {:?}", step));
            }
            match step {
                Step::Send(line) => self.type_line_in_active(&line),
                Step::Wait(secs) => {
                    run.wait_until = Some(Instant::now() + secs_to_dur(secs));
                }
                Step::Expect { text, timeout } => {
                    run.expect = Some((text.to_lowercase(), Instant::now() + secs_to_dur(timeout)));
                }
            }
            self.macro_run = Some(run);
            return true;
        }

        // 5. Current pane finished; start the next one, or we are done.
        match run.queue.pop_front() {
            Some(next) => {
                let first = run.first;
                run.first = false;
                if !first {
                    // `from = N` (1-based) splits pane N rather than the previous
                    // one — the key to a real grid. Otherwise split off the pane
                    // built just before. Target the leaf explicitly so the async
                    // split lands on it even though `active` may move meanwhile.
                    let target = next
                        .from
                        .and_then(|n| run.leaf_ids.get(n.saturating_sub(1)))
                        .or_else(|| run.leaf_ids.last())
                        .copied();
                    let dir = match next.dir {
                        Split::Right => SplitDir::LeftRight,
                        Split::Down => SplitDir::TopBottom,
                    };
                    let ratio = next.ratio.unwrap_or(50);
                    if cian_core::log::enabled() {
                        cian_core::log::log(&format!(
                            "macro: split from={:?} target_leaf={:?} dir={:?} ratio={} leaf_ids={:?}",
                            next.from, target, dir, ratio, run.leaf_ids
                        ));
                    }
                    match target {
                        Some(leaf) => self.shell.split_leaf(&cwd, leaf, dir, ratio),
                        None => self.shell.split_active(&cwd, dir),
                    }
                }
                run.apply = Some(next);
                self.macro_run = Some(run);
            }
            None => {
                // The layout is built. Turn on input broadcast if asked, so the
                // same keystrokes now reach every pane the macro made.
                if run.sync {
                    self.shell.set_broadcast(true);
                }
                self.message =
                    Some(tr(self.lang, "macro done: ", "マクロ完了: ").to_string() + &run.name);
                // Drop `run`: the macro is finished.
            }
        }
        true
    }

    /// Colour and log the now-active pane, remember its leaf id (so a later
    /// `from` can target it), and load its command + scripted steps.
    fn apply_pane(&mut self, step: &PaneStep, run: &mut MacroRun) {
        // Record this pane's leaf id in creation order.
        if let Some(id) = self.shell.active_leaf_id() {
            run.leaf_ids.push(id);
        }
        if cian_core::log::enabled() {
            cian_core::log::log(&format!(
                "macro: applied pane (leaf {:?}); leaf_ids now {:?}",
                self.shell.active_leaf_id(),
                run.leaf_ids
            ));
        }
        if let Some(spec) = &step.bg {
            if let Some(c) = crate::resolve_bg(spec) {
                self.shell.set_active_pane_bg(Some(c));
            }
        }
        if let Some(dir) = &step.log {
            self.start_session_log(dir);
        }
        run.steps.clear();
        if let Some(cmd) = &step.cmd {
            run.steps.push_back(Step::Send(cmd.clone()));
        }
        run.steps.extend(step.steps.iter().cloned());
    }

    /// Does the active shell pane's visible screen contain `needle` (already
    /// lower-cased)? Used by `expect` to wait for a prompt.
    fn shell_screen_has(&self, needle: &str) -> bool {
        self.shell
            .active_session()
            .and_then(|s| s.parser().lock().ok().map(|p| p.screen().contents()))
            .map(|c| c.to_lowercase().contains(needle))
            .unwrap_or(false)
    }

    /// Type one line into the active shell pane and press Enter. Enter is a
    /// carriage return (`\r`) — what a real keypress sends — so it also submits
    /// a password at a getpass prompt, where a bare `\n` may not.
    fn type_line_in_active(&mut self, line: &str) {
        if let Some(s) = self.shell.active_session_mut() {
            let mut bytes = line.as_bytes().to_vec();
            bytes.push(b'\r');
            s.write_input(&bytes);
        }
    }

    /// The macros' names, for the launcher list.
    pub(crate) fn macro_names(&self) -> Vec<String> {
        // A small tag tells the two kinds apart in the launcher: § automates
        // file operations, ▦ builds a shell layout. Display only — selection is
        // by index and `--macro-name` matches the bare name.
        self.macros
            .iter()
            .map(|m| format!("{} {}", if m.is_script() { "§" } else { "▦" }, m.name))
            .collect()
    }
}

/// A non-negative, capped duration from a seconds value.
fn secs_to_dur(secs: f64) -> Duration {
    Duration::from_secs_f64(secs.clamp(0.0, MAX_WAIT_SECS))
}
