//! The words cian says to a model, in one place.
//!
//! **They were written twice.** Every AI feature exists in both front ends and
//! each carried its own copy of the prompt — four of them, a paragraph each,
//! diverging quietly the way two copies of anything do. `scripts/parity.py`
//! exists because the two builds drifted on the *labels* people read; the
//! prompts are the labels the model reads, and nothing was checking those at
//! all. Sharing them removes the question rather than answering it.
//!
//! They live in `cian-core` because they are text and a rule, and depend on
//! nothing.
//!
//! Written as raw strings with real line breaks. A prompt is read far more
//! often by a person deciding whether it is right than by the program, and a
//! wall of `\` continuations is not readable.





/// Write one command line, for wherever that shell actually is.
///
/// `target` comes from [`crate::shellwhere::describe`] — the terminal title,
/// an `ssh` line in the scrollback, the shape of the prompt, and what
/// `init.lua` records about a known host. The wording of the first paragraph
/// and the three bullets is the terminal build's, which had all of this while
/// the window build had none.
///
/// The refusal clause is the part that earns its place. Asked for something
/// that is not one command, a model that must answer with a command answers
/// with a command — and a plausible half-right shell line is worse than
/// nothing, because it looks like the thing you asked for.
pub fn cmd(target: &str) -> String {
    format!(
        r#"Translate the user's request into ONE command line to run in {target}.

The command is pasted into that shell exactly as written and run there, so:
- Do NOT wrap it in `ssh` and do NOT add a hostname or any login/connection step — the shell is already at the right place.
- Do NOT `cd` somewhere else; it already runs where it runs.
- Use the command style and flags native to that system, and quote paths with spaces the way that shell quotes them.
- Output ONLY the command — no explanation, no markdown, no code fences, no leading prompt character.

The directory listing is there so you can use the real names. Prefer naming files over a wildcard when the listing shows you exactly which ones are meant.

**A remote pane is not the shell's disk.** When the pane is a remote listing over SFTP, the shell is still wherever it is — usually the local machine — so a command naming those files would run against paths that are not there. If the task is about the pane's files and the shell cannot reach them, refuse and say which two machines are involved.

**Two refusals, and they matter more than being helpful.** If the task cannot be done as one command line, answer with a single line beginning `# ` that says so in one sentence — do not invent a command that half does it. And never fold a destructive step (delete, overwrite, force-push, reset --hard) into a command that was asked to do something else; if deleting is what was asked for, write it plainly and alone so it can be read before it is run."#
    )
}

/// Draft a commit message from the staged diff.
pub const COMMIT: &str = r#"You write a git commit message for the given staged diff. Use the Conventional Commits style: a concise subject line under ~70 characters (an optional type prefix like feat:/fix:/refactor: is fine), then a blank line and a short body of bullet points explaining WHY, only if it adds something. Output ONLY the commit message — no code fences, no preamble."#;

/// Explain what went wrong in the shell panel.
///
/// Takes the platform because the fix depends on it — "command not found" is
/// a PATH question on Linux and quite often an execution-policy one on
/// Windows. Use [`os_name`] so both builds name the same three.
pub fn shell_error(os: &str) -> String {
    format!(
        "You explain shell/terminal errors for a developer on {os}. Given the recent terminal output, say plainly what went wrong and the most likely fix (a command or a change). If there is no error, say the output looks fine. Be concise; plain text, no markdown headings."
    )
}

/// What to call this platform when telling a model about it. Three names, and
/// both builds say the same one — they had a copy of this each.
pub fn os_name() -> &'static str {
    if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    }
}

/// Triage a log file from its tail.
pub const LOG: &str = r#"You triage a log file for an operator (often RHEL/AIX or Oracle). From the tail below: list the errors and warnings that matter, each with its key line; note a rough timeline if the timestamps show one; then give the single most likely cause and the next thing to check. Ignore routine INFO noise. Be concise; plain text, no markdown headings."#;
