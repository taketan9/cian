//! AI-backed actions on the App: chat, NL→command, junk detection, structure
//! and rename suggestions, semantic search, commit-message drafting, error
//! explanation, file summary — plus the duplicate-file scan that shares the
//! same review-and-approve shape. Split out of lib.rs as an `impl App` block;
//! it reaches the rest of App through `use super::*`.

use super::*;

/// A stored chat conversation for `ai_history.json` — a transcript, the backend
/// it spoke to (so a reopened conversation still routes follow-ups) and how the
/// window looked, so a reopened conversation keeps the title it had.
/// `skin` is absent in files written before it existed; those fall back to the
/// mode's default look.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct StoredChat {
    mode: ChatMode,
    #[serde(default)]
    skin: Option<ChatSkin>,
    log: Vec<ChatMsg>,
}

/// Load the saved chat history (portable-aware), newest first. Empty if there
/// is none or it is unreadable.
pub(crate) fn restore_ai_history() -> Vec<StoredChat> {
    let Some(path) = cian_lua::config_read_path("ai_history.json").filter(|p| p.exists()) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    serde_json::from_str::<Vec<StoredChat>>(&text).unwrap_or_default()
}

impl StoredChat {
    pub(crate) fn new(mode: ChatMode, skin: ChatSkin, log: Vec<ChatMsg>) -> Self {
        StoredChat { mode, skin: Some(skin), log }
    }
    pub(crate) fn mode(&self) -> ChatMode {
        self.mode
    }
    pub(crate) fn log(&self) -> &[ChatMsg] {
        &self.log
    }
    /// How the window looked, or the mode's default for entries saved before
    /// skins existed.
    pub(crate) fn skin(&self) -> ChatSkin {
        self.skin.clone().unwrap_or_else(|| ChatSkin::of(self.mode))
    }
}

impl PartialEq for StoredChat {
    /// Two snapshots are the same conversation when the backend and transcript
    /// match; the window dressing does not make it a different chat.
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode && self.log == other.log
    }
}

/// Read up to the last `max_bytes` of a file as text. Logs grow at the end, so
/// the tail is the part worth sending; a partial first line (from cutting mid
/// file) is dropped so the model does not read a fragment as a whole entry.
fn read_tail(path: &std::path::Path, max_bytes: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    let _ = f.seek(SeekFrom::Start(start));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let s = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        if let Some(nl) = s.find('\n') {
            return s[nl + 1..].to_string();
        }
    }
    s
}

/// Who the assistant is, prepended to every conversational (chat) system prompt.
/// This is the local model configured in `cian.ai`, so it answers
/// to "AI - simple" — the same name the menu and the transcript use. It refers
/// to itself in the first person as「私」.
fn persona() -> &'static str {
    "あなたはこの二画面ファイラ／ターミナル「cian」に組み込まれた AI アシスタントです。\
     あなたの名前は「AI - simple」。自分を指すときは常に一人称「私」を使い、\
     名前を尋ねられたら「私は cian の AI - simple です」と名乗ってください。\
     (Your name is \"AI - simple\", the local model built into cian; \
     always refer to yourself as「私」.)"
}

/// The viewer's selection as text, or the whole file when nothing is selected
/// — the same rule the copy key follows.
fn viewer_selection_text(p: &Popup) -> Option<String> {
    let Popup::Viewer { view, line, col, visual, anchor, .. } = p else { return None };
    let lines = &view.lines;
    if lines.is_empty() {
        return None;
    }
    let text = match visual {
        None => lines.join("\n"),
        Some(ViewVisual::Line) => {
            let (a, b) = (anchor.0.min(*line), anchor.0.max(*line).min(lines.len() - 1));
            lines[a..=b].join("\n")
        }
        Some(ViewVisual::Char) => {
            let (s, e) = order_pos((anchor.0, anchor.1), (*line, *col));
            viewer_charwise(lines, s, e)
        }
        Some(ViewVisual::Block) => {
            let b = cian_core::textops::Block::between(lines, *anchor, (*line, *col));
            cian_core::textops::block_text(lines, b).join("\n")
        }
    };
    Some(text).filter(|t| !t.trim().is_empty())
}

impl App {
    /// Is the AI helper configured and working? Returns the cached result of the
    /// background probe (see [`Self::spawn_ai_probe`]); `false` until the probe
    /// lands, so this NEVER blocks — the python `--check` can take seconds and
    /// must not freeze the UI (e.g. when building the right-click menu).
    pub(crate) fn ai_ready(&mut self) -> bool {
        let Some(cfg) = self.ai.as_ref() else { return false };
        // No endpoint, no AI. There is no default for it any more — a site's
        // gateway address is a site's business — so this is the ordinary case
        // for anyone who has not set one, and it has to be a plain "not
        // configured" rather than a connection that fails obscurely later.
        // …except in mock mode, which answers offline and has nothing to
        // reach for.
        if cfg.auth_mode != "mock"
            && cfg.endpoint.trim().is_empty()
            && cfg.api_base_url.trim().is_empty()
        {
            return false;
        }
        self.ai_ready.unwrap_or(false)
    }

    /// Kick off the AI availability check on a worker thread. Called at startup
    /// and after `:reload`; the result is installed by [`Self::poll_ai_probe`].
    pub(crate) fn spawn_ai_probe(&mut self) {
        self.ai_ready = None;
        self.ai_probe = None;
        let Some(cfg) = self.ai.clone() else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(cian_ai::available(&cfg));
        });
        self.ai_probe = Some(rx);
    }

    /// Install the AI probe's result once it lands. Returns true if it changed.
    pub(crate) fn poll_ai_probe(&mut self) -> bool {
        let Some(rx) = &self.ai_probe else { return false };
        match rx.try_recv() {
            Ok(ready) => {
                self.ai_ready = Some(ready);
                self.ai_probe = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.ai_ready = Some(false);
                self.ai_probe = None;
                true
            }
        }
    }

    /// True during the brief startup window — while the AI probe is still
    /// running, or for a short minimum — so a "starting up" splash can show.
    /// Capped so it can never linger.
    pub(crate) fn is_starting_up(&self) -> bool {
        let e = self.startup_at.elapsed();
        e < std::time::Duration::from_secs(6)
            && (self.ai_probe.is_some() || e < std::time::Duration::from_millis(1200))
    }

    /// Is an endpoint configured at all? Says what to add if not.
    ///
    /// Eleven entry points opened with these eight lines written out.
    fn ai_configured(&mut self) -> bool {
        if self.ai.is_some() {
            return true;
        }
        self.message = Some(tr(
            self.lang,
            "AI not configured — add cian.ai{ endpoint = \"…\" } to init.lua",
            "AI が未設定です — init.lua に cian.ai{ endpoint = \"…\" } を追加してください",
        )
        .into());
        false
    }

    /// Configured *and* answering. The full guard every prompt opens with.
    fn ai_available(&mut self) -> bool {
        if !self.ai_configured() {
            return false;
        }
        if !self.ai_ready() {
            self.message = Some(tr(self.lang, "AI unavailable (python, packages, or sign-in)", "AI を利用できません（python・パッケージ・サインインのいずれか）").into());
            return false;
        }
        true
    }

    pub(crate) fn open_ai_chat(&mut self) {
        if !self.ai_configured() {
            return;
        }
        if !self.ai_ready() {
            self.message =
                Some("AI unavailable (python, packages, or sign-in) — feature hidden".into());
            return;
        }
        self.new_ai_chat();
    }

    /// A fresh, empty AI - simple chat — the menu's "Chat", and `Ctrl+N` from
    /// inside a chat.
    pub(crate) fn new_ai_chat(&mut self) {
        let skin = ChatSkin::simple(tr(self.lang, "Chat", "チャット"));
        self.start_ai_chat_as(ChatMode::Ai, skin, Vec::new(), false);
    }

    /// Summarise the file open in the F3 viewer. Unlike the metadata-only
    /// features, this sends the file's TEXT to the model (a content-egress
    /// action), so it is gated behind an explicit key in the viewer. The reply
    /// opens in the AI chat, where it can be read, selected and copied.
    pub(crate) fn summarize_viewer(&mut self) {
        if !self.ai_configured() {
            return;
        }
        // Pull the decoded text and a name out of the viewer.
        let (name, content) = if let Popup::Viewer { title, view, .. } = &self.popup {
            (title.clone(), view.lines.join("\n"))
        } else {
            return;
        };
        if content.trim().is_empty() {
            self.message = Some(tr(self.lang, "nothing to summarise", "要約する対象がありません").into());
            return;
        }
        if !self.ai_ready() {
            self.message = Some(tr(self.lang, "AI unavailable (python, packages, or sign-in)", "AI を利用できません（python・パッケージ・サインインのいずれか）").into());
            return;
        }
        // Bound the payload: a summary rarely needs the whole of a large file,
        // and an unbounded body would blow the token budget.
        let body = truncate_text_for_ai(&content, 24_000);
        let system = "You summarise a file's contents for a developer. Give a \
             short, plain-text summary: what it is, its purpose, and the key \
             points or structure. Be concise; no preamble, no markdown headings."
            .to_string();
        // Open the chat with the request shown, so the reply lands in a place
        // that can be scrolled, selected and copied — and followed up in.
        self.start_ai_chat_as(
            ChatMode::Ai,
            ChatSkin::simple(tr(self.lang, "Summarize this file", "このファイルを要約")),
            vec![ChatMsg { user: true, text: format!("Summarise {}", name) }],
            true,
        );
        self.ai_request(AiPurpose::Chat, system, body);
    }

    /// Explain the error visible in the active shell pane. Sends the visible
    /// terminal text (a content-egress action, hence an explicit command/menu
    /// item), and opens the reply in the AI chat.
    pub(crate) fn explain_shell_error(&mut self) {
        if !self.ai_configured() {
            return;
        }
        // Grab the visible screen of the active shell pane.
        let screen = self.shell.active_session().and_then(|s| {
            s.parser().lock().ok().map(|p| p.screen().contents())
        });
        let Some(screen) = screen else {
            self.message = Some(tr(self.lang, "no shell here", "ここにシェルがありません").into());
            return;
        };
        // Collapse the trailing blank rows a terminal screen is padded with.
        let text = screen.trim_end().to_string();
        if text.is_empty() {
            self.message = Some(tr(self.lang, "nothing on the shell to explain", "シェルに説明する内容がありません").into());
            return;
        }
        if !self.ai_ready() {
            self.message = Some(tr(self.lang, "AI unavailable (python, packages, or sign-in)", "AI を利用できません（python・パッケージ・サインインのいずれか）").into());
            return;
        }
        let body = truncate_text_for_ai(&text, 8_000);
        let system = cian_core::aiprompt::shell_error(cian_core::aiprompt::os_name());
        self.start_ai_chat_as(
            ChatMode::Ai,
            ChatSkin::simple(tr(self.lang, "Explain the last error", "直近のエラーを説明")),
            vec![ChatMsg { user: true, text: "Explain the last error".into() }],
            true,
        );
        self.ai_request(AiPurpose::Chat, system, body);
    }

    /// Explain the diff currently on screen (a two-file diff or a folder
    /// compare): what changed and the likely intent, grouped rather than line by
    /// line. Reuses the same text the copy/save actions produce.
    pub(crate) fn explain_diff(&mut self) {
        if !self.ai_configured() {
            return;
        }
        let Some(text) = self.diff_as_text() else {
            self.message = Some(tr(self.lang, "no diff to explain", "説明する差分がありません").into());
            return;
        };
        if !self.ai_ready() {
            self.message = Some(tr(self.lang, "AI unavailable (python, packages, or sign-in)", "AI を利用できません（python・パッケージ・サインインのいずれか）").into());
            return;
        }
        let body = truncate_diff_for_ai(&text, 8_000);
        let system = "You explain a diff between two files (or two folders) for a \
             developer. Summarize WHAT changed and, where you can tell, the \
             likely intent — grouped by theme, not line by line. Call out \
             anything risky: a removed check, a changed default, a probable \
             typo. Be concise; plain text, no markdown headings."
            .to_string();
        self.start_ai_chat_as(
            ChatMode::Ai,
            ChatSkin::simple(tr(self.lang, "Explain this diff", "この差分を説明")),
            vec![ChatMsg { user: true, text: "Explain this diff".into() }],
            true,
        );
        self.ai_request(AiPurpose::Chat, system, body);
    }

    /// Triage the selected file as a log: from its tail, surface the errors that
    /// matter, a rough timeline, and the most likely cause / next check. Aimed
    /// at the RHEL/AIX/Oracle logs this is built for.
    pub(crate) fn triage_log(&mut self) {
        if !self.ai_configured() {
            return;
        }
        let picked = self
            .active_pane()
            .and_then(|p| p.selected())
            .filter(|e| !e.is_dir && !e.is_parent)
            .map(|e| (e.path.clone(), e.name.clone()));
        let Some((path, name)) = picked else {
            self.message = Some(tr(self.lang, "select a log file to triage", "調査するログファイルを選んでください").into());
            return;
        };
        if !self.ai_ready() {
            self.message = Some(tr(self.lang, "AI unavailable (python, packages, or sign-in)", "AI を利用できません（python・パッケージ・サインインのいずれか）").into());
            return;
        }
        // A log's meaning is at its end — read the tail, not the head.
        let tail = read_tail(&path, 16_000);
        if tail.trim().is_empty() {
            self.message = Some(tr(self.lang, "that file is empty", "そのファイルは空です").into());
            return;
        }
        let system = cian_core::aiprompt::LOG.to_string();
        self.start_ai_chat_as(
            ChatMode::Ai,
            ChatSkin::simple(tr(self.lang, "Triage this log", "このログを診断")),
            vec![ChatMsg { user: true, text: format!("Triage the log: {}", name) }],
            true,
        );
        self.ai_request(AiPurpose::Chat, system, tail);
    }

    /// Ask the local model about what is selected in the viewer — or about the
    /// whole file when nothing is.
    ///
    /// Three questions, because they are the three a file open in front of you
    /// raises: is this written well, what does this command do, and what is
    /// wrong with this code. The viewer steps aside rather than closing, and
    /// comes back when the chat does — the question is *about* the file, and
    /// the file may have unsaved edits in it.
    pub(crate) fn ai_over_viewer(&mut self, kind: AiOverText) {
        let Some(text) = self.viewer_return.as_deref().and_then(viewer_selection_text) else {
            self.message = Some(tr(self.lang, "nothing to ask about", "対象がありません").into());
            self.restore_viewer();
            return;
        };
        if !self.ai_ready() {
            self.message = Some(tr(self.lang, "AI unavailable (python, packages, or sign-in)", "AI を利用できません（python・パッケージ・サインインのいずれか）").into());
            self.restore_viewer();
            return;
        }
        // A model has a limit and a selection may be a whole file; the head is
        // the part that says what the thing is.
        let text: String = text.chars().take(16_000).collect();
        let (system, title) = match kind {
            AiOverText::Writing => (
                "You are an editor. Improve the passage below: fix grammar and \
                 typos, tighten wording, and keep the author's voice and \
                 language (answer in the language it is written in). Give the \
                 rewritten text first, then a short list of what you changed \
                 and why. Do not invent facts.",
                tr(self.lang, "Improve this writing", "この文章を推敲"),
            ),
            AiOverText::Command => (
                "You help an operator on RHEL/AIX. If the text below is a shell \
                 command, explain what it does, flag anything destructive, and \
                 suggest a safer or shorter form. If it is a description of a \
                 task, write the command that does it and explain each part. \
                 Plain text, no markdown headings.",
                tr(self.lang, "Explain / write this command", "コマンドを説明・作成"),
            ),
            AiOverText::Code => (
                "You review code. For the excerpt below: point out bugs, \
                 error handling that is missing, and anything that will not do \
                 what it looks like it does — most important first. Then give \
                 the corrected code. Say so plainly if you find nothing wrong.",
                tr(self.lang, "Review and fix this code", "このコードを点検・修正"),
            ),
        };
        self.start_ai_chat_as(
            ChatMode::Ai,
            ChatSkin::simple(title),
            vec![ChatMsg { user: true, text: title.to_string() }],
            true,
        );
        self.ai_request(AiPurpose::Chat, system.to_string(), text);
    }

    /// Does the shell's visible output look like it just ended in an error?
    ///
    /// A heuristic — cian has no shell-integration marks, so it cannot read an
    /// exit code — used only to *offer* an explanation (a hint chip), never to
    /// act. Kept to strong signatures on the last few non-empty lines so routine
    /// output does not keep the nudge lit. Off entirely when AI is unconfigured.
    pub(crate) fn shell_error_detected(&self) -> bool {
        if self.ai.is_none() {
            return false;
        }
        let Some(screen) = self
            .shell
            .active_session()
            .and_then(|s| s.parser().lock().ok().map(|p| p.screen().contents()))
        else {
            return false;
        };
        let tail: String = screen
            .lines()
            .rev()
            .filter(|l| !l.trim().is_empty())
            .take(6)
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        const SIGNS: [&str; 12] = [
            "command not found",
            "no such file or directory",
            "permission denied",
            "traceback (most recent call last)",
            "segmentation fault",
            "fatal:",
            "ora-0",
            "ora-1",
            "is not recognized as",
            "syntax error",
            "connection refused",
            "no such host",
        ];
        SIGNS.iter().any(|s| tail.contains(s))
    }

    /// Precondition facts to feed the model: the `cian.ai_context{...}` facts
    /// from init.lua, plus the connected server's `notes` when the active shell
    /// is on a known SSH host. Empty when nothing is configured.
    pub(crate) fn ai_context_block(&self) -> String {
        let mut facts: Vec<String> = self.config.ai_context.clone();
        // The server the active shell is logged into, matched to a configured
        // host so its recorded OS / middleware / versions can be handed over.
        if let Some(host) = self.shell.active_title().and_then(|t| host_from_title(&t)) {
            for h in &self.config.ssh_hosts {
                if h.host == host || h.name == host {
                    if let Some(notes) = &h.notes {
                        facts.push(format!("The server '{}' ({}): {}", h.name, h.host, notes));
                    }
                }
            }
        }
        if facts.is_empty() {
            return String::new();
        }
        let mut s = String::from("Context about the user's environment you can rely on:\n");
        for f in &facts {
            s.push_str("- ");
            s.push_str(f);
            s.push('\n');
        }
        s
    }

    /// Fire an AI request on a worker thread, tagged with what to do with the
    /// reply. Only one runs at a time.
    pub(crate) fn ai_request(&mut self, purpose: AiPurpose, system: String, user: String) {
        let Some(cfg) = self.ai.clone() else { return };
        // Prepend the user's environment facts so every purpose benefits.
        let context = self.ai_context_block();
        let system = if context.is_empty() {
            system
        } else {
            format!("{}\n{}", context, system)
        };
        // Give the conversational replies a consistent identity. Only for chat:
        // the structured purposes (rename / search / organize / commit) must
        // return parseable output, and a persona instruction would loosen that.
        let system = if matches!(purpose, AiPurpose::Chat) {
            format!("{}\n{}", persona(), system)
        } else {
            system
        };
        if self.ai_job.is_some() {
            self.message = Some(tr(self.lang, "AI is busy", "AI は処理中です").into());
            return;
        }
        // Pasted images ride along with a chat turn only: the structured
        // purposes parse the reply and have no user-visible place to attach one.
        let images: Vec<String> = if matches!(purpose, AiPurpose::Chat) {
            std::mem::take(&mut self.chat_attachments)
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        } else {
            Vec::new()
        };
        // The conversation, for a chat turn only. The structured purposes
        // parse what comes back, and an earlier turn in the request is an
        // earlier turn's shape in the answer.
        let prior = if matches!(purpose, AiPurpose::Chat) {
            std::mem::take(&mut self.chat_prior)
        } else {
            self.chat_prior.clear();
            Vec::new()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = cian_ai::chat_with(&cfg, &system, &prior, &user, &images)
                .map_err(|e| e.to_string());
            let _ = tx.send(r);
        });
        self.ai_job = Some(AiJob { rx, purpose });
    }

    /// Open a chat popup wearing the default look for its backend.
    ///
    /// Only the tests reach for this now — everything in the program opens a
    /// chat with a title of its own through [`start_ai_chat_as`](Self::start_ai_chat_as).
    #[cfg(test)]
    pub(crate) fn start_ai_chat(&mut self, mode: ChatMode, log: Vec<ChatMsg>, pending: bool) {
        self.start_ai_chat_as(mode, ChatSkin::of(mode), log, pending);
    }

    /// Open a chat popup, first tucking the current conversation (if it has an
    /// answer in it) into the history so switching or restarting never loses it.
    /// `skin` names the window and says who is answering — see [`ChatSkin`].
    pub(crate) fn start_ai_chat_as(
        &mut self,
        mode: ChatMode,
        skin: ChatSkin,
        log: Vec<ChatMsg>,
        pending: bool,
    ) {
        self.archive_current_ai_chat();
        // A fresh conversation starts with no attachments and no memory: an
        // image pasted for the old one, or the turns it was made of, must not
        // leak into this one. `chat_prior` can still be set here — a send that
        // found no AI configured, or a busy one, leaves it behind.
        self.chat_attachments.clear();
        self.chat_prior.clear();
        self.open_popup(Popup::AiChat {
            input: String::new(),
            log,
            scroll: usize::MAX,
            pending,
            sel: None,
            mode,
            skin,
        });
    }

    /// Snapshot the open chat into `ai_history` (newest first, deduped) if it
    /// holds at least one answer. A no-op otherwise.
    pub(crate) fn archive_current_ai_chat(&mut self) {
        if let Popup::AiChat { log, mode, skin, .. } = &self.popup {
            if log.iter().any(|m| !m.user) {
                let snap = StoredChat::new(*mode, skin.clone(), log.clone());
                if self.ai_history.first() != Some(&snap) {
                    self.ai_history.insert(0, snap);
                    self.ai_history.truncate(30);
                    self.save_ai_history();
                }
            }
        }
    }

    /// Persist the chat history so it survives a restart. Portable-aware (beside
    /// `init.lua`, or next to the executable). NOTE: this writes the full
    /// conversation text — including RAG answers — to `ai_history.json` in
    /// plaintext; failures are silent.
    pub(crate) fn save_ai_history(&self) {
        let Some(path) = cian_lua::config_write_path("ai_history.json") else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(&self.ai_history) {
            let _ = std::fs::write(path, json);
        }
    }

    /// A one-line title for a stored conversation — its first question.
    pub(crate) fn ai_history_title(log: &[ChatMsg]) -> String {
        log.iter()
            .find(|m| m.user)
            .map(|m| m.text.replace('\n', " "))
            .map(|t| if t.chars().count() > 60 { format!("{}…", t.chars().take(60).collect::<String>()) } else { t })
            .unwrap_or_else(|| "(empty)".to_string())
    }

    /// `Ctrl+R` in the chat: archive the current conversation, then show the
    /// history picker. With nothing to show, say so instead of an empty box.
    pub(crate) fn open_ai_history(&mut self) {
        self.archive_current_ai_chat();
        if self.ai_history.is_empty() {
            self.message =
                Some(tr(self.lang, "no past conversations yet", "過去の会話はまだありません").into());
            return;
        }
        self.open_popup(Popup::AiHistory { cursor: 0 });
    }

    /// Reopen the conversation at `i` as the live chat.
    pub(crate) fn load_ai_conversation(&mut self, i: usize) {
        let Some(c) = self.ai_history.get(i) else { return };
        // Reopened as it was: same backend for follow-ups, same name and colour,
        // so a reopened conversation is shown the way it was.
        self.open_popup(Popup::AiChat {
            input: String::new(),
            log: c.log().to_vec(),
            scroll: usize::MAX,
            pending: false,
            sel: None,
            mode: c.mode(),
            skin: c.skin(),
        });
    }

    /// Forget the stored conversation at `i`.
    pub(crate) fn delete_ai_conversation(&mut self, i: usize) {
        if i < self.ai_history.len() {
            self.ai_history.remove(i);
            self.save_ai_history();
        }
        if self.ai_history.is_empty() {
            self.popup = Popup::None;
        }
    }

    /// Add to the answer being written in the chat, or start one.
    pub(crate) fn append_ai_answer(&mut self, text: &str) {
        if let Popup::AiChat { log, scroll, .. } = &mut self.popup {
            match log.last_mut() {
                Some(m) if !m.user => m.text.push_str(text),
                _ => log.push(ChatMsg { user: false, text: text.to_string() }),
            }
            *scroll = usize::MAX;
        }
    }

    /// First-Esc behaviour in the chat: stop a running answer and leave the
    /// chat open. Returns true if it cancelled something; false means nothing
    /// was in flight (so Esc closes).
    pub(crate) fn cancel_ai_pending(&mut self) -> bool {
        if self.ai_job.is_some() {
            // The python worker may keep running, but stop waiting on it.
            self.ai_job = None;
            self.append_ai_answer(&format!("\n⚠ {}", tr(self.lang, "cancelled", "中断しました")));
            if let Popup::AiChat { pending, scroll, .. } = &mut self.popup {
                *pending = false;
                *scroll = usize::MAX;
            }
            return true;
        }
        false
    }

    /// Send the typed chat line to the model.
    pub(crate) fn send_ai_message(&mut self) {
        let (question, mode, prior) =
            if let Popup::AiChat { input, log, pending, scroll, mode, .. } = &mut self.popup {
                let q = input.trim().to_string();
                if q.is_empty() || *pending {
                    return;
                }
                input.clear();
                // Taken before the new question joins the transcript, so it is
                // the conversation *so far* — see [`App::chat_prior`].
                let prior: Vec<cian_ai::Turn> = log
                    .iter()
                    .map(|m| cian_ai::Turn { user: m.user, text: m.text.clone() })
                    .collect();
                log.push(ChatMsg { user: true, text: q.clone() });
                *pending = true;
                *scroll = usize::MAX;
                (q, *mode, prior)
            } else {
                return;
            };
        self.chat_prior = prior;
        match mode {
            ChatMode::Ai => {
                let system = "You are a concise assistant embedded in a terminal file \
                              manager. Answer briefly in plain text."
                    .to_string();
                self.ai_request(AiPurpose::Chat, system, question);
            }
        }
    }

    /// Open the "describe a command" prompt (if AI is available).
    pub(crate) fn start_ai_shell_prompt(&mut self) {
        if !self.ai_available() {
            return;
        }
        self.open_popup(text_input(
            tr(self.lang, " Command from description ", " 説明からコマンド生成 "),
            tr(self.lang, "describe what you want to do:", "やりたいことを説明してください:"),
            String::new(),
            InputKind::AiShellCmd,
        ));
    }

    /// Ask the model for a shell command that does what `description` says, then
    /// show it for review before it touches the prompt.
    pub(crate) fn start_ai_shell_cmd(&mut self, description: &str) {
        let description = description.trim().to_string();
        if description.is_empty() {
            return;
        }
        // Where will this command actually run? The active shell may be
        // local, or already logged into a server over SSH — the command must
        // suit THAT system (AIX `ls` vs Windows `dir`, and never an
        // ssh-wrapped one). The three signals and the sentence they build are
        // `cian_core::shellwhere`, shared with the window build, which had
        // none of them and wrote PowerShell for Linux servers.
        let screen = self
            .shell
            .active_session()
            .and_then(|s| s.parser().lock().ok().map(|p| p.screen().contents()));
        let title = self.shell.active_title();
        let hosts = self.config.ssh_hosts.clone();
        let known = |h: &str| {
            hosts
                .iter()
                .find(|x| x.host == h || x.name == h)
                .and_then(|x| x.notes.clone())
        };
        let target = cian_core::shellwhere::describe(
            title.as_deref(),
            screen.as_deref(),
            known,
            cian_core::aiprompt::os_name(),
            &self.shell_cmd_name(),
        );
        // No `ai_context_block()` here: `ai_request` prepends it already, and
        // appending it too sent the same paragraph twice.
        let system = cian_core::aiprompt::cmd(&target);
        self.message = Some(tr(self.lang, "asking AI for a command…", "AI にコマンドを問い合わせ中…").into());
        self.ai_request(AiPurpose::ShellCommand { description: description.clone() }, system, description);
    }

    /// Open the "not quite — more like this" prompt over a proposed command.
    pub(crate) fn start_ai_shell_refine_prompt(&mut self) {
        let Popup::AiShellConfirm { command, description } = &self.popup else { return };
        self.open_popup(text_input(
            tr(self.lang, " Adjust the command ", " コマンドを修正 "),
            tr(
                self.lang,
                "what should be different about it?",
                "どこをどう変えてほしいですか？",
            ),
            String::new(),
            InputKind::AiShellRefine {
                description: description.clone(),
                rejected: command.clone(),
            },
        ));
    }

    /// Ask again, with the first answer and what was wrong with it in hand.
    ///
    /// The whole exchange goes back rather than just the correction: a model
    /// told only "shorter" has nothing to make shorter. What comes back lands
    /// in the same review popup, so this can be done as many times as it takes
    /// — and each round keeps the ones before it, so the model is reading the
    /// conversation rather than the last sentence of it.
    pub(crate) fn start_ai_shell_refine(&mut self, description: &str, rejected: &str, note: &str) {
        let note = note.trim();
        if note.is_empty() {
            // Nothing said — put the command back rather than asking the model
            // to guess at what silence meant.
            self.open_popup(Popup::AiShellConfirm {
                command: rejected.to_string(),
                description: description.to_string(),
            });
            return;
        }
        let combined = format!(
            "{description}\n\n\
             You already proposed this command, and it was not right:\n\
             {rejected}\n\n\
             Change it so that: {note}",
        );
        self.start_ai_shell_cmd(&combined);
    }

    /// Draft a commit message from the staged diff of the active pane's repo,
    /// then show it editable before committing. Silent-ish when AI is off, and
    /// helpful when the stage is empty (the common "forgot to `git add`" case).
    pub(crate) fn start_ai_commit_message(&mut self) {
        if !self.ai_configured() {
            return;
        }
        let Some(dir) = self.cwd() else { return };
        // Not in a repo at all?
        let Some(diff) = cian_core::git::staged_diff(&dir) else {
            self.message = Some(tr(self.lang, "not a git repository", "git リポジトリではありません").into());
            return;
        };
        if diff.trim().is_empty() {
            self.message = Some(tr(self.lang, "nothing staged. `git add` first (or stage from the pane)", "ステージされていません。先に `git add`（ペインからでも可）").into());
            return;
        }
        if !self.ai_ready() {
            self.message = Some(tr(self.lang, "AI unavailable (python, packages, or sign-in)", "AI を利用できません（python・パッケージ・サインインのいずれか）").into());
            return;
        }
        let stat = cian_core::git::staged_stat(&dir).unwrap_or_default();
        // Keep the payload bounded: a huge diff would blow the token budget and
        // rarely improves the message. The stat line still names every file.
        let diff = truncate_diff_for_ai(&diff, 12_000);
        let system = cian_core::aiprompt::COMMIT.to_string();
        self.message = Some(tr(self.lang, "asking AI to draft a commit message…", "AI にコミットメッセージを作成させています…").into());
        self.ai_request(AiPurpose::CommitMessage { dir, stat }, system, diff);
    }

    /// Ask the AI which entries below the active pane look like junk (build
    /// output, caches, temp/backup files, OS cruft), then show them for review.
    pub(crate) fn start_ai_junk(&mut self) {
        self.start_ai_survey(true);
    }

    /// Ask the AI to propose an organised folder layout for the active pane,
    /// then show the moves for review.
    pub(crate) fn start_ai_structure(&mut self) {
        self.start_ai_survey(false);
    }

    /// The two of them, which are one request with two questions.
    ///
    /// They were written out twice and `scripts/audit.py` scored them 0.92
    /// alike the moment both moved onto the survey — the engine had already
    /// arrived at the shape they share (`"aijunk" | "aistructure"`), and this
    /// is the same shape said once. Only metadata (paths, sizes, ages, dir
    /// flags) leaves the machine either way; contents never do, which is the
    /// rule that makes these usable at work.
    fn start_ai_survey(&mut self, junk: bool) {
        if !self.ai_configured() {
            return;
        }
        let Some(pane) = self.active_pane() else { return };
        // **Not on a remote pane.** `cwd` on one of those is whatever local
        // directory was there before the connection — so this would walk
        // *this* machine and label the result as the far one's contents.
        // Wrong quietly, which is the worst way to be wrong.
        if pane.remote_view().is_some() {
            self.message = Some(tr(self.lang,
                "not on a remote pane — this reads the local disk",
                "リモートペインでは使えません（この機能は手元のディスクを読みます）").into());
            return;
        }
        let dir = pane.cwd.clone();
        if !self.ai_ready() {
            self.message = Some(tr(self.lang, "AI unavailable (python, packages, or sign-in)", "AI を利用できません（python・パッケージ・サインインのいずれか）").into());
            return;
        }
        // **Junk nests and tidying does not.** A `node_modules` two folders
        // down is the commonest thing anybody wants gone, and one level of
        // names never saw it. A structure proposal only ever moves the loose
        // entries of *this* directory, so going deeper would show the model
        // files it is not allowed to touch.
        //
        // What both gain over the old name list is the size and age columns.
        // A directory used to arrive with a blank where its subtree size
        // should be — so junk was judged on vocabulary alone — and age is what
        // makes `2023/` a better folder than `misc/`.
        //
        // On this thread, unlike the engine's copy, which moved the same walk
        // to a worker. The difference is what a pause costs: the engine serves
        // a window over a pipe, so a second spent here queues every keystroke
        // behind it and the window looks frozen. Here the pause is the same
        // second on an AI command that is about to wait several more on the
        // model, in a program whose key loop is one thread by design. If a
        // repo full of build output ever makes this feel long, `size_budget`
        // is the dial and a job is the proper answer.
        let limits = if junk {
            cian_core::survey::Limits { depth: 4, rows: 800, hidden: false, ..Default::default() }
        } else {
            cian_core::survey::Limits { depth: 1, rows: 600, hidden: false, ..Default::default() }
        };
        let stop = std::sync::atomic::AtomicBool::new(false);
        let found = cian_core::survey::survey(&dir, limits, &stop);
        if found.rows.is_empty() {
            self.message = Some(if junk {
                tr(self.lang, "nothing here to scan", "スキャンする対象がありません").into()
            } else {
                tr(self.lang, "nothing here to organise", "整理する対象がありません").to_string()
            });
            return;
        }
        // The path→place map, used both to build the prompt and to validate
        // the reply back to real paths.
        let names: Vec<(String, PathBuf)> =
            found.rows.iter().map(|r| (r.rel.clone(), r.path.clone())).collect();
        let user = cian_core::aiprompt::survey_user(
            &format!("Directory: {}", dir.display()),
            &found,
            std::time::SystemTime::now(),
        );
        self.message = Some(self.survey_shortfall(&found).unwrap_or_else(|| {
            if junk {
                tr(self.lang, "asking AI to find junk…", "AI に不要ファイルを探させています…").into()
            } else {
                tr(self.lang, "asking AI to suggest a structure…", "AI に構成を提案させています…").to_string()
            }
        }));
        let (purpose, system) = if junk {
            (AiPurpose::Junk { names }, cian_core::aiprompt::JUNK)
        } else {
            (AiPurpose::Structure { names, dir }, cian_core::aiprompt::STRUCTURE)
        };
        self.ai_request(purpose, system.to_string(), user);
    }

    /// What to say when the walk did not reach everything.
    ///
    /// **A cap nobody is told about is a lie.** "AI は不要ファイルを見つけま
    /// せんでした" about a tree two thirds of which was never opened reads as
    /// a fact about the tree. The model is told the same thing in English by
    /// `aiprompt::survey_user`; this is the half a person reads.
    fn survey_shortfall(&self, s: &cian_core::survey::Survey) -> Option<String> {
        if !s.partial() {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        if s.stopped_at.is_some() {
            parts.push(match (self.lang, s.whole_to()) {
                (Lang::Ja, Some(d)) => format!("{d} 階層下までで打ち切りました"),
                (Lang::Ja, None) => "このディレクトリの途中で打ち切りました".into(),
                (_, Some(d)) => format!("it stops {d} level(s) down"),
                (_, None) => "it stops partway through this directory".into(),
            });
        }
        if s.unopened > 0 {
            parts.push(match self.lang {
                Lang::Ja => format!("{} 個のディレクトリは深すぎて開いていません", s.unopened),
                _ => format!("{} directories were too deep", s.unopened),
            });
        }
        Some(match self.lang {
            Lang::Ja => format!("AI に問い合わせ中 ── ただし {}", parts.join("・")),
            _ => format!("asking AI — but {}", parts.join("; ")),
        })
    }

    /// Run the checked moves from a structure suggestion on a worker: create
    /// each destination sub-folder (under the pane's directory) and move the
    /// file in. Skips on name conflict rather than overwriting.
    pub(crate) fn apply_structure_plan(&mut self) {
        let (dir, moves) = if let Popup::StructureReview { items, dir, .. } = &self.popup {
            let picked: Vec<(PathBuf, String)> = items
                .iter()
                .filter(|it| it.selected)
                .map(|it| (it.path.clone(), it.dest.clone()))
                .collect();
            (dir.clone(), picked)
        } else {
            return;
        };
        if moves.is_empty() {
            self.message = Some(tr(self.lang, "nothing checked", "チェックされていません").into());
            return;
        }
        self.popup = Popup::None;
        self.start_op("organising", move |ctl| {
            let mut report = OpReport::default();
            let total = moves.len();
            for (i, (src, folder)) in moves.iter().enumerate() {
                if ctl.cancel.load(Ordering::Relaxed) {
                    break;
                }
                (ctl.on_progress)(&cian_core::progress::Progress {
                    files_done: i,
                    files_total: total,
                    current: src.display().to_string(),
                    ..Default::default()
                });
                let dest_dir = dir.join(folder);
                if let Err(e) = cian_core::ops::make_dir(&dir, folder, true) {
                    report.note_error(format!("{}: {}", folder, e));
                    continue;
                }
                match cian_core::ops::move_one(src, &dest_dir, Conflict::Skip) {
                    Ok(true) => report.ok += 1,
                    Ok(false) => report.skipped += 1,
                    Err(e) => report.note_error(format!("{}: {}", src.display(), e)),
                }
            }
            report
        });
    }

    /// Ask how to rename, then propose new names for the chosen files. The files
    /// are the marked ones, or the whole listing when nothing is marked.
    pub(crate) fn start_ai_rename_prompt(&mut self) {
        if !self.ai_available() {
            return;
        }
        // Which files: marks if any, else every real entry in the listing.
        let any = self.active_pane().map(|p| {
            p.mark_count() > 0 || p.entries.iter().any(|e| !e.is_parent)
        }).unwrap_or(false);
        if !any {
            self.message = Some(tr(self.lang, "nothing here to rename", "リネームする対象がありません").into());
            return;
        }
        self.open_popup(text_input(
            tr(self.lang, " AI rename ", " AIリネーム "),
            tr(
                self.lang,
                "how should these be renamed? (e.g. snake_case, add a date prefix):",
                "どうリネームしますか？（例: snake_case、日付を先頭に）:",
            ),
            String::new(),
            InputKind::AiRename,
        ));
    }

    /// Send the chosen files' names plus the instruction to the model and show
    /// its proposed renames for review.
    pub(crate) fn start_ai_rename(&mut self, instruction: &str) {
        let instruction = instruction.trim().to_string();
        if instruction.is_empty() {
            self.message = Some(tr(self.lang, "cancelled (no instruction)", "中止しました（指示なし）").into());
            return;
        }
        let Some(pane) = self.active_pane() else { return };
        // Marked files, or the whole listing (never the `..` row).
        let chosen: Vec<(String, PathBuf)> = if pane.mark_count() > 0 {
            pane.entries.iter()
                .filter(|e| !e.is_parent && pane.marks.contains(&e.path))
                .map(|e| (e.name.clone(), e.path.clone()))
                .collect()
        } else {
            pane.entries.iter()
                .filter(|e| !e.is_parent)
                .map(|e| (e.name.clone(), e.path.clone()))
                .collect()
        };
        if chosen.is_empty() {
            self.message = Some(tr(self.lang, "nothing to rename", "リネームする対象がありません").into());
            return;
        }
        let listing: String = chosen.iter().take(400).map(|(n, _)| format!("{}\n", n)).collect();
        let system = cian_core::aiprompt::RENAME.to_string();
        let user = format!("Instruction: {}\n\nFiles:\n{}", instruction, listing);
        let names = chosen;
        self.message = Some(tr(self.lang, "asking AI for new names…", "AI に新しい名前を問い合わせ中…").into());
        self.ai_request(AiPurpose::Rename { names }, system, user);
    }

    /// Prompt for a natural-language query, then semantic-search the tree.
    pub(crate) fn start_ai_search_prompt(&mut self) {
        if !self.ai_available() {
            return;
        }
        self.open_popup(text_input(
            tr(self.lang, " Semantic search ", " セマンティック検索 "),
            tr(self.lang, "describe what you're looking for:", "探しているものを説明してください:"),
            String::new(),
            InputKind::AiSearch,
        ));
    }

    /// Build a catalog of file paths under the active pane and ask the model
    /// which are most relevant to `query`. Metadata only — paths, not contents.
    pub(crate) fn start_ai_search(&mut self, query: &str) {
        let query = query.trim().to_string();
        if query.is_empty() {
            self.message = Some(tr(self.lang, "cancelled (no query)", "中止しました（検索語なし）").into());
            return;
        }
        let Some(root) = self.cwd() else { return };
        if self.active_pane().map(|p| p.remote_view().is_some()).unwrap_or(false) {
            self.message = Some(tr(self.lang,
                "not on a remote pane — this reads the local disk",
                "リモートペインでは使えません（この機能は手元のディスクを読みます）").into());
            return;
        }
        // Hidden included: somebody looking for "the eslint config" means
        // `.eslintrc`, and a search that cannot see dotfiles fails on exactly
        // the files nobody can remember the name of.
        //
        // Directories are listed too, and that is a change. The old catalog
        // was files only, on the grounds that a directory has nothing to
        // preview — but "where does the auth code live" is a question whose
        // answer is a folder, and a search that cannot name one cannot answer
        // it. F3 on a directory row simply does nothing, which is a smaller
        // cost than the question going unanswered.
        let limits = cian_core::survey::Limits { depth: 6, rows: 2000, hidden: true, ..Default::default() };
        let stop = std::sync::atomic::AtomicBool::new(false);
        let found = cian_core::survey::survey(&root, limits, &stop);
        if found.rows.is_empty() {
            self.message = Some(tr(self.lang, "no files here to search", "ここに検索対象のファイルがありません").into());
            return;
        }
        // Back into the shape the results list already knows how to show.
        let catalog: Vec<cian_core::search::Hit> = found
            .rows
            .iter()
            .map(|r| cian_core::search::Hit {
                path: r.path.clone(),
                rel: std::path::PathBuf::from(&r.rel),
                is_dir: r.is_dir,
                line: None,
            })
            .collect();
        let system = cian_core::aiprompt::SEARCH.to_string();
        let user = cian_core::aiprompt::survey_user(
            &format!("Question: {query}"),
            &found,
            std::time::SystemTime::now(),
        );
        self.message = Some(self.survey_shortfall(&found).unwrap_or_else(|| {
            tr(self.lang, "asking AI to find relevant files…", "AI に関連ファイルを探させています…").into()
        }));
        self.ai_request(AiPurpose::SemSearch { hits: catalog }, system, user);
    }

    /// Run the checked renames in place, then reload and report.
    pub(crate) fn apply_rename_plan(&mut self) {
        let renames: Vec<(PathBuf, String)> = if let Popup::RenameReview { items, .. } = &self.popup {
            items.iter().filter(|it| it.selected).map(|it| (it.path.clone(), it.new.clone())).collect()
        } else {
            return;
        };
        if renames.is_empty() {
            self.message = Some(tr(self.lang, "nothing checked", "チェックされていません").into());
            return;
        }
        self.popup = Popup::None;
        let mut report = OpReport::default();
        for (src, new) in &renames {
            // Skip if the target already exists, rather than clobbering it.
            if src.parent().map(|p| p.join(new).exists()).unwrap_or(false) {
                report.skipped += 1;
                continue;
            }
            match cian_core::ops::rename_in_place(src, new) {
                Ok(_) => report.ok += 1,
                Err(e) => report.note_error(format!("{}: {}", src.display(), e)),
            }
        }
        self.reload_active();
        self.flash(self.focused);
        self.show_op_report(&report);
    }

    /// Scan the active pane's tree for byte-identical files on a worker thread.
    pub(crate) fn start_dupes(&mut self) {
        if self.dupes_job.is_some() {
            self.message = Some(tr(self.lang, "a duplicate scan is already running", "重複スキャンは既に実行中です").into());
            return;
        }
        let Some(root) = self.cwd() else { return };
        // Collect files recursively, bounded so a giant tree cannot run away.
        const CAP: usize = 20_000;
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let mut files: Vec<PathBuf> = Vec::new();
        let q = cian_core::search::Query::new("");
        {
            let cancel = &cancel;
            let files = &mut files;
            cian_core::search::search(&root, &q, cancel, &mut |h| {
                if !h.is_dir {
                    files.push(h.path);
                    if files.len() >= CAP {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            });
        }
        if files.len() < 2 {
            self.message = Some(tr(self.lang, "nothing to compare", "比較する対象がありません").into());
            return;
        }
        self.message = Some(if self.lang == crate::theme::Lang::Ja {
            format!("{} 件のファイルを重複検査中…", files.len())
        } else {
            format!("scanning {} files for duplicates…", files.len())
        });
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let cancel = AtomicBool::new(false);
            let groups = cian_core::dedup::find_duplicates(&files, &cancel);
            let _ = tx.send(groups);
        });
        self.dupes_job = Some(rx);
    }

    /// Drain the duplicate scan; when it finishes, open the review popup.
    pub(crate) fn poll_dupes_job(&mut self) -> bool {
        let Some(rx) = &self.dupes_job else { return false };
        let groups = match rx.try_recv() {
            Ok(g) => g,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.dupes_job = None;
                return false;
            }
        };
        self.dupes_job = None;
        if groups.is_empty() {
            self.message = Some(tr(self.lang, "no duplicate files found", "重複ファイルは見つかりませんでした").into());
            return true;
        }
        // Flatten into rows: the first file of each group is the keeper (left
        // unchecked); the rest are pre-checked for deletion.
        let mut items = Vec::new();
        for (g, group) in groups.iter().enumerate() {
            for (i, path) in group.iter().enumerate() {
                let keeper = i == 0;
                items.push(DupeItem { path: path.clone(), group: g, keeper, selected: !keeper });
            }
        }
        let dupes = groups.len();
        self.message = Some(format!("{} duplicate group(s) — review and delete", dupes));
        self.open_popup(Popup::DupeReview { items, cursor: 0, scroll: 0 });
        true
    }

    /// Hand whatever the open review has checked — junk candidates, or the
    /// redundant copies of a duplicate — to the normal delete confirmation, so
    /// removal goes through the same trash/permanent path (and its own y/Enter
    /// approval) as any other delete. Never straight to disk from here.
    pub(crate) fn confirm_review_deletion(&mut self) {
        let targets = match &self.popup {
            Popup::JunkReview { items, .. } => checked_paths(items),
            Popup::DupeReview { items, .. } => checked_paths(items),
            _ => return,
        };
        if targets.is_empty() {
            self.message = Some(tr(self.lang, "nothing checked", "チェックされていません").into());
            return;
        }
        self.open_popup(Popup::ConfirmDelete { targets });
    }

    /// Commit the staged changes with the (possibly edited) drafted message.
    pub(crate) fn commit_with_drafted_message(&mut self) {
        let (dir, message) = if let Popup::CommitMessage { dir, buffer, .. } = &self.popup {
            (dir.clone(), buffer.trim().to_string())
        } else {
            return;
        };
        if message.is_empty() {
            self.message = Some(tr(self.lang, "empty message. nothing committed", "メッセージが空です。コミットしていません").into());
            return;
        }
        self.popup = Popup::None;
        match cian_core::git::commit(&dir, &message) {
            Ok(()) => {
                let subject = message.lines().next().unwrap_or("").to_string();
                self.message = Some(format!("✔ committed: {}", truncate(&subject, 60)));
                // The stage is now clean; refresh the markers.
                self.invalidate_git();
            }
            Err(e) => {
                self.open_popup(Popup::Notice {
                    lines: vec!["commit failed:".into(), String::new(), e.to_string()],
                });
            }
        }
    }

    /// The shell program's base name, for the command-generation prompt.
    pub(crate) fn shell_cmd_name(&self) -> String {
        std::path::Path::new(self.shell.command())
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sh".into())
    }

    /// Copy the chat: the selected transcript lines if a range is selected,
    /// otherwise the whole of the last assistant reply.
    pub(crate) fn copy_ai_text(&mut self) {
        let text = if let Popup::AiChat { log, sel, .. } = &self.popup {
            match sel {
                Some((a, b)) => {
                    let lo = (*a).min(*b);
                    let hi = (*a).max(*b).min(self.ai_lines.len().saturating_sub(1));
                    if self.ai_lines.is_empty() {
                        String::new()
                    } else {
                        self.ai_lines[lo..=hi].join("\n")
                    }
                }
                None => log.iter().rev().find(|m| !m.user).map(|m| m.text.clone()).unwrap_or_default(),
            }
        } else {
            return;
        };
        if text.trim().is_empty() {
            self.message = Some(tr(self.lang, "nothing to copy", "コピーする対象がありません").into());
            return;
        }
        if let Some(cb) = self.clipboard.as_mut() {
            let _ = cb.set_text(text);
        }
        self.message = Some(tr(self.lang, "copied", "コピーしました").into());
        if let Popup::AiChat { sel, .. } = &mut self.popup {
            *sel = None;
        }
    }

    /// Paste into the chat: an image on the clipboard becomes an attachment,
    /// anything else becomes text. Splitting those onto two keys made the user
    /// classify their own clipboard before pressing anything.
    pub(crate) fn paste_into_chat(&mut self) {
        let has_image = self.clipboard.as_mut().map(|cb| cb.get_image().is_ok()).unwrap_or(false);
        if has_image {
            self.attach_clipboard_image();
            return;
        }
        let text = self.clipboard_text();
        if let (Some(t), Popup::AiChat { input, .. }) = (text, &mut self.popup) {
            input.push_str(t.trim_end_matches(['\r', '\n']));
        }
    }

    /// Attach the image on the system clipboard to the open chat (Alt+V), so a
    /// screenshot can be asked about. Written out as a PNG under the temp dir
    /// because the helper wants a file path:
    /// and the Simple AI helper base64s it into the request. Alt rather than
    /// Ctrl/Cmd+V — the terminal claims those for pasting text.
    pub(crate) fn attach_clipboard_image(&mut self) {
        let Some(cb) = self.clipboard.as_mut() else {
            self.message = Some(tr(self.lang, "no clipboard", "クリップボードなし").into());
            return;
        };
        let img = match cb.get_image() {
            Ok(i) => i,
            Err(_) => {
                self.message =
                    Some(tr(self.lang, "clipboard has no image", "クリップボードに画像なし").into());
                return;
            }
        };
        let (w, h) = (img.width as u32, img.height as u32);
        let Some(buf) = image::RgbaImage::from_raw(w, h, img.bytes.into_owned()) else {
            self.message = Some(tr(self.lang, "unreadable image", "画像を読めません").into());
            return;
        };
        let dir = std::env::temp_dir().join("cian-paste");
        if std::fs::create_dir_all(&dir).is_err() {
            self.message = Some(tr(self.lang, "cannot write temp file", "一時ファイルを作れません").into());
            return;
        }
        // Named off the monotonic clock rather than a per-question index: the
        // list is emptied on send, so an index would reuse the name of a file
        // the helper may still be reading out of the temp dir.
        let path = dir.join(format!("paste-{}.png", self.startup_at.elapsed().as_nanos()));
        if buf.save(&path).is_err() {
            self.message = Some(tr(self.lang, "cannot write temp file", "一時ファイルを作れません").into());
            return;
        }
        self.chat_attachments.push(path);
        self.message = Some(format!("▣ image attached ({w}×{h})"));
    }

    /// Drain the AI worker and route the reply by its purpose.
    pub(crate) fn poll_ai_job(&mut self) -> bool {
        let Some(job) = &self.ai_job else { return false };
        let result = match job.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("AI worker died".to_string()),
        };
        let purpose = self.ai_job.take().map(|j| j.purpose).unwrap_or(AiPurpose::Chat);
        match purpose {
            AiPurpose::Chat => {
                if let Popup::AiChat { log, pending, scroll, .. } = &mut self.popup {
                    *pending = false;
                    *scroll = usize::MAX;
                    match result {
                        Ok(text) => log.push(ChatMsg { user: false, text }),
                        Err(e) => log.push(ChatMsg { user: false, text: format!("[error] {}", e) }),
                    }
                }
            }
            AiPurpose::ShellCommand { description } => match result {
                Ok(text) => {
                    let command = clean_ai_command(&text);
                    if command.is_empty() {
                        self.message = Some(tr(self.lang, "AI returned no command", "AI からコマンドが返りませんでした").into());
                    } else {
                        self.open_popup(Popup::AiShellConfirm { command, description });
                    }
                }
                Err(e) => self.message = Some(format!("AI: {}", e)),
            },
            AiPurpose::CommitMessage { dir, stat } => match result {
                Ok(text) => {
                    let msg = clean_ai_commit_message(&text);
                    if msg.is_empty() {
                        self.message = Some(tr(self.lang, "AI returned no message", "AI からメッセージが返りませんでした").into());
                    } else {
                        self.open_popup(Popup::CommitMessage { buffer: msg, stat, dir, editing: false });
                    }
                }
                Err(e) => self.message = Some(format!("AI: {}", e)),
            },
            AiPurpose::Junk { names, .. } => match result {
                Ok(text) => {
                    let items = parse_junk_reply(&text, &names);
                    if items.is_empty() {
                        self.message = Some(tr(self.lang, "AI found no obvious junk", "AI は不要そうなファイルを見つけませんでした").into());
                    } else {
                        self.open_popup(Popup::JunkReview { items, cursor: 0, scroll: 0 });
                    }
                }
                Err(e) => self.message = Some(format!("AI: {}", e)),
            },
            AiPurpose::Structure { names, dir } => match result {
                Ok(text) => {
                    let items = parse_structure_reply(&text, &names);
                    if items.is_empty() {
                        self.message = Some(tr(self.lang, "AI had no structure changes to suggest", "AI から構成変更の提案はありません").into());
                    } else {
                        self.open_popup(Popup::StructureReview { items, cursor: 0, scroll: 0, dir });
                    }
                }
                Err(e) => self.message = Some(format!("AI: {}", e)),
            },
            AiPurpose::Rename { names } => match result {
                Ok(text) => {
                    let items = parse_rename_reply(&text, &names);
                    if items.is_empty() {
                        self.message = Some(tr(self.lang, "AI proposed no renames", "AI からリネームの提案はありません").into());
                    } else {
                        self.open_popup(Popup::RenameReview { items, cursor: 0, scroll: 0, by_ai: true });
                    }
                }
                Err(e) => self.message = Some(format!("AI: {}", e)),
            },
            AiPurpose::SemSearch { hits } => match result {
                Ok(text) => {
                    let matched = parse_sem_search_reply(&text, &hits);
                    if matched.is_empty() {
                        self.message = Some(tr(self.lang, "AI found no relevant files", "AI は該当するファイルを見つけませんでした").into());
                    } else {
                        // Reuse the find-results list: F3 preview, Ctrl+n/N, Esc.
                        self.find_return = None;
                        self.message = Some(format!("{} relevant file(s) — Enter previews", matched.len()));
                        self.popup =
                            Popup::FindResults { hits: matched, cursor: 0, scroll: 0, by_ai: true };
                    }
                }
                Err(e) => self.message = Some(format!("AI: {}", e)),
            },
        }
        true
    }
}

#[cfg(test)]
mod ai_history_tests {
    use super::*;

    #[test]
    fn stored_chats_round_trip_mode_skin_and_log() {
        let stored = vec![
            StoredChat::new(
                ChatMode::Ai,
                ChatSkin::of(ChatMode::Ai),
                vec![
                    ChatMsg { user: true, text: "q1".into() },
                    ChatMsg { user: false, text: "a1\nline".into() },
                ],
            ),
            // A window with a title of its own — the AI actions that name what
            // they did rather than just "Chat".
            StoredChat::new(
                ChatMode::Ai,
                ChatSkin { title: "AI - Rename".into(), simple: true },
                vec![ChatMsg { user: true, text: "q2".into() }],
            ),
        ];
        // Serialize exactly as save_ai_history does, then read back as restore does.
        let json = serde_json::to_string(&stored).unwrap();
        let back: Vec<StoredChat> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, stored, "mode and transcript survive a round trip");
        assert_eq!(back[0].skin().title, "Chat");
        assert_eq!(back[1].skin().title, "AI - Rename");
    }

    /// History written before skins existed still loads, falling back to the
    /// backend's default look.
    #[test]
    fn a_skinless_stored_chat_falls_back_to_its_mode() {
        let json = r#"[{"mode":"Ai","log":[{"user":true,"text":"q"}]}]"#;
        let back: Vec<StoredChat> = serde_json::from_str(json).unwrap();
        assert_eq!(back[0].skin().title, "Chat");
        assert!(back[0].skin().simple);
    }
}
