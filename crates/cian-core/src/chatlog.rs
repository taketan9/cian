//! The AI chat history, and the shape it has on disk.
//!
//! **Here because there are two front ends and one file.** The terminal build
//! wrote `ai_history.json` and the window build did not know it existed, so a
//! conversation had in one was invisible in the other — and the moment the
//! window learned to write it, a second definition of the same schema would
//! have started drifting from the first. One definition, one file, both
//! readers.
//!
//! The path is passed in rather than worked out here: where a config lives is
//! `cian-lua`'s question (portable-first — the copy beside the executable wins
//! over `~/.config/cian`), and this crate does not depend on it.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One turn of a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// True for the person's turn, false for the model's.
    pub user: bool,
    pub text: String,
    /// What that turn really was, when what is shown is a label for it.
    ///
    /// The doors that open with something already asked — triage this log,
    /// summarise this file — show `Triage the log: access.log` and send the
    /// log. That reads well and, once conversations started being carried,
    /// meant a follow-up asked about *the file name*: the model was handed a
    /// sentence naming a log it had never been shown. Showing the payload
    /// instead would make the transcript unreadable, so the turn holds both.
    ///
    /// `None` when they are the same, which is every turn that was typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<String>,
}

impl Turn {
    /// A turn the person typed: shown and sent alike.
    pub fn you(text: impl Into<String>) -> Self {
        Turn { user: true, text: text.into(), sent: None }
    }

    /// A turn the person *made* — a label on screen, a payload to the model.
    pub fn you_sending(label: impl Into<String>, sent: impl Into<String>) -> Self {
        Turn { user: true, text: label.into(), sent: Some(sent.into()) }
    }

    /// A turn the model answered with.
    pub fn ai(text: impl Into<String>) -> Self {
        Turn { user: false, text: text.into(), sent: None }
    }

    /// What goes into the request for this turn.
    pub fn for_model(&self) -> &str {
        self.sent.as_deref().unwrap_or(&self.text)
    }
}

/// How a chat window presented itself: the name in its frame, and whether the
/// local model was the one answering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skin {
    pub title: String,
    pub simple: bool,
}

/// One conversation, as it is written down.
///
/// `mode` is a string rather than an enum so that a build which gains a second
/// backend does not make this file unreadable to a build that has not — an
/// unknown name is a conversation you can still read, and that is the point of
/// keeping it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin: Option<Skin>,
    pub log: Vec<Turn>,
}

/// A one-line name for a conversation: its first question.
///
/// The *shown* text, not the payload — a history row reading like sixteen
/// kilobytes of log would be a history you cannot skim.
pub fn title_of(log: &[Turn]) -> String {
    log.iter()
        .find(|m| m.user)
        .map(|m| m.text.replace('\n', " "))
        .map(|t| {
            if t.chars().count() > 60 {
                format!("{}…", t.chars().take(60).collect::<String>())
            } else {
                t
            }
        })
        .unwrap_or_else(|| "(empty)".to_string())
}

/// Read the history, newest first. Empty when there is none or it will not
/// parse — a broken history is not worth refusing to start over.
pub fn load(path: Option<&Path>) -> Vec<Stored> {
    let Some(path) = path.filter(|p| p.exists()) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    serde_json::from_str::<Vec<Stored>>(&text).unwrap_or_default()
}

/// Write it back. Best-effort: a read-only config directory means the history
/// does not survive a restart, which is not worth interrupting anyone over.
pub fn save(path: Option<&Path>, all: &[Stored]) {
    let Some(path) = path else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(all) {
        let _ = std::fs::write(path, json);
    }
}

/// Put a finished conversation at the front, and keep the list from growing
/// without end.
///
/// Deduped by content: opening the history and closing it again archives the
/// same conversation a second time, and a picker showing the same exchange
/// four times is a picker nobody reads to the bottom of.
///
/// Nothing is remembered until the model has answered once. A window opened
/// and shut is not a conversation, and half the rows being empty questions is
/// the fastest way to make a history worthless.
pub fn remember(all: &mut Vec<Stored>, one: Stored) {
    if !one.log.iter().any(|m| !m.user) {
        return;
    }
    all.retain(|c| c.log != one.log);
    all.insert(0, one);
    all.truncate(KEEP);
}

/// How many conversations to keep. Enough to find last week's, few enough that
/// the file stays something a person could read if they had to.
pub const KEEP: usize = 50;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_with_no_answer_is_not_worth_keeping() {
        let mut all = Vec::new();
        remember(&mut all, Stored { mode: "Ai".into(), skin: None, log: vec![Turn::you("hi")] });
        assert!(all.is_empty(), "a window opened and shut is not a conversation");
    }

    #[test]
    fn the_same_conversation_is_remembered_once() {
        let log = vec![Turn::you("hi"), Turn::ai("hello")];
        let one = || Stored { mode: "Ai".into(), skin: None, log: log.clone() };
        let mut all = Vec::new();
        remember(&mut all, one());
        remember(&mut all, one());
        assert_eq!(all.len(), 1, "archiving it twice is one row");
    }

    #[test]
    fn the_title_is_the_question_that_was_shown_not_the_payload() {
        let log = vec![
            Turn::you_sending("Triage the log: access.log", "ERROR ".repeat(4000)),
            Turn::ai("it is 500s"),
        ];
        assert_eq!(title_of(&log), "Triage the log: access.log");
    }

    /// It round-trips through the file, and an old entry still reads.
    ///
    /// The `sent` field arrived after the format did. A history written before
    /// it has to keep opening, or the price of the fix is everything anyone
    /// had asked before it.
    #[test]
    fn the_file_round_trips_and_old_entries_still_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ai_history.json");
        std::fs::write(
            &path,
            r#"[{"mode":"Ai","log":[{"user":true,"text":"before sent existed"}]}]"#,
        )
        .unwrap();
        let old = load(Some(&path));
        assert_eq!(old.len(), 1, "an entry from before the field still reads");
        assert_eq!(old[0].log[0].sent, None);

        let mut all = old;
        remember(
            &mut all,
            Stored {
                mode: "Ai".into(),
                skin: Some(Skin { title: "Chat".into(), simple: true }),
                log: vec![Turn::you_sending("summarise a.txt", "the file"), Turn::ai("ok")],
            },
        );
        save(Some(&path), &all);
        let back = load(Some(&path));
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].log[0].sent.as_deref(), Some("the file"), "the payload survived");
        assert_eq!(back[0].skin.as_ref().unwrap().title, "Chat");
    }

    #[test]
    fn a_missing_or_broken_file_is_no_history_rather_than_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(Some(&dir.path().join("nope.json"))).is_empty());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{not json").unwrap();
        assert!(load(Some(&bad)).is_empty());
        assert!(load(None).is_empty());
    }
}

/// Decode base64 (RFC 4648, the standard alphabet), for an image pasted into a
/// chat.
///
/// **Written here rather than pulled in.** cian is packaged for an offline
/// Windows machine — every crate it uses is vendored into the folder that gets
/// carried in — so one dependency for one well-understood function is a
/// re-vendor and a network trip on somebody else's schedule. The published
/// test vectors are below; that is what makes writing it out safe.
///
/// Whitespace is skipped, because a data URI that arrived through a JSON
/// document may have been wrapped. Anything else invalid gives `None` rather
/// than a guess: half an image is not an image.
pub fn from_base64(text: &str) -> Option<Vec<u8>> {
    fn six(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut pad = 0;
    for &c in text.as_bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            pad += 1;
            continue;
        }
        // Padding is the end of the data. A character after it is a stream
        // that disagrees with itself.
        if pad > 0 {
            return None;
        }
        acc = (acc << 6) | six(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    // Whatever is left over must be zero — the bits a shorter final group
    // pads with. Anything else means the text was truncated mid-byte.
    if pad > 2 || acc & ((1 << bits) - 1) != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod base64_tests {
    use super::from_base64;

    /// RFC 4648 §10, verbatim.
    #[test]
    fn the_published_vectors() {
        for (encoded, plain) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9vYmE=", "fooba"),
            ("Zm9vYmFy", "foobar"),
        ] {
            assert_eq!(
                from_base64(encoded).as_deref(),
                Some(plain.as_bytes()),
                "{encoded:?}",
            );
        }
    }

    #[test]
    fn every_byte_survives_the_round_trip() {
        // A PNG is bytes, not text: the pair that matters is 0x00..0xFF.
        let all: Vec<u8> = (0..=255u8).collect();
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut enc = String::new();
        for c in all.chunks(3) {
            let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
            let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
            for i in 0..4 {
                if i <= c.len() {
                    enc.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    enc.push('=');
                }
            }
        }
        assert_eq!(from_base64(&enc).as_deref(), Some(&all[..]));
    }

    #[test]
    fn wrapped_text_still_decodes() {
        assert_eq!(from_base64("Zm9v\n  YmFy\n").as_deref(), Some(&b"foobar"[..]));
    }

    /// Half an image is not an image.
    #[test]
    fn what_is_not_base64_is_nothing_rather_than_a_guess() {
        assert_eq!(from_base64("Zm9v!"), None, "a character outside the alphabet");
        assert_eq!(from_base64("Zm9v=Zm9v"), None, "data after the padding");
        assert_eq!(from_base64("Zh=="), None, "bits left over that are not zero");
    }
}
