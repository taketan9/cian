//! `Ctrl+-` / `Ctrl++` — the terminal's font size, one step at a time.
//!
//! A program inside a terminal cannot resize that terminal's font. The font
//! belongs to the emulator, there is no portable escape sequence for it, and
//! the two terminals on this desk answer to different things: kitty has
//! remote control, iTerm2 has profiles and an AppleScript-driven keystroke,
//! WezTerm has its own key bindings and no CLI for it.
//!
//! So cian does the part that *is* its own: it owns the keys, it keeps the
//! level, it puts the level back on the next launch — and it runs whatever
//! command the terminal understands, named once in `init.lua`:
//!
//! ```lua
//! cian.font{ set = "kitten @ set-font-size {}", start = 13 }   -- kitty
//! ```
//!
//! `set` is preferred over `bigger`/`smaller` because it is the only form
//! that can be *restored*: a relative step has nothing to say at startup
//! except "do it again N times", which is a guess about where it started.

use super::*;

impl App {
    /// The saved level, or where the config says to start.
    pub(crate) fn font_level_start(cfg: &cian_lua::FontOptions) -> i64 {
        crate::state_get("font_level")
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|v| *v >= cfg.min && *v <= cfg.max)
            .unwrap_or(cfg.start)
    }

    /// Put the remembered size back, once, at startup. Only for the absolute
    /// form — stepping N times from an unknown starting point would be a
    /// guess dressed up as a restore.
    pub(crate) fn apply_saved_font(&mut self) {
        let Some(cfg) = self.config.font.clone() else { return };
        let level = Self::font_level_start(&cfg);
        self.font_level = level;
        if cfg.set.is_some() && Some(level) != Some(cfg.start) {
            self.run_font_command(&cfg, level, 0);
        }
    }

    /// `Ctrl++` / `Ctrl+-`. `step` is +1 or -1.
    pub(crate) fn font_step(&mut self, step: i64) {
        let Some(cfg) = self.config.font.clone() else {
            self.message = Some(
                tr(
                    self.lang,
                    "font size is the terminal's own — see cian.font{} in init.lua",
                    "フォントサイズは端末側の設定です — init.lua の cian.font{} を参照",
                )
                .into(),
            );
            return;
        };
        let want = (self.font_level + step).clamp(cfg.min, cfg.max);
        if want == self.font_level {
            self.message = Some(if step > 0 {
                tr(self.lang, "already the largest", "これ以上大きくできません").into()
            } else {
                tr(self.lang, "already the smallest", "これ以上小さくできません").into()
            });
            return;
        }
        self.font_level = want;
        let _ = crate::state_set("font_level", &want.to_string());
        self.run_font_command(&cfg, want, step);
    }

    /// Run whichever form the config gave, off the UI thread.
    fn run_font_command(&mut self, cfg: &cian_lua::FontOptions, level: i64, step: i64) {
        let cmd = match (&cfg.set, step > 0) {
            (Some(t), _) if t.contains("{}") => t.replace("{}", &level.to_string()),
            (Some(t), _) => format!("{t} {level}"),
            (None, true) => match &cfg.bigger {
                Some(c) => c.clone(),
                None => return,
            },
            (None, false) => match &cfg.smaller {
                Some(c) => c.clone(),
                None => return,
            },
        };
        self.message = Some(if self.lang == Lang::Ja {
            format!("フォント {level}")
        } else {
            format!("font {level}")
        });
        std::thread::spawn(move || {
            let out = shell_command(&cmd).output();
            // A helper that is not there is worth knowing about; it is the
            // whole feature failing silently otherwise.
            if let Ok(o) = out {
                if !o.status.success() {
                    cian_core::log::log(&format!(
                        "font: {cmd} — {}",
                        String::from_utf8_lossy(&o.stderr).trim()
                    ));
                }
            }
        });
    }
}

/// A command line as the platform's shell would run it.
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

    fn cfg() -> cian_lua::FontOptions {
        cian_lua::FontOptions {
            set: Some("echo {}".into()),
            start: 13,
            min: 10,
            max: 16,
            ..Default::default()
        }
    }

    #[test]
    fn stepping_stays_inside_the_range_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_path_buf();
        let mut config = cian_lua::Config::default();
        config.font = Some(cfg());
        let mut app = App::new(p.clone(), p, config).unwrap();
        app.font_level = 15;
        app.font_step(1);
        assert_eq!(app.font_level, 16);
        app.font_step(1);
        assert_eq!(app.font_level, 16, "the top is the top");
        assert!(app.message.as_deref().is_some_and(|m| !m.is_empty()), "and it says so");
        for _ in 0..10 {
            app.font_step(-1);
        }
        assert_eq!(app.font_level, 10, "…and the bottom is the bottom");
    }

    /// With nothing configured the keys say where the setting actually lives,
    /// rather than doing nothing at all.
    #[test]
    fn unconfigured_says_where_the_setting_is() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_path_buf();
        let mut app = App::new(p.clone(), p, cian_lua::Config::default()).unwrap();
        app.font_step(1);
        assert!(
            app.message.as_deref().is_some_and(|m| m.contains("cian.font")),
            "{:?}",
            app.message
        );
    }
}
