//! Where is that shell, actually?
//!
//! A shell panel starts as whatever cian launched, and then somebody runs
//! `ssh`, or `su`, or `bash` inside PowerShell — and from that moment the
//! panel is a different machine speaking a different language. Nothing tells
//! cian this happened, and asking would mean typing a command into somebody
//! else's prompt.
//!
//! So it reads the evidence that is already on the screen. Three signals, most
//! reliable first: the terminal title (`taketan@web01: ~/proj`), an `ssh …`
//! command still visible in the scrollback (AIX and ksh often never set a
//! title), and the shape of the prompt itself. Any of them can be wrong; none
//! of them costs anything.
//!
//! **This is the terminal build's, moved rather than rewritten.** It had all
//! three and the window build had none, so `:aicmd` in the window wrote
//! PowerShell for a Linux server it was logged into — and the answer is
//! *placed at that prompt*, which is the one place it could not run.

/// The host from a `user@host` terminal title.
pub fn host_from_title(title: &str) -> Option<String> {
    let after_at = title.split('@').nth(1)?;
    // The host runs up to the first `:`, space, or slash.
    let host: String = after_at
        .chars()
        .take_while(|c| !matches!(c, ':' | ' ' | '/' | '\t'))
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

pub fn ssh_host_from_screen(screen: &str) -> Option<String> {
    // Short flags that consume the next token, so its argument isn't mistaken
    // for the target host (`ssh -p 2222 aix1`).
    const ARGFLAGS: &[&str] = &[
        "-p", "-i", "-l", "-o", "-F", "-J", "-b", "-c", "-D", "-e", "-L", "-R", "-W", "-w", "-m",
        "-O", "-Q", "-S",
    ];
    let mut found = None;
    for line in screen.lines() {
        let bytes = line.as_bytes();
        let mut from = 0;
        while let Some(rel) = line[from..].find("ssh ") {
            let start = from + rel;
            from = start + 4;
            if start != 0 && !bytes[start - 1].is_ascii_whitespace() {
                continue; // part of a longer word, e.g. "passh "
            }
            let toks: Vec<&str> = line[start + 4..].split_whitespace().collect();
            // A `user@host` token is unambiguous; otherwise the first non-flag,
            // non-flag-argument token is the target.
            let mut target = toks.iter().find(|t| t.contains('@') && !t.starts_with('-')).copied();
            if target.is_none() {
                let mut skip = false;
                for tok in &toks {
                    if skip {
                        skip = false;
                        continue;
                    }
                    if tok.starts_with('-') {
                        if ARGFLAGS.contains(tok) {
                            skip = true;
                        }
                        continue;
                    }
                    target = Some(tok);
                    break;
                }
            }
            if let Some(t) = target {
                let h = t.rsplit('@').next().unwrap_or(t);
                let h = h.split(':').next().unwrap_or(h);
                if !h.is_empty() && h.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_')) {
                    found = Some(h.to_string());
                }
            }
        }
    }
    found
}

pub fn shell_looks_unix(screen: &str) -> Option<bool> {
    if screen.contains("PS ") && screen.contains(":\\") {
        return Some(false);
    }
    if screen.contains("Microsoft Windows") || screen.contains("C:\\") || screen.contains("\\Windows\\") {
        return Some(false);
    }
    for line in screen.lines() {
        let t = line.trim_end();
        if t.ends_with('$') || t.ends_with('#') {
            return Some(true); // a typical Unix prompt line
        }
    }
    if ["/usr/", "/home/", "/etc/", "/var/", "/opt/"].iter().any(|p| screen.contains(p)) {
        return Some(true);
    }
    None
}


/// 打ちかけの行を消すバイト列 ── **コマンドを打ち込む前に送るもの。**
///
/// cian がシェルにコマンドを書くとき、プロンプトに**打ちかけの字が残っている**
/// ことがある。書き込みはいまの行の後ろに足すので、`clear` と打って Enter を
/// 押さないまま「右クリック ▸ SSH接続」を選ぶと、シェルは `clearssh user@host`
/// を受け取って走らせる（本物の pty で再現した。`zsh: command not found:
/// clearecho`）。**打ち込む側が、行を空にしてから書く。**
///
/// `Ctrl+E` で行末へ、`Ctrl+U` で行頭まで削る。readline（bash）・zle（zsh）・
/// PSReadLine（PowerShell）のどれでも、この2つで行が空になる。`cmd.exe` だけは
/// その2つを知らず、行を消すのは `Esc` なので分ける。
///
/// **`Ctrl+C` にしていない。** どのシェルでも行を捨てられるが、前面で何かが
/// 走っていればそれを殺す。行を削るバイトなら、走っているプログラムの標準入力へ
/// 流れても何も壊さない。判定を外したときの被害が軽いほうを選ぶ。
///
/// `screen` は pty の見えている中身。`ssh` した先が Unix なら、手元の `cmd.exe`
/// ではなく**向こうの shell が相手**なので、画面の判定を先に効かせる。
pub fn line_clear(started: &str, screen: Option<&str>) -> &'static [u8] {
    const KILL: &[u8] = b"\x05\x15"; // Ctrl+E, Ctrl+U
    const ESC: &[u8] = b"\x1b";
    if screen.and_then(shell_looks_unix) == Some(true) {
        return KILL;
    }
    let name = started.rsplit(['/', '\\']).next().unwrap_or(started).to_ascii_lowercase();
    if name == "cmd.exe" || name == "cmd" {
        ESC
    } else {
        KILL
    }
}

/// What to tell a model about where its command will run.
///
/// `notes` is what `init.lua` records about a known host — "AIX 7.2, ksh, no
/// GNU coreutils" — and it is the only place the *far* machine's OS can come
/// from at all: SFTP does not report it and neither does a prompt.
pub fn describe(
    title: Option<&str>,
    screen: Option<&str>,
    known: impl Fn(&str) -> Option<String>,
    local_os: &str,
    local_shell: &str,
) -> String {
    let host = title
        .and_then(host_from_title)
        .or_else(|| screen.and_then(ssh_host_from_screen));
    match host {
        Some(h) => match known(&h) {
            Some(notes) => format!(
                "a shell already logged in over SSH to the server '{h}'. \
                 That system: {notes}. Use that system's own commands and flags \
                 (AIX / Solaris / HP-UX differ from GNU/Linux)."
            ),
            None => format!(
                "a Unix-like shell on '{h}' (it may be a remote server reached over \
                 SSH). Use POSIX / Unix commands, not Windows ones."
            ),
        },
        // No host name, but the shell clearly is not a Windows one — a
        // Unix/POSIX session, very likely SSH'd into a server.
        None if screen.and_then(shell_looks_unix) == Some(true) => {
            "a Unix / POSIX shell (it looks like a remote or Unix session). Use ls-style \
             POSIX commands, not Windows (`dir`/PowerShell) ones."
                .to_string()
        }
        None => format!("your local {local_os} {local_shell} shell"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **打ちかけを消す鍵はシェルで違う。** 外すと、消えないか、別の字が入る。
    #[test]
    fn the_line_is_cleared_with_what_that_shell_understands() {
        let kill = b"\x05\x15".as_slice();
        let esc = b"\x1b".as_slice();
        // readline / zle / PSReadLine
        assert_eq!(line_clear("zsh", None), kill);
        assert_eq!(line_clear("/bin/bash", None), kill);
        assert_eq!(line_clear("powershell.exe", None), kill);
        assert_eq!(
            line_clear("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", None),
            kill
        );
        // cmd.exe だけ Esc
        assert_eq!(line_clear("cmd.exe", None), esc);
        assert_eq!(line_clear("C:\\Windows\\System32\\cmd.exe", None), esc);
        // **Ctrl+C は送らない。** 前面のプログラムを殺さないため
        assert!(!line_clear("zsh", None).contains(&0x03));
        assert!(!line_clear("cmd.exe", None).contains(&0x03));
    }

    /// `ssh` した先が相手。手元が `cmd.exe` でも、向こうが Unix なら Unix の鍵。
    #[test]
    fn the_far_shell_wins_over_the_one_cian_started() {
        let kill = b"\x05\x15".as_slice();
        let esc = b"\x1b".as_slice();
        assert_eq!(line_clear("cmd.exe", Some("taketan@web01:~/proj$ ")), kill);
        // PowerShell と読めても Ctrl+U は効く。cmd.exe だけが Esc
        assert_eq!(line_clear("zsh", Some("PS C:\\Users\\t> ")), kill);
        assert_eq!(line_clear("cmd.exe", Some("PS C:\\Users\\t> ")), esc);
    }


    #[test]
    fn a_title_names_its_host() {
        assert_eq!(host_from_title("taketan@web01: ~/proj"), Some("web01".into()));
        assert_eq!(host_from_title("root@db-server:/var"), Some("db-server".into()));
        assert_eq!(host_from_title("just a title"), None);
    }

    /// The prompt on an AIX box never sets a title, so the `ssh` line in the
    /// scrollback is the only thing left to read.
    #[test]
    fn the_ssh_line_names_its_host() {
        assert_eq!(ssh_host_from_screen("$ ssh admin@aix1\n").as_deref(), Some("aix1"));
        assert_eq!(ssh_host_from_screen("$ ssh -p 2222 aix1 uptime\n").as_deref(), Some("aix1"));
        assert_eq!(ssh_host_from_screen("$ ssh -i ~/k user@10.0.0.9\n").as_deref(), Some("10.0.0.9"));
        assert_eq!(ssh_host_from_screen("$ ssh -p 2222 aix1\n"), Some("aix1".into()),
                   "the port's argument is not the host");
        assert_eq!(ssh_host_from_screen("$ passh me@x\n"), None, "not part of a longer word");
        // The *most recent* one: a session that hopped twice is on the second.
        assert_eq!(ssh_host_from_screen("$ ssh one\n$ ssh two\n"), Some("two".into()));
    }

    #[test]
    fn windows_signals_win() {
        assert_eq!(shell_looks_unix("PS C:\\Users\\t> "), Some(false));
        assert_eq!(shell_looks_unix("taketan@web01:~$ "), Some(true));
        // A Unix-looking `$` inside Windows output must not flip it.
        assert_eq!(shell_looks_unix("Microsoft Windows [Version 10]\nx$"), Some(false));
        assert_eq!(shell_looks_unix("hello"), None);
        assert_eq!(shell_looks_unix("root@aix1:/home/app #"), Some(true));
        assert_eq!(shell_looks_unix("looking in /usr/local/bin"), Some(true), "a path is a signal too");
    }

    /// The sentence itself: a known host contributes the far machine's OS,
    /// which nothing else can supply.
    #[test]
    fn a_known_host_brings_its_own_os() {
        let notes = |h: &str| (h == "aix1").then(|| "AIX 7.2, ksh".to_string());
        let said = describe(Some("t@aix1: /home"), None, notes, "Windows", "powershell.exe");
        assert!(said.contains("AIX 7.2, ksh"), "{said}");
        assert!(said.contains("aix1"));

        let said = describe(None, Some("PS C:\\> "), |_| None, "Windows", "powershell.exe");
        assert!(said.contains("your local Windows powershell.exe shell"), "{said}");

        let said = describe(None, Some("t@box:~$ "), |_| None, "Windows", "powershell.exe");
        assert!(said.contains("POSIX"), "{said}");
    }

    /// **No title, and the host is only in the scrollback.** This is the AIX
    /// case the fallback exists for — ksh never sets a title — and it is the
    /// one `describe` was not testing: removing the fallback left every test
    /// passing.
    #[test]
    fn the_scrollback_alone_is_enough() {
        let notes = |h: &str| (h == "aix1").then(|| "AIX 7.2, ksh".to_string());
        let said = describe(None, Some("$ ssh admin@aix1\nadmin@aix1 $ "), notes, "Windows", "powershell.exe");
        assert!(said.contains("AIX 7.2, ksh"), "the host came from the ssh line: {said}");
    }
}
