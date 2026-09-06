//! Optional AI features for cian, talking to Azure OpenAI through the same
//! Windows broker (WAM) authentication.
//!
//! The actual auth and API call live in a small Python helper ([`SCRIPT`],
//! embedded at build time and written to a cache dir on first use) because the
//! broker credential is a Python/azure-identity concept with no practical pure
//! Rust equivalent. cian shells out to it, one process per request, and treats
//! any failure — no python, no packages, offline, not signed in — as "AI
//! unavailable": the features simply do not appear. Nothing here ever blocks
//! cian from running.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// The bundled Python helper, materialised to disk when needed.
const SCRIPT: &str = include_str!("../cian_ai.py");

/// How to reach the model. `auth_mode` is
/// `broker` (Windows AAD), `apikey`, or `mock` (offline echo, for testing).
#[derive(Debug, Clone)]
pub struct AiConfig {
    pub python: String,
    pub endpoint: String,
    pub model: String,
    pub api_version: String,
    pub auth_mode: String,
    pub api_key: String,
    pub api_base_url: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            python: "python".into(),
            // Deliberately empty. This used to carry one organisation's
            // internal API gateway, compiled into a program whose source is
            // public — a hostname nobody outside that network can use and
            // everybody outside it can read. Where the AI is meant to be used,
            // `cian.ai{ endpoint = … }` says so; where it is not, an empty
            // string is the honest default and the error names what to set.
            endpoint: String::new(),
            model: "gpt-5-mini".into(),
            api_version: "2025-04-01-preview".into(),
            auth_mode: "broker".into(),
            api_key: String::new(),
            api_base_url: String::new(),
        }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    messages: Vec<Message<'a>>,
    model: &'a str,
    endpoint: &'a str,
    api_version: &'a str,
    auth_mode: &'a str,
    api_key: &'a str,
    api_base_url: &'a str,
    max_tokens: u32,
    /// Local image file paths to attach to the last user turn (Vision). The
    /// helper reads and base64-encodes them; empty when there are none.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    images: &'a [String],
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

/// One turn of a conversation already had.
///
/// **This did not exist, and its absence was invisible.** `chat` sent
/// `[system, user]` and nothing else, so a chat window that showed six
/// exchanges was six unrelated questions to a model that had been told none of
/// them. On screen it looked exactly like a conversation; "as I said above"
/// was answered by a stranger. Found 2026-09-06 by reading, because the one
/// thing this cannot be caught by is looking at the transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// True for the person's turn, false for the model's.
    pub user: bool,
    pub text: String,
}

/// How much of a conversation is worth carrying, in bytes of UTF-8.
///
/// Every turn is re-sent every turn — that is what a memory costs against a
/// stateless endpoint — so a long afternoon in one window would otherwise grow
/// each question without limit until the endpoint refused it, and the refusal
/// would arrive as "the AI stopped working". Oldest turns are dropped first,
/// and whole: half a turn is a sentence with no speaker.
///
/// Bytes rather than tokens because cian cannot count the model's tokens
/// without the model, and bytes are what it can measure exactly. Twenty-four
/// thousand is about eight thousand Japanese characters, which leaves room
/// beside a 1024-token reply.
const CARRY: usize = 24_000;

/// The turns to actually send: the most recent ones that fit in [`CARRY`].
///
/// **The newest turn is kept whatever it weighs.** It is the answer the
/// question being asked is usually *about* — "fix line 30" after a page of
/// code — so dropping it for being big turns the follow-up into nonsense. A
/// request too large for the endpoint is refused and says so; a request that
/// quietly lost the thing it refers to is answered, wrongly.
fn carried(prior: &[Turn]) -> &[Turn] {
    let Some(from_at_most) = prior.len().checked_sub(1) else { return prior };
    let mut used = prior[from_at_most].text.len();
    let mut from = from_at_most;
    for (i, t) in prior[..from_at_most].iter().enumerate().rev() {
        used += t.text.len();
        if used > CARRY {
            break;
        }
        from = i;
    }
    &prior[from..]
}

#[derive(Deserialize)]
struct Reply {
    ok: bool,
    #[serde(default)]
    content: String,
    #[serde(default)]
    error: String,
}

/// Write the embedded helper to a stable cache path and return it. Rewritten
/// each call is cheap and keeps it in sync with the binary.
fn script_path() -> Result<PathBuf> {
    let dir = cache_dir().join("cian");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join("cian_ai.py");
    // Only rewrite when the content differs, so a running helper is not clobbered.
    let stale = std::fs::read_to_string(&path).map(|s| s != SCRIPT).unwrap_or(true);
    if stale {
        std::fs::write(&path, SCRIPT).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(path)
}

fn cache_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
    if cfg!(windows) {
        if let Ok(x) = std::env::var("LOCALAPPDATA") {
            if !x.is_empty() {
                return PathBuf::from(x);
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache");
    }
    std::env::temp_dir()
}

/// Whether the AI helper is usable: python runs and the packages the auth mode
/// needs import. Does not touch the network or prompt for sign-in. Cheap enough
/// to call once at startup; cache the result.
pub fn available(cfg: &AiConfig) -> bool {
    // `mock` is always available (no packages, no network) — handy for tests.
    let Ok(script) = script_path() else { return false };
    cian_core::proc::quiet(&cfg.python)
        .arg(&script)
        .arg("--check")
        .arg(&cfg.auth_mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Send a chat turn and return the assistant's reply. Blocks on the network, so
/// callers run it on a worker thread.
pub fn chat(cfg: &AiConfig, system: &str, user: &str, images: &[String]) -> Result<String> {
    chat_with(cfg, system, &[], user, images)
}

/// Ask, with the conversation so far.
///
/// `prior` is oldest-first and excludes the question being asked. One-shot
/// callers — the renamer, the search, the commit-message drafter — pass none
/// and get exactly what [`chat`] always did: those parse the reply, and a
/// previous turn in the request is a previous turn's format in the answer.
pub fn chat_with(
    cfg: &AiConfig,
    system: &str,
    prior: &[Turn],
    user: &str,
    images: &[String],
) -> Result<String> {
    let script = script_path()?;
    let mut messages = Vec::new();
    if !system.is_empty() {
        messages.push(Message { role: "system", content: system });
    }
    for t in carried(prior) {
        messages.push(Message {
            role: if t.user { "user" } else { "assistant" },
            content: &t.text,
        });
    }
    messages.push(Message { role: "user", content: user });
    let req = ChatRequest {
        messages,
        model: &cfg.model,
        endpoint: &cfg.endpoint,
        api_version: &cfg.api_version,
        auth_mode: &cfg.auth_mode,
        api_key: &cfg.api_key,
        api_base_url: &cfg.api_base_url,
        max_tokens: 1024,
        images,
    };
    let body = serde_json::to_vec(&req)?;

    let mut child = cian_core::proc::quiet(&cfg.python)
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PYTHONUTF8", "1")
        .spawn()
        .with_context(|| format!("launch {} (is Python installed?)", cfg.python))?;
    child.stdin.take().context("stdin")?.write_all(&body).context("send request")?;
    let out = child.wait_with_output().context("run AI helper")?;

    let reply: Reply = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parse AI reply: {}", String::from_utf8_lossy(&out.stdout)))?;
    if reply.ok {
        Ok(reply.content)
    } else {
        Err(anyhow!("AI: {}", reply.error))
    }
}

impl AiConfig {
    /// Build a request-side config from what `cian.ai{…}` declared.
    ///
    /// Here rather than in a front end because there are two of them now, and
    /// this is the one place that decides what a setting in `init.lua` means.
    pub fn from_lua(config: &cian_lua::Config) -> Option<AiConfig> {
        config.ai.as_ref().map(|a| AiConfig {
            python: a.python.clone(),
            endpoint: a.endpoint.clone(),
            model: a.model.clone(),
            api_version: a.api_version.clone(),
            auth_mode: a.auth_mode.clone(),
            api_key: a.api_key.clone(),
            api_base_url: a.api_base_url.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_python() -> bool {
        cian_core::proc::quiet("python3").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    #[test]
    fn mock_chat_round_trips_through_python() {
        if !have_python() {
            eprintln!("no python3; skipping");
            return;
        }
        let cfg = AiConfig { python: "python3".into(), auth_mode: "mock".into(), ..Default::default() };
        assert!(available(&cfg), "mock check passes");
        let reply = chat(&cfg, "you are terse", "hello there", &[]).unwrap();
        assert_eq!(reply, "[mock] hello there", "the helper echoed the last message");
    }

    /// The conversation reaches the helper, and is counted there.
    ///
    /// Asserted at the far end on purpose. Everything between here and the
    /// model is `Serialize`, and a `prior` that is built correctly and then
    /// dropped on the way out looks, from the Rust side, exactly like one that
    /// arrived — which is the shape of the bug this fixes.
    #[test]
    fn the_conversation_so_far_reaches_the_helper() {
        if !have_python() {
            eprintln!("no python3; skipping");
            return;
        }
        let cfg = AiConfig { python: "python3".into(), auth_mode: "mock".into(), ..Default::default() };
        let prior = vec![
            Turn { user: true, text: "what is in this folder".into() },
            Turn { user: false, text: "three text files".into() },
        ];
        let reply = chat_with(&cfg, "you are terse", &prior, "and the biggest?", &[]).unwrap();
        assert_eq!(
            reply, "[mock +2] and the biggest?",
            "both earlier turns went with the question",
        );
    }

    /// A conversation longer than [`CARRY`] loses its oldest turns, whole.
    #[test]
    fn a_long_conversation_is_trimmed_from_the_front() {
        // 一手あたり CARRY のおよそ 1/4。6手で足が出る。
        let big = "あ".repeat(CARRY / 12);
        let prior: Vec<Turn> = (0..6)
            .map(|i| Turn { user: i % 2 == 0, text: format!("{i}{big}") })
            .collect();
        let kept = carried(&prior);
        assert!(kept.len() < prior.len(), "something was dropped, got {}", kept.len());
        assert!(
            kept.iter().map(|t| t.text.len()).sum::<usize>() <= CARRY,
            "what is kept fits",
        );
        assert_eq!(kept.last(), prior.last(), "the newest turn is always one of them");
        assert!(
            kept.first().unwrap().text.starts_with(|c: char| c.is_ascii_digit()),
            "turns are kept whole — half a turn is a sentence with no speaker",
        );
    }

    /// One turn heavier than the whole budget is still sent.
    ///
    /// It is what the next question is about. Dropping it leaves a follow-up
    /// referring to something the model was never shown — answered, and wrong,
    /// which is worse than a request the endpoint refuses out loud.
    #[test]
    fn the_newest_turn_survives_even_when_it_alone_is_too_big() {
        let prior = vec![
            Turn { user: true, text: "old".into() },
            Turn { user: false, text: "x".repeat(CARRY * 2) },
        ];
        let kept = carried(&prior);
        assert_eq!(kept.len(), 1, "the older turn went");
        assert_eq!(kept[0], prior[1], "the newest stayed");
    }

    /// Nothing to carry is the one-shot case every structured caller uses.
    #[test]
    fn no_prior_turns_is_the_request_it_always_was() {
        assert!(carried(&[]).is_empty());
    }
}
