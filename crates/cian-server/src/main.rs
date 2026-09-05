//! The engine, spoken over a pipe.
//!
//! cian's file manager lives in [`cian_core`] and knows nothing about how it is
//! drawn — the terminal front end proves that, and this is the second caller.
//! One JSON object per line in, one per line out, over stdin and stdout.
//!
//! **A line each way, not a stream.** The protocol has to be readable in a log
//! and typeable by hand when something is wrong at a customer's desk, and both
//! of those rule out anything framed by byte counts.
//!
//! ```text
//! → {"id":1,"method":"list","params":{"pane":"left","path":"/tmp"}}
//! ← {"id":1,"ok":{"cwd":"/tmp","entries":[…]}}
//! ← {"id":1,"error":"no such directory: /nope"}
//! ```
//!
//! Every reply carries the `id` it answers, so the caller may have several in
//! flight. A long operation also speaks unasked, on lines carrying `event`
//! instead of `id` — see [`wire`].

use std::io::BufRead;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use cian_core::Pane;

mod find;
mod jobs;
mod shell;
mod undo;
mod wire;

use find::Find;
use jobs::{Jobs, Kind};
use undo::{Stack, Undo};
use wire::Out;

/// One call from the front end.
#[derive(Deserialize)]
struct Request {
    id: u64,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

/// What a pane looks like to whoever is drawing it.
///
/// Deliberately not `cian_core::Pane` itself: that carries the undo stacks, the
/// history and the marks as a set, and a front end needs none of it to paint a
/// listing. Sending the whole thing would make every field of the engine part
/// of the protocol.
#[derive(Serialize)]
struct PaneView {
    cwd: String,
    entries: Vec<Row>,
    cursor: usize,
    /// How many rows are marked. The front end could count them, but this is
    /// the number it puts on the status line and counting is the engine's job.
    marked: usize,
    /// Whether dotfiles are showing. The switches menu puts the current value
    /// beside the name, so it has to come from the engine rather than from
    /// whatever the front end last remembered asking for.
    hidden_shown: bool,
    /// This side's tabs — one crumb each — and which is showing.
    tabs: Vec<String>,
    tab: usize,
    /// The filter narrowing this listing, when one is. Esc has to know
    /// whether there is anything to clear, and guessing from the row count
    /// cannot tell "filtered to 3" from "a folder with 3 things in it".
    filter: String,
    /// The archive this pane is looking inside, if it is. The window needs it
    /// for the same reason it needs `remote`: the rows name nothing on this
    /// disk, so opening one has to go a different way.
    archive: Option<String>,
    /// `user@host` when this pane is showing a server. The window needs it to
    /// know that Enter, `..` and `c` all mean something over the network.
    remote: Option<String>,
    /// …and *where* on it. The window used to name `cwd` when it asked
    /// "copy 3 → here?", which on a remote pane is the local directory it
    /// walked in from — so the question described a copy that was not the one
    /// about to happen.
    remote_path: Option<String>,
    /// The label of the flat listing showing here, if one is — a branch view
    /// or a panelized search. The window needs it to know that Esc means
    /// "back to the directory" rather than "nothing to cancel".
    flat: Option<String>,
    /// How this pane is ordered. Sorting is per pane in the core; a window
    /// that remembers one global "current sort" describes the wrong pane the
    /// moment the other one is sorted — its picker cursor and the ▲▼ in the
    /// column header both lied after a Tab.
    sort_key: &'static str,
    sort_reverse: bool,
}

/// One line of a listing.
#[derive(Serialize)]
struct Row {
    name: String,
    path: String,
    is_dir: bool,
    len: u64,
    /// Seconds since the epoch, or `null` where the filesystem has no opinion.
    modified: Option<u64>,
    /// Listed but not downloaded — reading it would pull it over the network.
    cloud: bool,
    /// The synthetic `..` row: navigable, never a target.
    parent: bool,
    marked: bool,
}

impl PaneView {
    /// The active tab of a side, with the side's tab strip attached.
    fn of_side(side: &Side) -> PaneView {
        let mut v = PaneView::of(side.get());
        v.tabs = side
            .tabs
            .iter()
            .map(|p| {
                p.flat_label()
                    .map(str::to_string)
                    .or_else(|| p.remote_view().map(|(h, _)| h.to_string()))
                    .unwrap_or_else(|| {
                        p.cwd
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| p.cwd.display().to_string())
                    })
            })
            .collect();
        v.tab = side.at;
        v
    }

    fn of(pane: &Pane) -> Self {
        PaneView {
            cwd: pane.cwd.display().to_string(),
            cursor: pane.cursor,
            marked: pane.mark_count(),
            hidden_shown: pane.show_hidden,
            sort_key: pane.sort.key.label(),
            sort_reverse: pane.sort.reverse,
            flat: pane.flat_label().map(str::to_string),
            // Filled in by `of_side`; a bare pane does not know its siblings.
            tabs: Vec::new(),
            tab: 0,
            remote: pane.remote_view().map(|(host, _)| host.to_string()),
            remote_path: pane.remote_view().map(|(_, path)| path.to_string()),
            archive: pane.archive_view().map(|(a, _)| a.display().to_string()),
            filter: pane.filter.clone(),
            entries: pane
                .entries
                .iter()
                .map(|e| Row {
                    name: e.name.clone(),
                    path: e.path.display().to_string(),
                    is_dir: e.is_dir,
                    len: e.len,
                    modified: e.modified.and_then(|t| {
                        t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
                    }),
                    cloud: e.cloud,
                    parent: e.is_parent,
                    marked: pane.marks.contains(&e.path),
                })
                .collect(),
        }
    }
}

/// The two panes and whatever is running over them.
/// One tab of the shell panel: its layout, and which pane has the keyboard.
struct ShellTab {
    root: shell::Node,
    focus: u64,
    /// Shift+F12 — this tab is showing only its focused pane, full size.
    /// The others keep running; they are hidden, not paused.
    zoom: bool,
    /// Type into every pane of this tab at once.
    ///
    /// The reason splits exist for a lot of people: four servers side by side
    /// and the same command on all four. Per tab rather than global, because
    /// the tab you built for that is not the tab you keep a shell in.
    sync: bool,
    /// Which panes hear the synced input. Empty means "all of them", which is
    /// what a fresh sync starts as — the subset is for the case where one of
    /// the four is the box you must not type into. cian-tui's
    /// `Shells::sync_members`, with the same rule: turning sync off clears it,
    /// so the next sync starts as all again.
    sync_members: std::collections::BTreeSet<u64>,
    /// What this tab is *for*, when it has been said. Empty means the tab
    /// shows its number instead.
    ///
    /// Four tabs called `shell 1`..`shell 4` are four tabs you have to open
    /// to tell apart, and the whole reason for a second one is that the first
    /// is busy with something in particular. `Aserver` answers that at a
    /// glance; `shell 2` never can.
    name: String,
}

/// One side of the window, and the tabs it holds.
///
/// The active tab *is* the pane as far as everything else is concerned —
/// `pane_mut` hands back a `&mut Pane` exactly as it always did, so the forty
/// call sites that operate on "this pane" did not have to learn what a tab is.
/// Only the handful that switch or close one know there is a list.
struct Side {
    tabs: Vec<Pane>,
    at: usize,
}

impl Side {
    fn new(pane: Pane) -> Self {
        Side { tabs: vec![pane], at: 0 }
    }

    fn now(&mut self) -> &mut Pane {
        let at = self.at.min(self.tabs.len().saturating_sub(1));
        &mut self.tabs[at]
    }

    fn get(&self) -> &Pane {
        &self.tabs[self.at.min(self.tabs.len().saturating_sub(1))]
    }
}

struct Session {
    left: Side,
    right: Side,
    jobs: Jobs,
    out: Out,
    /// A counter for transfer progress, kept apart from the job queue's.
    /// SFTP does not go through that queue — see the note in `transfer`.
    transfer_op: u64,
    undo: Stack,
    find: Find,
    /// Held for a later paste. Independent of the system clipboard, and of
    /// which pane is focused — that is the point of it.
    clip: Option<cian_core::clip::Clipboard>,
    /// The input source the person was typing with before cian switched it
    /// off for normal mode. Held so `restore` puts back *their* choice, not a
    /// guess.
    ime_saved: Option<String>,
    /// Ceiling on transfer speed, bytes a second. A copy to a server is a
    /// copy over somebody else's network, and a file manager that takes all
    /// of it is one you cannot run during the day.
    limit_bps: Option<u64>,
    /// A file on the server, opened by downloading it: the connection, the
    /// remote path, and where the copy is. Same shape and same reason as the
    /// archive member below.
    remote_member: Option<(cian_scp::Target, String, std::path::PathBuf)>,
    /// A member of an archive, opened by extracting it: which archive, which
    /// member, and where the copy is. Kept so a save knows where to put it
    /// back — a temporary file with no idea where it came from is a file that
    /// can only be lost.
    member: Option<(std::path::PathBuf, String, std::path::PathBuf)>,
    /// The right-hand file of a side-by-side comparison, when one is up.
    /// `open` holds the left; both are kept so a save on either goes back
    /// through the encoding that side arrived with.
    pair: Option<(std::path::PathBuf, cian_core::grepedit::TextFile)>,
    /// The file open in the viewer, as `view_file` read it — kept whether it
    /// is text or a binary, because re-decoding needs the raw bytes and a
    /// second read would be a second answer to "what is in this file".
    shown: Option<(std::path::PathBuf, cian_core::viewer::View)>,
    /// A binary open in the hex editor, as it was read plus whatever has been
    /// overwritten. Separate from `open` because the two save differently:
    /// text goes back through its encoding, bytes go back as bytes.
    hex: Option<(std::path::PathBuf, cian_core::viewer::View)>,
    /// The file the viewer has open, as it was read.
    ///
    /// A save writes back through this rather than through anything the front
    /// end says, so it cannot land on another path and cannot lose the
    /// encoding, BOM or line ending the file arrived with. Getting those wrong
    /// turns a one-line edit into a diff on every line — and on a Shift_JIS
    /// log, into a file the tool that wrote it can no longer read.
    /// …and what the file looked like when it was read, so a save can tell
    /// whether it is still writing over the thing it opened. Kept in the same
    /// place as the path and the text rather than beside them: three facts
    /// about one file, and a stamp that can fall out of step with the path it
    /// belongs to is worse than no stamp.
    open: Option<(
        std::path::PathBuf,
        cian_core::grepedit::TextFile,
        Option<cian_core::stamp::Stamp>,
    )>,
    /// Re-read and checksum every file after an SFTP transfer.
    ///
    /// Off by default, as in cian-tui: it doubles the traffic, and the answer
    /// it gives is only worth that on a link you do not trust.
    verify_transfers: bool,
    /// What `u` has taken back, waiting for Ctrl+R.
    ///
    /// Cleared by anything else that pushes onto the undo stack: once you have
    /// done something new, the branch you undid is gone. A redo stack that
    /// survives that puts files back on top of work done since.
    redo: Stack,
    /// The shell panel's tabs, and which one is showing.
    ///
    /// Started on demand rather than at launch: most sessions never open the
    /// panel, and a shell process per window that nobody asked for is a
    /// process nobody accounts for. More than one because a long build in tab
    /// one is the reason you want tab two.
    /// Every shell alive, whichever tab or split it belongs to.
    shells: Vec<shell::Shell>,
    /// One tree per tab: how that tab's shells are arranged, and which of them
    /// has the keyboard.
    tabs: Vec<ShellTab>,
    shell_at: usize,
    shell_next: u64,
    /// Where a remote pane is connected, and how.
    ///
    /// Held rather than asked for each time: SFTP wants a password, and a file
    /// manager that asks again for every directory you walk into is one nobody
    /// uses twice. Kept only in memory — never written anywhere.
    remotes: std::collections::HashMap<String, cian_scp::Target>,
}

impl Session {
    fn new(dir: std::path::PathBuf, out: Out) -> anyhow::Result<Self> {
        Ok(Session {
            // A high start, so a transfer's op can never collide with a job
            // queue id — the window keys its progress bar on the number.
            transfer_op: 1_000_000,
            left: Side::new(Pane::new(dir.clone())?),
            right: Side::new(Pane::new(dir)?),
            jobs: Jobs::default(),
            out,
            undo: Stack::default(),
            find: Find::default(),
            clip: None,
            open: None,
            shown: None,
            pair: None,
            member: None,
            remote_member: None,
            ime_saved: None,
            hex: None,
            redo: Stack::default(),
            // init.lua has the say; the toggle moves it for this session.
            // cian-tui reads the same two options (lib.rs:4971 for the cloud
            // one, which is process-wide in cian-core).
            verify_transfers: {
                let cfg = cian_lua::load();
                cian_core::cloud::set_include(cfg.options.read_cloud_files.unwrap_or(false));
                cfg.options.verify_transfers.unwrap_or(false)
            },
            // `cian.set_option("transfer_limit", "2M")`. cian-tui reads it at
            // startup (lib.rs:3095) and the window ignored it entirely, so a
            // config written to be kind to somebody's network was kind in
            // only one of the two builds.
            limit_bps: cian_lua::load().options.transfer_limit.as_deref()
                .and_then(cian_core::parse_rate),
            shells: Vec::new(),
            tabs: Vec::new(),
            shell_at: 0,
            shell_next: 1,
            remotes: std::collections::HashMap::new(),
        })
    }

    /// The paths an operation acts on: the marked rows, or the one under the
    /// cursor when nothing is marked. Never the `..` row — it is navigable but
    /// is not a thing to copy.
    /// Where the front end says its cursor is.
    ///
    /// **The front end owns the cursor.** It moves on every `j` without asking,
    /// because a round trip per keystroke to redraw one highlighted row would
    /// be absurd — but that left the engine's own idea of it only being
    /// updated by `enter` and `mark`. Three presses of `j` and then `r`
    /// renamed whatever had been under the cursor three rows ago.
    ///
    /// So every request that names a pane states the cursor too, and it is
    /// taken here, once, rather than in each of the handlers that consult it.
    fn take_cursor(&mut self, req: &Request) {
        // Both panes, every time. `compare` needs the row under each cursor —
        // `=` is one key and the answer is what the two of them are pointing
        // at — and a request that could only state one of them made that
        // impossible to ask for.
        for which in ["left", "right"] {
            let Some(at) = req.params["cursors"][which].as_u64() else { continue };
            if let Ok(pane) = self.pane_mut(which) {
                // Clamped rather than trusted: the listing can have changed
                // under the front end between its last draw and this request.
                pane.cursor = (at as usize).min(pane.entries.len().saturating_sub(1));
            }
        }
    }

    /// The row under the cursor, which is never `..`.
    ///
    /// Four handlers had written this out, and the parent guard is the whole
    /// point of it: without it `r` renames the directory you are standing in
    /// and `view` tries to read it. One place, so the guard cannot be the one
    /// thing a fifth handler forgets.
    fn selected(&mut self, which: &str) -> anyhow::Result<(std::path::PathBuf, String, bool)> {
        let pane = self.pane_mut(which)?;
        let Some(e) = pane.entries.get(pane.cursor).filter(|e| !e.is_parent) else {
            anyhow::bail!("対象がありません");
        };
        Ok((e.path.clone(), e.name.clone(), e.is_dir))
    }

    /// Climb one level inside an archive; past the root, leave it and land on
    /// the archive file, which is where you were when you went in.
    fn archive_up(
        &mut self,
        which: &str,
        archive: &std::path::Path,
        sub: &str,
    ) -> anyhow::Result<serde_json::Value> {
        if sub.is_empty() {
            let dir = archive.parent().unwrap_or(archive).to_path_buf();
            let name = archive.file_name().map(|s| s.to_string_lossy().into_owned());
            let pane = self.pane_mut(which)?;
            *pane = Pane::new(dir)?;
            if let Some(name) = name {
                if let Some(i) = pane.entries.iter().position(|e| e.name == name) {
                    pane.cursor = i;
                }
            }
            return self.view(which);
        }
        // "a/b/" → "a/"; "a/" → "".
        let parent = sub
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|(head, _)| format!("{head}/"))
            .unwrap_or_default();
        let child = sub.trim_end_matches('/').rsplit('/').next().map(str::to_string);
        let members = cian_core::archive::list(archive)?;
        let rows = cian_core::archive::archive_rows(archive, &members, &parent);
        let pane = self.pane_mut(which)?;
        pane.enter_archive(archive.to_path_buf(), parent, rows);
        // Land on the directory just left, as real navigation does.
        if let Some(child) = child {
            if let Some(i) = pane.entries.iter().position(|e| e.name == child) {
                pane.cursor = i;
            }
        }
        self.view(which)
    }

    /// This pane's directory without borrowing the pane mutably — `list`
    /// needs it while it is still deciding what to do.
    fn pane_cwd(&self, which: &str) -> std::path::PathBuf {
        match which {
            "right" => self.right.get().cwd.clone(),
            _ => self.left.get().cwd.clone(),
        }
    }

    fn targets(&self, which: &str) -> anyhow::Result<Vec<std::path::PathBuf>> {
        let pane = match which {
            "left" => self.left.get(),
            "right" => self.right.get(),
            other => anyhow::bail!("no such pane: {other}"),
        };
        let marked: Vec<_> = pane
            .entries
            .iter()
            .filter(|e| !e.is_parent && pane.marks.contains(&e.path))
            .map(|e| e.path.clone())
            .collect();
        if !marked.is_empty() {
            return Ok(marked);
        }
        match pane.entries.get(pane.cursor).filter(|e| !e.is_parent) {
            Some(e) => Ok(vec![e.path.clone()]),
            None => Ok(Vec::new()),
        }
    }

    /// Where a transfer goes: the directory the other pane is showing. Two
    /// panes side by side, and you copy between them — that is the whole idea,
    /// and it is why the destination never has to be typed.
    /// A new step onto the undo stack — and the redo stack emptied, because
    /// once something new has happened the branch that was undone is gone.
    /// Every "something happened" site goes through here; only the undo/redo
    /// handler itself pushes raw, since its pushes are the walk, not a step.
    fn did(&self, step: Undo) {
        did_step(&self.undo, &self.redo, step);
    }

    fn other_cwd(&self, which: &str) -> std::path::PathBuf {
        match which {
            "left" => self.right.get().cwd.clone(),
            _ => self.left.get().cwd.clone(),
        }
    }

    fn pane_mut(&mut self, which: &str) -> anyhow::Result<&mut Pane> {
        Ok(self.side_mut(which)?.now())
    }

    /// This side, as the window wants it: the active tab's listing *and* the
    /// tab strip.
    ///
    /// Every reply that hands back a pane goes through here. Returning a bare
    /// pane worked until there were tabs, and then every operation would have
    /// quietly dropped the strip — a tab bar that vanishes when you rename a
    /// file is worse than no tab bar.
    fn view(&mut self, which: &str) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::to_value(PaneView::of_side(self.side_mut(which)?))?)
    }

    /// The pane with the keyboard, in the tab that is showing.
    fn shell_now(&mut self) -> Option<&mut shell::Shell> {
        let id = self.tabs.get(self.shell_at)?.focus;
        self.shells.iter_mut().find(|s| s.id == id).filter(|s| s.alive())
    }

    /// The panel as the window needs it: every pane of the showing tab, where
    /// it sits, and its screen.
    ///
    /// Places are worked out here because the tree is here. The window puts
    /// boxes where it is told; it does not need to know what a split is.
    fn shell_reply(&self) -> serde_json::Value {
        let Some(tab) = self.tabs.get(self.shell_at) else {
            return serde_json::json!({ "gone": true });
        };
        let mut places = Vec::new();
        if tab.zoom {
            // Zoomed: the focused pane is the whole panel and the rest are
            // off screen. Sizing follows automatically, because sizes come
            // from these boxes.
            places.push((tab.focus, 0.0, 0.0, 1.0, 1.0));
        } else {
            tab.root.places(0.0, 0.0, 1.0, 1.0, &mut places);
        }
        let panes: Vec<_> = places
            .iter()
            .filter_map(|(id, x, y, w, h)| {
                let sh = self.shells.iter().find(|s| s.id == *id)?;
                Some(serde_json::json!({
                    "id": id,
                    "x": x, "y": y, "w": w, "h": h,
                    "focused": *id == tab.focus,
                    "screen": sh.screen(),
                }))
            })
            .collect();
        // The dividers, so the window can put a handle on each one. Empty
        // while a tab is zoomed: there is one pane on screen and nothing to
        // divide.
        let mut cuts = Vec::new();
        if !tab.zoom {
            tab.root.dividers(0.0, 0.0, 1.0, 1.0, &mut cuts);
        }
        let dividers: Vec<_> = cuts
            .iter()
            .map(|(id, down, x, y, w, h)| serde_json::json!({
                "id": id, "down": down, "x": x, "y": y, "w": w, "h": h,
            }))
            .collect();
        serde_json::json!({
            "panes": panes,
            "dividers": dividers,
            "tabs": self.tabs.len(),
            "tab": self.shell_at,
            "showing": tab.focus,
            "sync": tab.sync,
            "names": self.tabs.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
        })
    }

    /// Start a shell and hand back its id.
    fn new_shell(&mut self, cwd: &std::path::Path, rows: u16, cols: u16) -> anyhow::Result<u64> {
        let id = self.shell_next;
        self.shell_next += 1;
        // `cian.set_option("shell", …)`, which the terminal build has honoured
        // since it had a shell panel and this one never did. Read per panel
        // rather than cached: `init.lua` is reloadable, and a shell opened
        // after a reload should be the shell that was just asked for.
        let program = cian_lua::load().options.shell.unwrap_or_else(cian_pty::default_shell);
        let sh = shell::Shell::start(id, cwd, rows, cols, &program, self.out.clone())?;
        self.shells.push(sh);
        Ok(id)
    }

    /// Forget every shell no tab still points at.
    fn prune_shells(&mut self) {
        let mut alive = Vec::new();
        for tab in &self.tabs {
            tab.root.leaves(&mut alive);
        }
        self.shells.retain(|s| alive.contains(&s.id));
    }

    fn side_mut(&mut self, which: &str) -> anyhow::Result<&mut Side> {
        match which {
            "left" => Ok(&mut self.left),
            "right" => Ok(&mut self.right),
            other => Err(anyhow::anyhow!("no such pane: {other}")),
        }
    }

    /// Answer one call. The error is a string because it is going to a person,
    /// through a dialog, not to code that will match on it.
    fn handle(&mut self, req: &Request) -> anyhow::Result<serde_json::Value> {
        self.take_cursor(req);
        match req.method.as_str() {
            // Both panes as they stand. What the front end asks for on startup
            // and after anything that could have changed the world.
            "state" => Ok(serde_json::json!({
                "left": PaneView::of_side(&self.left),
                "right": PaneView::of_side(&self.right),
            })),
            // Read a directory into a pane.
            "list" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let path = req.params["path"].as_str().map(|raw| {
                    // `~` and relative paths resolve against the *pane*, not
                    // against wherever the engine process happens to have been
                    // started. `is_dir()` on a bare ".." answers for the
                    // process's directory, which is a different place with the
                    // same name.
                    let p = std::path::PathBuf::from(shellexpand(raw));
                    // A converted SharePoint address is absolute wherever it
                    // means anything — but only Windows *knows* that
                    // `\\host@SSL\…` is a root, so off Windows it would be
                    // joined onto the pane's directory and the error would
                    // name a path nobody typed.
                    if p.is_absolute() || raw != cian_core::sharepoint::to_unc(raw) {
                        p
                    } else {
                        self.pane_cwd(&which).join(p)
                    }
                });
                // A SharePoint link that cannot become a path, said *before*
                // trying. "not a directory" would be true and useless — the
                // useful sentence is which kind of link it is and what to copy
                // instead. And a link that *is* right but will not open needs
                // the sign-in steps rather than a shrug.
                if let Some(raw) = req.params["path"].as_str() {
                    if let Some(why) = cian_core::sharepoint::refuse(raw) {
                        anyhow::bail!("{why}");
                    }
                }
                let sp = req.params["path"]
                    .as_str()
                    .map(|raw| raw != cian_core::sharepoint::to_unc(raw))
                    .unwrap_or(false);
                let pane = self.pane_mut(&which)?;
                if let Some(p) = path {
                    if !p.is_dir() {
                        if sp {
                            anyhow::bail!(
                                "{} を開けません{}",
                                p.display(),
                                cian_core::sharepoint::hint()
                            );
                        }
                        anyhow::bail!("not a directory: {}", p.display());
                    }
                    // `go_to`, not `Pane::new` — see its doc comment. Building
                    // a fresh pane here discarded the history, the marks, the
                    // sort and the hidden-file setting on every `:cd`, `z`,
                    // `o`, `O`, and on every F5.
                    pane.go_to(p)?;
                } else {
                    pane.reload()?;
                }
                self.view(&which)
            }
            // Step into whatever the cursor is on, or out to the parent.
            "enter" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let at = req.params["cursor"].as_u64().map(|n| n as usize);
                let pane = self.pane_mut(&which)?;
                if let Some(n) = at {
                    pane.cursor = n.min(pane.entries.len().saturating_sub(1));
                }
                // Inside an archive the rows are synthetic: their paths do
                // not exist on the disk, so the ordinary descent would look
                // for a directory that is not there. A directory row means
                // "list this prefix instead".
                if let Some((archive, sub)) = pane.archive_view() {
                    let (archive, sub) = (archive.to_path_buf(), sub.to_string());
                    let Some(row) = pane.entries.get(pane.cursor) else {
                        anyhow::bail!("対象がありません");
                    };
                    if row.is_parent {
                        return self.archive_up(&which, &archive, &sub);
                    }
                    if !row.is_dir {
                        // The window reads members itself (Enter and F3 both route to the
                        // extract-and-open path); reaching this line means an old front
                        // end, so the message says what to do rather than "not yet".
                        anyhow::bail!("アーカイブ内のファイルは F3 で開いてください");
                    }
                    let deeper = format!("{sub}{}/", row.name);
                    let members = cian_core::archive::list(&archive)?;
                    let rows = cian_core::archive::archive_rows(&archive, &members, &deeper);
                    let pane = self.pane_mut(&which)?;
                    pane.enter_archive(archive, deeper, rows);
                    return self.view(&which);
                }
                let was = pane.cwd.clone();
                pane.enter_selected()?;
                // Only if it actually went somewhere: `Enter` on a file will
                // one day open it, and that is not a step to walk back.
                if pane.cwd != was {
                    self.did(Undo::Navigated { pane: which.clone(), from: was });
                }
                self.view(&which)
            }
            "parent" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let pane = self.pane_mut(&which)?;
                if let Some((archive, sub)) = pane.archive_view() {
                    let (archive, sub) = (archive.to_path_buf(), sub.to_string());
                    return self.archive_up(&which, &archive, &sub);
                }
                let was = pane.cwd.clone();
                pane.go_parent()?;
                if pane.cwd != was {
                    self.did(Undo::Navigated { pane: which.clone(), from: was });
                }
                self.view(&which)
            }
            // Marking. `at` is a row; without it the cursor's row is meant.
            "mark" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let at = req.params["at"].as_u64().map(|n| n as usize);
                let pane = self.pane_mut(&which)?;
                let row = at.unwrap_or(pane.cursor);
                pane.toggle_mark_at(row);
                // Marking walks down the list, the way it does everywhere else:
                // one keystroke marks and moves on.
                if at.is_none() && pane.cursor + 1 < pane.entries.len() {
                    pane.cursor += 1;
                }
                self.view(&which)
            }
            "markall" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                mark_all(self.pane_mut(&which)?);
                self.view(&which)
            }
            "invert" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let pane = self.pane_mut(&which)?;
                for i in 0..pane.entries.len() {
                    pane.toggle_mark_at(i);
                }
                self.view(&which)
            }
            // Exactly these, and nothing else, marked.
            //
            // Visual selection re-marks from its anchor on every move, so it
            // needs to *state* the set rather than toggle towards it —
            // toggling would make an overshoot permanent instead of something
            // you correct by moving back.
            "setmarks" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let want: std::collections::HashSet<String> = req.params["paths"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str()).map(str::to_string).collect())
                    .unwrap_or_default();
                let pane = self.pane_mut(&which)?;
                pane.clear_marks();
                for i in 0..pane.entries.len() {
                    if want.contains(&pane.entries[i].path.display().to_string()) {
                        pane.set_mark_at(i);
                    }
                }
                self.view(&which)
            }
            "unmarkall" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let pane = self.pane_mut(&which)?;
                pane.clear_marks();
                self.view(&which)
            }
            // The operations. Each answers with the number it will report
            // under, before it has touched anything.
            "copy" | "move" | "delete" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                // Named paths where the caller has them, marks otherwise.
                //
                // A review screen has already decided which rows it means —
                // the dupes it ticked are not the pane's marks and must not
                // have to become them first. Marking them to delete them
                // would leave the marks behind on whatever survived.
                let named = paths_of(req);
                let paths = if named.is_empty() { self.targets(&which)? } else { named };
                if paths.is_empty() {
                    anyhow::bail!("nothing to operate on");
                }
                // **Does this cross the local/remote boundary?** A remote
                // pane's `cwd` still points at the directory it walked in
                // from, so an ordinary copy went *there* and reported "1
                // copied" while the server was never touched. Same shape as
                // the zip below, and the same silence.
                if matches!(req.method.as_str(), "copy" | "move") {
                    let other = if which == "left" { "right" } else { "left" };
                    let here_remote = self.pane_mut(&which)?.remote_view().is_some();
                    let there = {
                        let p = self.pane_mut(other)?;
                        (p.remote_view().map(|(_, path)| path.to_string()), p.cwd.clone())
                    };
                    let (there_remote, there_local) = there;
                    if here_remote || there_remote.is_some() {
                        let cut = req.method == "move";
                        let src_t = self.remotes.get(&which).cloned();
                        let dst_t = self.remotes.get(other).cloned();
                        let plan = match (here_remote, &there_remote) {
                            // Local → server.
                            (false, Some(dest)) => {
                                let Some(target) = dst_t else {
                                    anyhow::bail!("そのペインはサーバに繋がっていません")
                                };
                                // Directories go too now — `plan_upload`
                                // walks the tree and the job makes each
                                // folder before the files in it. They used to
                                // be filtered out here, because SFTP has no
                                // recursive put and half a folder is worse
                                // than none.
                                jobs::Remote::Up {
                                    target,
                                    files: paths.clone(),
                                    dest: dest.clone(),
                                }
                            }
                            // Server → local.
                            (true, None) => {
                                let Some(target) = src_t else {
                                    anyhow::bail!("このペインはサーバに繋がっていません")
                                };
                                jobs::Remote::Down {
                                    target,
                                    files: paths.iter().map(|p| p.display().to_string()).collect(),
                                    dest: there_local,
                                }
                            }
                            // Server → server, relayed through here.
                            (true, Some(dest)) => {
                                let (Some(src), Some(dst)) = (src_t, dst_t) else {
                                    anyhow::bail!("どちらかのペインがサーバに繋がっていません")
                                };
                                // **The same server twice is a rename, not a
                                // round trip.** Relaying a move between two
                                // directories on one machine would pull every
                                // byte down and push it back up to end where
                                // the server could have put it instantly —
                                // and would break the atomicity a rename has.
                                let same = src.host == dst.host
                                    && src.port == dst.port
                                    && src.user == dst.user;
                                if same && cut {
                                    let mut moved = 0usize;
                                    for p in &paths {
                                        let from = p.display().to_string();
                                        let name =
                                            from.rsplit('/').next().unwrap_or(&from).to_string();
                                        cian_scp::rename(
                                            &src,
                                            &from,
                                            &cian_scp::remote_join(dest, &name),
                                        )?;
                                        moved += 1;
                                    }
                                    return Ok(serde_json::json!({
                                        "renamed": moved, "remote": true,
                                    }));
                                }
                                jobs::Remote::Across {
                                    src,
                                    dst,
                                    files: paths.iter().map(|p| p.display().to_string()).collect(),
                                    dest: dest.clone(),
                                }
                            }
                            (false, None) => unreachable!("neither side is remote"),
                        };
                        let count = paths.len();
                        let (op, queued) = self.jobs.start_remote(plan, cut, self.limit_bps, self.out.clone());
                        return Ok(serde_json::json!({
                            "op": op, "count": count, "queued": queued, "remote": true,
                        }));
                    }
                }
                // **Is the other pane inside a zip?** `enter_archive` leaves
                // `cwd` alone — an archive view is synthetic, and the pane
                // still remembers the directory it walked in from. So the
                // copy went to *that* directory, dropped the file beside the
                // archive, and said "copied": right message, wrong place, no
                // way to tell from the screen. The terminal build has asked
                // "add to the zip?" since it could read one (actions.rs:92).
                if matches!(req.method.as_str(), "copy" | "move") {
                    let into = {
                        let other = self.pane_mut(if which == "left" { "right" } else { "left" })?;
                        other.archive_view().map(|(a, sub)| (a.to_path_buf(), sub.to_string()))
                    };
                    if let Some((archive, sub)) = into {
                        if req.method == "move" {
                            anyhow::bail!("zip へはコピー（追加）のみ — 移動は未対応");
                        }
                        if !zip_writable(&archive) {
                            anyhow::bail!("これは書き換えられない形式です");
                        }
                        // The window asks before this call, the way it asks
                        // before every other write; the engine answers with
                        // what it would do so the sheet can name it.
                        // What it *would* do, so the window can name it in
                        // the confirmation. Nothing is written until `zipadd`.
                        return Ok(serde_json::json!({
                            "zipadd": {
                                "archive": archive.display().to_string(),
                                "sub": sub,
                                "count": paths.len(),
                            }
                        }));
                    }
                }
                let (kind, dest) = match req.method.as_str() {
                    "copy" => (Kind::Copy, Some(self.other_cwd(&which))),
                    "move" => (Kind::Move, Some(self.other_cwd(&which))),
                    _ => (Kind::Delete, None),
                };
                // The window's answer to the confirmation, with the terminal
                // build's defaults: an existing destination is skipped unless
                // overwriting was asked for by name, and a delete goes to the
                // trash unless "permanent" was.
                let conflict = match req.params["conflict"].as_str() {
                    Some("overwrite") => cian_core::ops::Conflict::Overwrite,
                    _ => cian_core::ops::Conflict::Skip,
                };
                let mode = match req.params["mode"].as_str() {
                    Some("permanent") => cian_core::ops::DeleteMode::Permanent,
                    _ => cian_core::ops::DeleteMode::Trash,
                };
                let count = paths.len();
                let (op, queued) = self.jobs.start(
                    jobs::Plan { kind, conflict, delete: mode },
                    paths, dest, self.out.clone(),
                    self.undo.clone(), self.redo.clone(),
                );
                Ok(serde_json::json!({ "op": op, "count": count, "queued": queued }))
            }
            // Hold the selection for a later paste, and drop it somewhere
            // else. `c`/`m` go straight to the other pane; this is the other
            // half of the pair, for when the destination is not on screen yet.
            "clip" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let op = match req.params["op"].as_str() {
                    Some("cut") => cian_core::clip::Op::Cut,
                    _ => cian_core::clip::Op::Copy,
                };
                let paths = self.targets(&which)?;
                if paths.is_empty() {
                    anyhow::bail!("対象がありません");
                }
                let count = paths.len();
                self.clip = Some(cian_core::clip::Clipboard { paths, op });
                Ok(serde_json::json!({
                    "held": count,
                    "op": if op == cian_core::clip::Op::Cut { "cut" } else { "copy" },
                }))
            }
            "paste" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let dest = self.pane_mut(&which)?.cwd.clone();
                // The engine has no system clipboard of its own — the window
                // is the only thing here that can see one, and it has not been
                // asked for yet. `plan` takes the fallback as a closure for
                // exactly this: the day it can, it is one argument.
                let (paths, op) =
                    match cian_core::clip::plan(self.clip.as_ref(), Vec::new, &dest) {
                        cian_core::clip::Paste::Empty => {
                            anyhow::bail!("クリップボードは空です")
                        }
                        cian_core::clip::Paste::AlreadyHere => {
                            anyhow::bail!("既にこのディレクトリです")
                        }
                        cian_core::clip::Paste::Go { paths, op, .. } => (paths, op),
                    };
                let kind = if op == cian_core::clip::Op::Cut { Kind::Move } else { Kind::Copy };
                if !cian_core::clip::survives(op) {
                    self.clip = None;
                }
                let count = paths.len();
                // Skip, as the terminal build's paste does: what is already
                // there survives, and pasting again is cheap.
                let (job, queued) = self.jobs.start(
                    jobs::Plan::of(kind), paths, Some(dest), self.out.clone(),
                    self.undo.clone(), self.redo.clone(),
                );
                // Which it is, said back: the key pressed was "paste" either
                // way, and only the register knew whether that meant a copy.
                Ok(serde_json::json!({
                    "op": job,
                    "queued": queued,
                    "count": count,
                    "kind": if matches!(kind, Kind::Move) { "move" } else { "copy" },
                }))
            }
            // Hand the file to whatever the desktop opens it with. A
            // directory goes to the other pane instead, which is what the
            // terminal build's Ctrl+Enter does — one key, and the answer
            // depends on what is under the cursor.
            "open" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let other = if which == "left" { "right" } else { "left" };
                let (path, name, is_dir) = self.selected(&which)?;
                if is_dir {
                    let there = self.pane_mut(other)?;
                    *there = Pane::new(path)?;
                    let view = serde_json::to_value(PaneView::of(there))?;
                    return Ok(serde_json::json!({ "pane": other, "view": view, "name": name }));
                }
                cian_core::proc::open_with_desktop(&path)?;
                Ok(serde_json::json!({ "opened": name }))
            }
            // Read a file for the viewer.
            //
            // Decoding is the engine's job, not the window's. A browser reads
            // UTF-8 and nothing else, and half of what this meets on a
            // Japanese Windows machine is Shift_JIS — a log, a batch file,
            // something out of an old tool. Handing over raw bytes would mean
            // writing that detection a second time, in JavaScript.
            "view" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (path, name, is_dir) = self.selected(&which)?;
                if is_dir {
                    anyhow::bail!("{name} はディレクトリです");
                }
                let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                // `view_file` rather than `read_text`, because it answers for
                // everything: text in whatever encoding, a hex dump for a
                // binary, extracted text for an Office file or a PDF. The
                // narrower read refused a binary with "looks binary", which is
                // true and leaves the person with nothing.
                let shown = cian_core::viewer::view_file(&path)?;
                let binary = matches!(shown.kind, cian_core::viewer::ViewKind::Binary);
                // The editable copy is only fetched for real text: a hex dump
                // is a rendering, and saving one back would write the dump.
                let file = if binary {
                    None
                } else {
                    cian_core::grepedit::read_text(&path).ok()
                };
                let reply = serde_json::json!({
                    "name": name,
                    "path": path.display().to_string(),
                    "lines": shown.lines,
                    "bytes": len,
                    "binary": binary,
                    "truncated": shown.truncated,
                    "encoding": format!("{:?}", shown.encoding),
                    "eol": format!("{:?}", shown.eol),
                    "bom": shown.bom,
                    "lang": if binary {
                        None
                    } else {
                        cian_core::highlight::detect(&path).map(|l| format!("{l:?}"))
                    },
                });
                // Text and binary are remembered separately: one saves back
                // through its encoding, the other as bytes, and a single slot
                // would make `save` guess which it was holding.
                if binary {
                    self.open = None;
                    self.hex = Some((path.clone(), shown.clone()));
                } else {
                    self.hex = None;
                    self.open = file.map(|f| (path.clone(), f, cian_core::stamp::of(&path)));
                }
                self.shown = Some((path, shown));
                Ok(reply)
            }
            // Write the open file back, in the encoding it arrived in.
            "save" => {
                let Some((path, original, stamp)) = self.open.as_ref() else {
                    anyhow::bail!("開いているファイルがありません");
                };
                // **Is this still the file that was opened?**
                //
                // It used to write regardless — the encoding, the BOM and the
                // line endings were all carried faithfully back onto whatever
                // happened to be there now. On a shared folder (a synced
                // library, a SharePoint mount, an NFS home) two people saving
                // one note meant the second silently erased the first, with
                // nothing on screen to say so, because nothing had looked.
                //
                // Refused rather than merged: cian is not a merge tool and
                // pretending otherwise on somebody else's writing is worse
                // than stopping. The window offers overwriting, saving
                // elsewhere, or looking at the difference.
                let forced = req.params["force"].as_bool().unwrap_or(false);
                if !forced {
                    if let Some(st) = stamp {
                        if cian_core::stamp::changed(path, st) {
                            let said = cian_core::stamp::describe(path, st);
                            return Ok(serde_json::json!({
                                "conflict": said,
                                "path": path.display().to_string(),
                            }));
                        }
                    }
                }
                let lines: Vec<String> = lines_of(req).unwrap_or_default();
                let file = cian_core::grepedit::TextFile { lines, ..original.clone() };
                cian_core::grepedit::write_text(path, &file)?;
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let lines = file.lines.len();
                // The stamp is taken *after* the write, so the copy we now
                // hold is the one on disk.
                let st = cian_core::stamp::of(path);
                self.open = Some((path.clone(), file, st));
                Ok(serde_json::json!({ "saved": name, "lines": lines }))
            }
            // ---- What is here, measured rather than felt ----
            //
            // Every one of these already exists in cian-core, written and
            // tested for the terminal build. The engine's whole job is to let
            // the window ask; none of the answering happens here.
            "count" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let paths = self.targets(&which)?;
                let o = cian_core::count::Options::default();
                let r = cian_core::count::count(&paths, &o);
                Ok(serde_json::json!({
                    "files": r.total.files,
                    "steps": r.total.steps(&o),
                    "lines": r.total.total,
                    "blank": r.total.blank,
                    "comments": r.total.comment,
                    "truncated": r.truncated,
                    "by_ext": r.by_ext.iter().take(20).map(|(e, c)| serde_json::json!({
                        "ext": if e.is_empty() { "(拡張子なし)" } else { e },
                        "files": c.files,
                        "steps": c.steps(&o),
                    })).collect::<Vec<_>>(),
                }))
            }
            "attr" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (path, name, _) = self.selected(&which)?;
                let a = cian_core::attrs::read_attrs(&path)?;
                Ok(serde_json::json!({
                    "name": name,
                    "path": path.display().to_string(),
                    "mode": a.mode.map(|m| format!("{:o}", m & 0o7777)),
                    "readonly": a.readonly,
                    "owner": a.owner,
                    "size": a.size,
                    "is_dir": a.is_dir,
                }))
            }
            "chmod" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let spec = req.params["spec"].as_str().unwrap_or("").trim().to_string();
                if spec.is_empty() {
                    anyhow::bail!("モードを指定してください（例: 644）");
                }
                // On a server it is the same intention and a different call.
                // cian-tui's remote pane answers the local keys (A/a/r/d);
                // `:chmod` is the one that stopped at the boundary, so a mode
                // on a server meant leaving cian for a shell.
                if let Some(target) = self.remotes.get(&which).cloned() {
                    let mode = u32::from_str_radix(spec.trim_start_matches("0o"), 8)
                        .map_err(|_| anyhow::anyhow!("8進で書いてください（例: 644）"))?;
                    let paths = self.targets(&which)?;
                    for p in &paths {
                        cian_scp::chmod(&target, &p.display().to_string(), mode)?;
                    }
                    let pane = self.pane_mut(&which)?;
                    pane.reload()?;
                    return Ok(serde_json::json!({ "changed": paths.len(), "spec": spec }));
                }
                let paths = self.targets(&which)?;
                for p in &paths {
                    cian_core::attrs::set_mode(p, &spec)?;
                }
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                Ok(serde_json::json!({ "changed": paths.len(), "spec": spec }))
            }
            "readonly" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let on = req.params["on"].as_bool().unwrap_or(true);
                let paths = self.targets(&which)?;
                for p in &paths {
                    cian_core::attrs::set_readonly(p, on)?;
                }
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                Ok(serde_json::json!({ "changed": paths.len(), "on": on }))
            }
            // Checksums. Cancellable because a checksum of something large is
            // the one "quick look" in here that is not quick.
            "hash" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let kind = match req.params["kind"].as_str() {
                    Some("md5") => cian_core::attrs::HashKind::Md5,
                    _ => cian_core::attrs::HashKind::Sha256,
                };
                let paths = self.targets(&which)?;
                let stop = std::sync::atomic::AtomicBool::new(false);
                let mut rows = Vec::new();
                for p in paths.iter().take(200) {
                    // A directory has no checksum, and saying so beats the
                    // read error the caller would otherwise be handed.
                    if p.is_dir() {
                        rows.push(serde_json::json!({
                            "name": p.file_name().map(|s| s.to_string_lossy().into_owned()),
                            "sum": "(ディレクトリ)",
                        }));
                        continue;
                    }
                    let sum = cian_core::attrs::hash_file(p, kind, &stop)?;
                    rows.push(serde_json::json!({
                        "name": p.file_name().map(|s| s.to_string_lossy().into_owned()),
                        "sum": sum,
                    }));
                }
                Ok(serde_json::json!({ "kind": req.params["kind"].as_str().unwrap_or("sha256"), "rows": rows }))
            }
            // What is biggest here. On a worker with a cancel flag, because
            // pointed at a home directory it is minutes rather than seconds.
            "du" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let dir = match req.params["path"].as_str() {
                    Some(p) => std::path::PathBuf::from(p),
                    None => self.pane_mut(&which)?.cwd.clone(),
                };
                let stop = std::sync::atomic::AtomicBool::new(false);
                let rows = cian_core::du::analyze(&dir, &stop, &mut |_| {});
                Ok(serde_json::json!({
                    "cwd": dir.display().to_string(),
                    "rows": rows.iter().take(500).map(|e| serde_json::json!({
                        "name": e.name,
                        "path": e.path.display().to_string(),
                        "size": e.size,
                        "is_dir": e.is_dir,
                    })).collect::<Vec<_>>(),
                }))
            }
            // Find by name, or grep inside files. One method: the two differ
            // by a mode, and the pattern language — bare text is a literal,
            // /re/ is a regex, /re/i ignores case — is the same for both, so
            // splitting them would be two doors onto one room.
            "search" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let needle = req.params["needle"].as_str().unwrap_or("").to_string();
                if needle.is_empty() {
                    anyhow::bail!("探す文字列がありません");
                }
                let mode = match req.params["mode"].as_str() {
                    Some("content") => cian_core::search::Mode::Content,
                    _ => cian_core::search::Mode::Name,
                };
                let root = self.pane_mut(&which)?.cwd.clone();
                let query = cian_core::search::Query::parse(&needle, mode)
                    .map_err(|e| anyhow::anyhow!(e))?;
                let stop = std::sync::atomic::AtomicBool::new(false);
                let mut hits = Vec::new();
                let outcome = cian_core::search::search(&root, &query, &stop, &mut |h| {
                    if hits.len() < 2000 {
                        hits.push(serde_json::json!({
                            "path": h.path.display().to_string(),
                            "rel": h.rel.display().to_string(),
                            "is_dir": h.is_dir,
                            "line": h.line.as_ref().map(|(n, t)| serde_json::json!({
                                "n": n,
                                "text": t.chars().take(400).collect::<String>(),
                            })),
                        }));
                    }
                });
                Ok(serde_json::json!({
                    "root": root.display().to_string(),
                    "needle": needle,
                    "mode": if matches!(mode, cian_core::search::Mode::Content) { "content" } else { "name" },
                    "truncated": matches!(outcome, cian_core::search::Outcome::Truncated),
                    "hits": hits,
                }))
            }
            // Load a set of paths into a pane as if it were a listing.
            //
            // The terminal build calls it panelizing, and it is what makes a
            // search result useful rather than merely informative: the matches
            // become rows to mark and operate on with the keys already known.
            "panelize" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let paths: Vec<std::path::PathBuf> = paths_of(req);
                if paths.is_empty() {
                    anyhow::bail!("読み込むものがありません");
                }
                let label = req.params["label"].as_str().unwrap_or("結果").to_string();
                let pane = self.pane_mut(&which)?;
                let root = pane.cwd.clone();
                let entries: Vec<cian_core::Entry> = paths
                    .iter()
                    .map(|p| {
                        let rel = p.strip_prefix(&root).unwrap_or(p);
                        cian_core::Entry::flat(rel, p.clone(), p.is_dir())
                    })
                    .collect();
                pane.enter_flat(label, entries);
                self.view(&which)
            }
            // Everything below here, one row per file.
            //
            // Its own method rather than a search for nothing: a search wants
            // something to look for and is right to refuse an empty needle,
            // and "show me all of it" is a different question with a different
            // answer — directories are rows in a listing and noise in a branch.
            "branch" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let root = self.pane_mut(&which)?.cwd.clone();
                let stop = std::sync::atomic::AtomicBool::new(false);
                let query = cian_core::search::Query::new("");
                let mut entries = Vec::new();
                cian_core::search::search(&root, &query, &stop, &mut |h| {
                    if !h.is_dir && entries.len() < 20_000 {
                        entries.push(cian_core::Entry::flat(&h.rel, h.path, false));
                    }
                });
                let found = entries.len();
                let pane = self.pane_mut(&which)?;
                pane.enter_flat("ブランチ", entries);
                Ok(serde_json::json!({
                    "found": found,
                    "pane": self.view(&which)?,
                }))
            }
            // ---- Left against right ----
            //
            // One method for both, because `=` is one key. What is under the
            // two cursors decides: two files are compared line by line, two
            // directories recursively. Asking the window to work out which
            // would put the decision where the files are not.
            "compare" => {
                let (lp, ln, ld) = self.selected("left")?;
                let (rp, rn, rd) = self.selected("right")?;
                if ld != rd {
                    anyhow::bail!("{ln} と {rn} は種類が違います");
                }
                let stop = std::sync::atomic::AtomicBool::new(false);
                if ld {
                    let d = cian_core::dirdiff::compare(&lp, &rp, &stop, &mut |_| {});
                    let rows: Vec<_> = d.entries.iter().take(5000).map(|e| serde_json::json!({
                        "rel": e.rel.display().to_string(),
                        "is_dir": e.is_dir,
                        "status": match e.status {
                            cian_core::dirdiff::Status::OnlyLeft => "left",
                            cian_core::dirdiff::Status::OnlyRight => "right",
                            cian_core::dirdiff::Status::Differ => "differ",
                        },
                    })).collect();
                    return Ok(serde_json::json!({
                        "kind": "dirs",
                        "left": lp.display().to_string(),
                        "right": rp.display().to_string(),
                        "truncated": d.truncated,
                        "rows": rows,
                    }));
                }
                // With an encoding named, both sides are decoded that way
                // before being compared — cian-tui's `e` on this screen. Two
                // Shift_JIS files read as UTF-8 differ on every line that
                // holds a Japanese character, which is a comparison that says
                // nothing about the files.
                let d = match req.params["enc"].as_str() {
                    Some("utf8") => cian_core::diff::diff_files_with_encoding(
                        &lp, &rp, cian_core::viewer::TextEncoding::Utf8)?,
                    Some("sjis") => cian_core::diff::diff_files_with_encoding(
                        &lp, &rp, cian_core::viewer::TextEncoding::ShiftJis)?,
                    Some("utf16le") => cian_core::diff::diff_files_with_encoding(
                        &lp, &rp, cian_core::viewer::TextEncoding::Utf16Le)?,
                    Some("utf16be") => cian_core::diff::diff_files_with_encoding(
                        &lp, &rp, cian_core::viewer::TextEncoding::Utf16Be)?,
                    _ => cian_core::diff::diff_files(&lp, &rp)?,
                };
                // Folded to three lines of context. The whole file is right
                // for a file being read and wrong for a difference being
                // looked at: the point is what changed, and pages of identical
                // lines between two changes hide it.
                // …but not always: cian-tui's `f` unfolds them, for the times
                // the question is "what is around this change" rather than
                // "what changed". The window could only ever see the folded
                // view, so that key had nothing to toggle.
                let folded = if req.params["folded"].as_bool().unwrap_or(true) {
                    cian_core::diff::fold(&d.rows, 3)
                } else {
                    d.rows.clone()
                };
                let rows: Vec<_> = folded.iter().take(20_000).map(|r| match r {
                    cian_core::diff::Row::Same { left, right } => serde_json::json!({
                        "kind": "same", "ln": left.no, "rn": right.no,
                        "left": left.text, "right": right.text,
                    }),
                    cian_core::diff::Row::Changed { left, right } => serde_json::json!({
                        "kind": "changed", "ln": left.no, "rn": right.no,
                        "left": left.text, "right": right.text,
                    }),
                    cian_core::diff::Row::Removed { left } => serde_json::json!({
                        "kind": "removed", "ln": left.no, "left": left.text,
                    }),
                    cian_core::diff::Row::Added { right } => serde_json::json!({
                        "kind": "added", "rn": right.no, "right": right.text,
                    }),
                    cian_core::diff::Row::Skipped { lines } => serde_json::json!({
                        "kind": "skipped", "lines": lines,
                    }),
                }).collect();
                Ok(serde_json::json!({
                    "kind": "files",
                    "left": ln, "right": rn,
                    // The paths as well as the names: `>` and `<` copy one
                    // side over the other and need somewhere to copy to.
                    "left_path": lp.display().to_string(),
                    "right_path": rp.display().to_string(),
                    "added": d.added, "removed": d.removed, "changed": d.changed,
                    "truncated": d.truncated,
                    "summary": cian_core::diff::summary(&d),
                    "rows": rows,
                }))
            }
            // ---- Bulk rename ----
            //
            // The plan first, always. `:renamepattern` can rename a hundred
            // files, and the one thing that makes that safe is seeing the
            // hundred new names before any of them exists.
            "renameplan" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let pattern = req.params["pattern"].as_str().unwrap_or("").to_string();
                let paths = self.targets(&which)?;
                let names: Vec<String> = paths
                    .iter()
                    .map(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
                    .collect();
                let planned = cian_core::rename::plan_batch(&pattern, &names, Default::default())
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let rows: Vec<_> = names.iter().zip(planned.iter()).zip(paths.iter())
                    .map(|((from, to), p)| serde_json::json!({
                        "from": from, "to": to,
                        "path": p.display().to_string(),
                        "same": from == to,
                        "clash": p.with_file_name(to).exists() && from != to,
                    }))
                    .collect();
                Ok(serde_json::json!({ "pattern": pattern, "rows": rows }))
            }
            "renameapply" => {
                let pairs = req.params["rows"].as_array().cloned().unwrap_or_default();
                let mut done = 0usize;
                let mut errors: Vec<String> = Vec::new();
                for row in &pairs {
                    let (Some(path), Some(to)) = (row["path"].as_str(), row["to"].as_str()) else {
                        continue;
                    };
                    let from = std::path::PathBuf::from(path);
                    match cian_core::ops::rename_in_place(&from, to) {
                        Ok(_) => {
                            self.did(Undo::Rename { from: from.clone(), to: from.with_file_name(to) });
                            done += 1;
                        }
                        Err(e) => errors.push(format!("{}: {e}", from.display())),
                    }
                }
                for which in ["left", "right"] {
                    let _ = self.pane_mut(which).map(|p| p.reload());
                }
                Ok(serde_json::json!({ "renamed": done, "errors": errors }))
            }
            // ---- Archives ----
            "archivelist" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (path, name, _) = self.selected(&which)?;
                if !cian_core::archive::is_archive(&path) {
                    anyhow::bail!("{name} はアーカイブではありません");
                }
                let members = cian_core::archive::list(&path)?;
                Ok(serde_json::json!({
                    "name": name,
                    "path": path.display().to_string(),
                    "members": members.iter().take(5000).map(|m| serde_json::json!({
                        "name": m.name, "is_dir": m.is_dir,
                        "size": m.size, "compressed": m.compressed,
                    })).collect::<Vec<_>>(),
                }))
            }
            "compress" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let kind = req.params["kind"].as_str().unwrap_or("zip").to_string();
                let paths = self.targets(&which)?;
                if paths.is_empty() {
                    anyhow::bail!("対象がありません");
                }
                let cwd = self.pane_mut(&which)?.cwd.clone();
                let stem = req.params["name"].as_str().map(str::to_string).unwrap_or_else(|| {
                    paths[0].file_stem().map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "archive".into())
                });
                let ext = match kind.as_str() {
                    "tar" => "tar",
                    "targz" => "tar.gz",
                    _ => "zip",
                };
                let dest = cwd.join(format!("{stem}.{ext}"));
                if dest.exists() {
                    anyhow::bail!("{} はすでにあります", dest.display());
                }
                let report = quietly(|ctl| match kind.as_str() {
                    "tar" => cian_core::archive::create_tar(&paths, &dest, false, ctl),
                    "targz" => cian_core::archive::create_tar(&paths, &dest, true, ctl),
                    _ => cian_core::archive::create_zip(
                        &paths, &dest, req.params["password"].as_str(), ctl),
                });
                self.did(Undo::Created { path: dest.clone() });
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                Ok(serde_json::json!({
                    "made": dest.file_name().map(|s| s.to_string_lossy().into_owned()),
                    "ok": report.ok, "errors": report.errors,
                    "pane": self.view(&which)?,
                }))
            }
            "extract" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (path, name, _) = self.selected(&which)?;
                if !cian_core::archive::is_archive(&path) {
                    anyhow::bail!("{name} はアーカイブではありません");
                }
                let cwd = self.pane_mut(&which)?.cwd.clone();
                // One member where the caller named one, the whole archive
                // otherwise. `extract` has taken a member list since it was
                // written; only the whole-archive call was ever made, so the
                // list screen could show a file it could not get out.
                let members: Vec<String> = match req.params["member"].as_str() {
                    Some(m) if !m.is_empty() => vec![m.to_string()],
                    _ => Vec::new(),
                };
                let report = quietly(|ctl| cian_core::archive::extract(
                    &path, &members, &cwd, req.params["password"].as_str(), "", ctl));
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                Ok(serde_json::json!({
                    "from": name, "ok": report.ok, "errors": report.errors,
                    "pane": self.view(&which)?,
                }))
            }
            // ---- Version control ----
            //
            // git and svn behind one set of methods, because a person standing
            // in a working copy wants "the history of this file" and not "the
            // git history of this file". Which one it is, is a property of the
            // directory rather than a question worth asking.
            "vcs" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (dir, paths) = {
                    let pane = self.pane_mut(&which)?;
                    (pane.cwd.clone(), pane.entries.iter().map(|e| e.path.clone()).collect::<Vec<_>>())
                };
                let git = cian_core::git::status(&dir);
                let svn = cian_core::svn::is_working_copy(&dir);
                // The per-row badge, worked out here because `mark_for` knows
                // how a path is normalised and the window must not learn a
                // second answer to that. Keyed by the same `path` string the
                // rows carry, so the window is looking up rather than matching.
                //
                // Directories get `~` when something under them has changed,
                // which is the whole reason the column is worth its width: it
                // says where to go next without walking in.
                let mut marks = serde_json::Map::new();
                if let Some(g) = git.as_ref() {
                    for p in &paths {
                        if let Some(m) = g.mark_for(p) {
                            marks.insert(
                                p.display().to_string(),
                                serde_json::json!({
                                    "badge": m.badge(),
                                    "kind": format!("{m:?}").to_lowercase(),
                                }),
                            );
                        }
                    }
                }
                Ok(serde_json::json!({
                    "marks": marks,
                    "kind": if git.is_some() { Some("git") } else if svn { Some("svn") } else { None },
                    "branch": git.as_ref().map(|g| g.branch.clone()),
                    "root": git.as_ref().map(|g| g.root.display().to_string()),
                    // What the terminal build puts on its status line: the
                    // branch bar every developer glances at. It answered only
                    // "which branch", so the window could not draw the rest.
                    "ahead": git.as_ref().map(|g| g.ahead),
                    "behind": git.as_ref().map(|g| g.behind),
                    "changed": git.as_ref().map(|g| g.changed_count()),
                }))
            }
            "log" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let only_this = req.params["file"].as_bool().unwrap_or(false);
                let dir = self.pane_mut(&which)?.cwd.clone();
                let file = if only_this { self.selected(&which).ok().map(|(p, _, _)| p) } else { None };
                let (kind, commits) = if cian_core::git::status(&dir).is_some() {
                    ("git", cian_core::git::log(&dir, file.as_deref(), 200))
                } else if cian_core::svn::is_working_copy(&dir) {
                    ("svn", cian_core::svn::log(&dir, file.as_deref(), 200))
                } else {
                    anyhow::bail!("git でも svn でもありません");
                };
                Ok(serde_json::json!({
                    "kind": kind,
                    "of": file.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()),
                    "commits": commits.iter().map(|c| serde_json::json!({
                        "hash": c.hash, "date": c.date, "author": c.author, "subject": c.subject,
                    })).collect::<Vec<_>>(),
                }))
            }
            // The diff of one file against what is committed, or of one commit.
            "vcsdiff" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let dir = self.pane_mut(&which)?.cwd.clone();
                let is_git = cian_core::git::status(&dir).is_some();
                let text = match req.params["hash"].as_str() {
                    Some(h) => if is_git {
                        cian_core::git::show(&dir, h)
                    } else {
                        cian_core::svn::show(&dir, h)
                    },
                    None => {
                        let (path, _, _) = self.selected(&which)?;
                        if is_git {
                            cian_core::git::file_diff(&dir, &path)
                        } else {
                            cian_core::svn::file_diff(&dir, &path)
                        }
                    }
                };
                let Some(text) = text else {
                    anyhow::bail!("差分がありません");
                };
                Ok(serde_json::json!({
                    "lines": text.lines().take(20_000).map(str::to_string).collect::<Vec<_>>(),
                }))
            }
            "stage" | "unstage" | "discard" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let dir = self.pane_mut(&which)?.cwd.clone();
                if cian_core::git::status(&dir).is_none() {
                    anyhow::bail!("git リポジトリではありません");
                }
                let paths = self.targets(&which)?;
                match req.method.as_str() {
                    "stage" => cian_core::git::stage(&dir, &paths)?,
                    "unstage" => cian_core::git::unstage(&dir, &paths)?,
                    _ => cian_core::git::discard(&dir, &paths)?,
                }
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                Ok(serde_json::json!({
                    "did": req.method,
                    "count": paths.len(),
                    "pane": self.view(&which)?,
                }))
            }
            // Files with the same contents. Compared by content, not by name —
            // which is the whole reason to ask, since two copies of a photo
            // rarely share a name.
            "dedup" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let paths = self.targets(&which)?;
                let stop = std::sync::atomic::AtomicBool::new(false);
                let groups = cian_core::dedup::find_duplicates(&paths, &stop);
                Ok(serde_json::json!({
                    "groups": groups.iter().map(|g| g.iter()
                        .map(|p| p.display().to_string()).collect::<Vec<_>>())
                        .collect::<Vec<_>>(),
                }))
            }
            // ---- The shell ----
            "shellopen" | "shelltab" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let rows = req.params["rows"].as_u64().unwrap_or(24) as u16;
                let cols = req.params["cols"].as_u64().unwrap_or(80) as u16;
                if req.method == "shellopen" && !self.tabs.is_empty() {
                    for sh in &mut self.shells {
                        sh.resize(rows, cols);
                    }
                    return Ok(self.shell_reply());
                }
                let cwd = self.pane_mut(&which)?.cwd.clone();
                let id = self.new_shell(&cwd, rows, cols)?;
                self.tabs.push(ShellTab {
                    root: shell::Node::Leaf(id), focus: id,
                    sync: false, zoom: false, name: String::new(),
                    sync_members: Default::default(),
                });
                self.shell_at = self.tabs.len() - 1;
                Ok(self.shell_reply())
            }
            // Split the focused pane. Shift+F8 side by side, Shift+F9 stacked
            // — the terminal build's two keys.
            "shellsplit" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let down = req.params["down"].as_bool().unwrap_or(false);
                let rows = req.params["rows"].as_u64().unwrap_or(24) as u16;
                let cols = req.params["cols"].as_u64().unwrap_or(80) as u16;
                let Some(tab) = self.tabs.get(self.shell_at) else {
                    anyhow::bail!("シェルが開いていません");
                };
                let at = tab.focus;
                let cwd = self.pane_mut(&which)?.cwd.clone();
                let fresh = self.new_shell(&cwd, rows, cols)?;
                let tab = &mut self.tabs[self.shell_at];
                if !tab.root.split_at(at, fresh, down) {
                    anyhow::bail!("分割できませんでした");
                }
                tab.focus = fresh;
                Ok(self.shell_reply())
            }
            // Drag the border the focused pane sits against.
            // Name this tab for what it is doing. An empty name puts the
            // number back.
            "shellrename" => {
                let name = arg(req, "name");
                // A tab strip is a row of short labels; a long one would push
                // every other tab off the strip it is supposed to help read.
                if name.chars().count() > 24 {
                    anyhow::bail!("名前は 24 文字までです");
                }
                let at = req.params["tab"].as_u64().map(|v| v as usize).unwrap_or(self.shell_at);
                let Some(tab) = self.tabs.get_mut(at) else {
                    anyhow::bail!("そのタブはありません");
                };
                tab.name = name;
                Ok(self.shell_reply())
            }
            // A divider dragged to a place, rather than nudged in a
            // direction. `id` is the first leaf of the split's near half, and
            // `ratio` is where the boundary landed as a fraction of the split.
            "shellsetratio" => {
                let id = req.params["id"].as_u64().unwrap_or(0);
                let down = req.params["down"].as_bool().unwrap_or(false);
                let to = req.params["ratio"].as_f64().unwrap_or(0.5) as f32;
                let Some(tab) = self.tabs.get_mut(self.shell_at) else {
                    anyhow::bail!("シェルが開いていません");
                };
                let moved = tab.root.set_ratio(id, down, to);
                let mut reply = self.shell_reply();
                reply["moved"] = serde_json::Value::Bool(moved);
                Ok(reply)
            }
            "shellresizepane" => {
                let wider = req.params["wider"].as_bool().unwrap_or(true);
                let down = req.params["down"].as_bool().unwrap_or(false);
                let Some(tab) = self.tabs.get_mut(self.shell_at) else {
                    anyhow::bail!("シェルが開いていません");
                };
                let id = tab.focus;
                // Said rather than raised. "No inner split along this axis" is
                // not a failure — it is the case where the key means the
                // outer divider instead, which is what the terminal build
                // does with it (keys.rs `resize_split`). Bailing here made
                // the front end show an error for a key that had more to try.
                let moved = tab.root.resize(id, wider, down);
                let mut reply = self.shell_reply();
                reply["moved"] = serde_json::Value::Bool(moved);
                Ok(reply)
            }
            // Move the keyboard to the next pane of this tab.
            "shellfocus" => {
                let step = req.params["step"].as_i64().unwrap_or(1);
                let at = req.params["at"].as_u64();
                let Some(tab) = self.tabs.get_mut(self.shell_at) else {
                    anyhow::bail!("シェルが開いていません");
                };
                let mut ids = Vec::new();
                tab.root.leaves(&mut ids);
                match at.and_then(|n| ids.get(n as usize).copied()) {
                    Some(id) => tab.focus = id,
                    None if at.is_some() => anyhow::bail!("そのペインはありません"),
                    None => {
                        if let Some(now) = ids.iter().position(|id| *id == tab.focus) {
                            let n = ids.len() as i64;
                            tab.focus = ids[((now as i64 + step).rem_euclid(n)) as usize];
                        }
                    }
                }
                Ok(self.shell_reply())
            }
            // Close the focused pane; the last one closes the tab.
            "shellpaneclose" => {
                // Which pane: the focused one for Shift+F10, or a named one
                // when a shell has ended by itself. The same close either way
                // — a pane that is over is a pane that is over.
                let named = req.params["id"].as_u64();
                let owners: Vec<Vec<u64>> = self.tabs.iter().map(|t| {
                    let mut ids = Vec::new();
                    t.root.leaves(&mut ids);
                    ids
                }).collect();
                // The rule, and why a miss is not "the focused tab", is in
                // `shell::tab_for_close` with its tests.
                let Some(at) = shell::tab_for_close(&owners, named, self.shell_at) else {
                    if named.is_some() {
                        // A pane that has already gone announcing that it has
                        // gone. Nothing to do, and nothing to report.
                        return Ok(self.shell_reply());
                    }
                    anyhow::bail!("シェルが開いていません");
                };
                if named.is_some() {
                    self.shell_at = at;
                }
                let Some(tab) = self.tabs.get_mut(at) else {
                    anyhow::bail!("シェルが開いていません");
                };
                let going = named.unwrap_or(tab.focus);
                if tab.root.close(going) {
                    let mut ids = Vec::new();
                    tab.root.leaves(&mut ids);
                    tab.focus = ids[0];
                    self.prune_shells();
                    return Ok(self.shell_reply());
                }
                // It was the only pane: closing it closes the tab.
                self.tabs.remove(at);
                self.prune_shells();
                if self.tabs.is_empty() {
                    return Ok(serde_json::json!({ "gone": true }));
                }
                self.shell_at = self.shell_at.min(self.tabs.len() - 1);
                Ok(self.shell_reply())
            }
            "shellinput" => {
                let text = req.params["text"].as_str().unwrap_or("").to_string();
                let Some(tab) = self.tabs.get(self.shell_at) else {
                    anyhow::bail!("シェルが開いていません");
                };
                // With sync on, every pane of this tab hears it.
                let targets: Vec<u64> = if tab.sync {
                    let mut ids = Vec::new();
                    tab.root.leaves(&mut ids);
                    // The chosen members if any are set, otherwise every pane.
                    if !tab.sync_members.is_empty() {
                        ids.retain(|i| tab.sync_members.contains(i));
                    }
                    ids
                } else {
                    vec![tab.focus]
                };
                for id in targets {
                    if let Some(sh) = self.shells.iter().find(|s| s.id == id) {
                        sh.write(text.as_bytes());
                    }
                }
                Ok(serde_json::json!({}))
            }
            // Type into every pane of this tab at once, or stop.
            "shellpanezoom" => {
                let Some(tab) = self.tabs.get_mut(self.shell_at) else {
                    anyhow::bail!("シェルが開いていません");
                };
                tab.zoom = !tab.zoom;
                let on = tab.zoom;
                let mut reply = self.shell_reply();
                reply["zoom"] = serde_json::json!(on);
                Ok(reply)
            }
            // Write everything this pane shows (and will show) to a file.
            "shelllog" => {
                let name = arg(req, "name");
                let dir = self.pane_cwd(req.params["pane"].as_str().unwrap_or("left"));
                let Some(sh) = self.shell_now() else {
                    anyhow::bail!("シェルが開いていません");
                };
                if sh.is_logging() {
                    let was = sh.log_path();
                    sh.stop_log();
                    return Ok(serde_json::json!({
                        "stopped": was.map(|p| p.display().to_string()),
                    }));
                }
                if name.is_empty() {
                    anyhow::bail!("ログの名前がありません");
                }
                let at = dir.join(name);
                sh.start_log(&at)?;
                Ok(serde_json::json!({ "logging": at.display().to_string() }))
            }
            "shellsync" => {
                let Some(tab) = self.tabs.get_mut(self.shell_at) else {
                    anyhow::bail!("シェルが開いていません");
                };
                tab.sync = req.params["on"].as_bool().unwrap_or(!tab.sync);
                let on = tab.sync;
                // A fresh sync is all panes; the subset is chosen after.
                if !on {
                    tab.sync_members.clear();
                }
                let mut reply = self.shell_reply();
                reply["sync"] = serde_json::json!(on);
                Ok(reply)
            }
            // Put this pane in the sync group, or take it out. With nobody
            // chosen the group is every pane, so the first pane picked is the
            // one that *narrows* it — cian-tui's `toggle_sync_member`.
            "shellsyncmember" => {
                let Some(tab) = self.tabs.get_mut(self.shell_at) else {
                    anyhow::bail!("シェルが開いていません");
                };
                let id = tab.focus;
                if !tab.sync_members.insert(id) {
                    tab.sync_members.remove(&id);
                }
                let n = tab.sync_members.len();
                let mut reply = self.shell_reply();
                reply["members"] = serde_json::json!(n);
                Ok(reply)
            }
            "shellresize" => {
                // Each pane gets the size of *its* box, not the panel's — a
                // pane that thinks it is full width wraps its output at the
                // wrong column, which is the classic broken-split look.
                let (rows, cols) = (
                    req.params["rows"].as_u64().unwrap_or(24) as f32,
                    req.params["cols"].as_u64().unwrap_or(80) as f32,
                );
                for tab in &self.tabs {
                    let mut places = Vec::new();
                    tab.root.places(0.0, 0.0, 1.0, 1.0, &mut places);
                    for (id, _, _, w, h) in places {
                        if let Some(sh) = self.shells.iter_mut().find(|s| s.id == id) {
                            sh.resize(
                                ((rows * h) as u16).max(2),
                                ((cols * w) as u16).max(20),
                            );
                        }
                    }
                }
                Ok(serde_json::json!({}))
            }
            "shellscroll" => {
                let lines = req.params["lines"].as_i64();
                let Some(sh) = self.shell_now() else {
                    anyhow::bail!("シェルが開いていません");
                };
                match lines {
                    Some(n) => sh.scroll(n as isize),
                    None => sh.to_bottom(),
                }
                Ok(self.shell_reply())
            }
            "shellgo" => {
                if self.tabs.is_empty() {
                    anyhow::bail!("シェルが開いていません");
                }
                let n = self.tabs.len() as i64;
                self.shell_at = match req.params["at"].as_i64() {
                    Some(at) => at.rem_euclid(n) as usize,
                    None => (self.shell_at as i64 + req.params["step"].as_i64().unwrap_or(1))
                        .rem_euclid(n) as usize,
                };
                Ok(self.shell_reply())
            }
            "shellclose" => {
                if self.tabs.is_empty() {
                    return Ok(serde_json::json!({ "gone": true }));
                }
                self.tabs.remove(self.shell_at);
                self.prune_shells();
                if self.tabs.is_empty() {
                    return Ok(serde_json::json!({ "gone": true }));
                }
                self.shell_at = self.shell_at.min(self.tabs.len() - 1);
                Ok(self.shell_reply())
            }
            // Run a command in the shell, in this pane's directory.
            //
            // `%` is the selection, `%f` the file, `%d` the directory — the
            // terminal build's substitutions, so a command that works there
            // works here.
            "run" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let line = req.params["line"].as_str().unwrap_or("").to_string();
                if line.trim().is_empty() {
                    anyhow::bail!("コマンドがありません");
                }
                let cwd = self.pane_mut(&which)?.cwd.clone();
                let paths = self.targets(&which).unwrap_or_default();
                let quoted: Vec<String> = paths.iter().map(|p| quote(&p.display().to_string())).collect();
                let file = paths.first().map(|p| quote(&p.display().to_string())).unwrap_or_default();
                let text = line
                    .replace("%d", &quote(&cwd.display().to_string()))
                    .replace("%f", &file)
                    .replace('%', &quoted.join(" "));
                let Some(sh) = self.shell_now() else {
                    anyhow::bail!("シェルが開いていません");
                };
                sh.write(format!("{text}\n").as_bytes());
                Ok(serde_json::json!({ "sent": text }))
            }
            // One command per marked file. `{}` is the path — the terminal
            // build's `:each`.
            "each" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let line = req.params["line"].as_str().unwrap_or("").to_string();
                if !line.contains("{}") {
                    anyhow::bail!("{{}} がありません（例: :each grep -l foo {{}}）");
                }
                let paths = self.targets(&which)?;
                let Some(sh) = self.shell_now() else {
                    anyhow::bail!("シェルが開いていません");
                };
                for p in &paths {
                    let one = line.replace("{}", &quote(&p.display().to_string()));
                    sh.write(format!("{one}\n").as_bytes());
                }
                Ok(serde_json::json!({ "ran": paths.len() }))
            }
            // Walk into an archive as though it were a directory.
            //
            // The rows come from cian-core, which is where they came from for
            // the terminal build too — two front ends disagreeing about what
            // is inside one zip would be a strange thing to ship.
            "enterarchive" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let sub = req.params["sub"].as_str().unwrap_or("").to_string();
                let archive = match req.params["archive"].as_str() {
                    Some(p) => std::path::PathBuf::from(p),
                    None => self.selected(&which)?.0,
                };
                if !cian_core::archive::is_archive(&archive) {
                    anyhow::bail!("アーカイブではありません");
                }
                let members = cian_core::archive::list(&archive)?;
                let rows = cian_core::archive::archive_rows(&archive, &members, &sub);
                let pane = self.pane_mut(&which)?;
                pane.enter_archive(archive.clone(), sub.clone(), rows);
                Ok(serde_json::json!({
                    "archive": archive.display().to_string(),
                    "sub": sub,
                    "pane": self.view(&which)?,
                }))
            }
            // The file's bytes, for something the window can draw but not read
            // — an image, mostly. Capped, and refused outright above the cap
            // rather than truncated: half a PNG is not a smaller PNG.
            "bytes" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (path, name, is_dir) = self.selected(&which)?;
                if is_dir {
                    anyhow::bail!("{name} はディレクトリです");
                }
                const CAP: u64 = 24 * 1024 * 1024;
                let len = std::fs::metadata(&path)?.len();
                if len > CAP {
                    anyhow::bail!("{name} は大きすぎます（{} MB）", len / 1024 / 1024);
                }
                let bytes = std::fs::read(&path)?;
                Ok(serde_json::json!({
                    "name": name,
                    "kind": mime_of(&path),
                    "len": len,
                    "b64": b64(&bytes),
                }))
            }
            // Strip UTF-8 byte-order marks. UTF-16 ones are left alone:
            // without one, a UTF-16 file's byte order is guesswork.
            "nobom" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let paths: Vec<_> = self.targets(&which)?.into_iter().filter(|p| p.is_file()).collect();
                if paths.is_empty() {
                    anyhow::bail!("対象がありません");
                }
                let (mut stripped, mut none, mut utf16, mut failed) = (0, 0, 0, 0);
                for p in &paths {
                    match cian_core::ops::strip_utf8_bom(p) {
                        Ok(Some(true)) => stripped += 1,
                        Ok(Some(false)) => none += 1,
                        Ok(None) => utf16 += 1,
                        Err(_) => failed += 1,
                    }
                }
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                Ok(serde_json::json!({
                    "stripped": stripped, "none": none, "utf16": utf16, "failed": failed,
                    "pane": self.view(&which)?,
                }))
            }
            // The headings and definitions in the open file, for jumping.
            "outline" => {
                let Some((path, file, _)) = self.open.as_ref() else {
                    anyhow::bail!("開いているファイルがありません");
                };
                let items = cian_core::outline::outline(path, &file.lines);
                Ok(serde_json::json!({
                    "items": items.iter().map(|i| serde_json::json!({
                        "line": i.line, "level": i.level, "text": i.text,
                        "kind": format!("{:?}", i.kind),
                    })).collect::<Vec<_>>(),
                }))
            }
            // Files dropped onto a pane from the desktop. A move, like a drag
            // between two folders anywhere else.
            "drop" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let paths: Vec<std::path::PathBuf> = paths_of(req);
                if paths.is_empty() {
                    anyhow::bail!("落とされたものがありません");
                }
                let dest = self.pane_mut(&which)?.cwd.clone();
                let count = paths.len();
                let (op, queued) = self.jobs.start(
                    jobs::Plan::of(Kind::Move), paths, Some(dest), self.out.clone(),
                    self.undo.clone(), self.redo.clone(),
                );
                Ok(serde_json::json!({ "op": op, "count": count, "queued": queued }))
            }
            // ---- Line operations on the open file ----
            //
            // Done here rather than in the window because cian-core already
            // does them, correctly, for the terminal build. `:han` and `:zen`
            // in particular are a table of Japanese width mappings that nobody
            // should own two copies of.
            "textop" => {
                let Some((_, file, _)) = self.open.as_ref() else {
                    anyhow::bail!("開いているファイルがありません");
                };
                let lines: Vec<String> = lines_of(req).unwrap_or_else(|| file.lines.clone());
                let width = req.params["width"].as_u64().unwrap_or(4) as usize;
                use cian_core::textops as t;
                let out = match req.params["op"].as_str().unwrap_or("") {
                    "sort" => t::sort(&lines, false),
                    "rsort" => t::sort(&lines, true),
                    "uniq" => t::uniq(&lines),
                    "han" => lines.iter().map(|l| t::to_halfwidth(l)).collect(),
                    "zen" => lines.iter().map(|l| t::to_fullwidth(l)).collect(),
                    "expand" => t::expand_tabs(&lines, width),
                    "expandall" => t::expand_all_tabs(&lines, width),
                    "unexpand" => t::unexpand_tabs(&lines, width),
                    "reindent" => t::reindent(&lines, width),
                    other => anyhow::bail!("知らない操作: {other}"),
                };
                Ok(serde_json::json!({ "lines": out }))
            }
            // Change the line endings the open file will be written with.
            "eol" => {
                let Some((_, file, _)) = self.open.as_mut() else {
                    anyhow::bail!("開いているファイルがありません");
                };
                file.eol = match req.params["kind"].as_str() {
                    Some("crlf") => cian_core::viewer::Eol::Crlf,
                    _ => cian_core::viewer::Eol::Lf,
                };
                Ok(serde_json::json!({ "eol": format!("{:?}", file.eol) }))
            }
            // ---- Replace across every file a grep matched ----
            //
            // The plan first, and every line of it: this writes to files that
            // are not open and cannot be undone with `u`. Seeing each line
            // before and after is the only thing that makes it safe.
            "replaceplan" => {
                let paths: Vec<std::path::PathBuf> = paths_of(req);
                let spec = req.params["spec"].as_str().unwrap_or("");
                let sub = cian_core::substitute::parse(spec).map_err(|e| anyhow::anyhow!(e))?;
                let (changes, skipped) = cian_core::grepedit::plan(&paths, &sub);
                Ok(serde_json::json!({
                    "changes": changes.iter().map(|c| serde_json::json!({
                        "path": c.path.display().to_string(),
                        "line": c.line, "before": c.before, "after": c.after,
                    })).collect::<Vec<_>>(),
                    "skipped": skipped.iter().map(|s| serde_json::json!({
                        "path": s.path.display().to_string(), "why": s.why,
                    })).collect::<Vec<_>>(),
                }))
            }
            "replaceapply" => {
                let changes: Vec<cian_core::grepedit::Change> = req.params["changes"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| Some(cian_core::grepedit::Change {
                        path: std::path::PathBuf::from(v["path"].as_str()?),
                        line: v["line"].as_u64()? as usize,
                        before: v["before"].as_str()?.to_string(),
                        after: v["after"].as_str()?.to_string(),
                        picked: true,
                    })).collect())
                    .unwrap_or_default();
                if changes.is_empty() {
                    anyhow::bail!("置換する行がありません");
                }
                let r = cian_core::grepedit::apply(&changes);
                for which in ["left", "right"] {
                    let _ = self.pane_mut(which).map(|p| p.reload());
                }
                Ok(serde_json::json!({
                    "files": r.files, "lines": r.lines, "stale": r.stale, "errors": r.errors,
                }))
            }
            // ---- svn, the three that are not shared with git ----
            "svn" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let dir = self.pane_mut(&which)?.cwd.clone();
                if !cian_core::svn::is_working_copy(&dir) {
                    anyhow::bail!("svn の作業コピーではありません");
                }
                let paths = self.targets(&which).unwrap_or_default();
                let what = req.params["what"].as_str().unwrap_or("");
                // These three answer with `()`; what to say is this end's job.
                let said = match what {
                    "update" => {
                        cian_core::svn::update(&dir)?;
                        "svn update しました".to_string()
                    }
                    "commit" => {
                        let msg = req.params["message"].as_str().unwrap_or("");
                        if msg.trim().is_empty() {
                            anyhow::bail!("コミットメッセージがありません");
                        }
                        cian_core::svn::commit(&dir, &paths, msg)?;
                        format!("{} 件を svn commit しました", paths.len())
                    }
                    "resolve" => {
                        cian_core::svn::resolve(&dir, &paths)?;
                        format!("{} 件を解決済みにしました", paths.len())
                    }
                    other => anyhow::bail!("知らない svn 操作: {other}"),
                };
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                Ok(serde_json::json!({
                    "said": said,
                    "pane": self.view(&which)?,
                }))
            }
            // ---- A server, in this pane ----
            //
            // Not a separate window or a transfer dialog: the rows are rows,
            // and `c`/`m` across to the other pane are an upload or a
            // download. That is the terminal build's arrangement and the
            // reason it is worth having at all.
            // The hosts init.lua declares, for the Shift+S picker. Names
            // only — whether a password is stored is a yes or a no, never the
            // password itself. Secrets do not travel to the window.
            // `:ssh` / Shift+S / メニューの「SSH接続」 — a **shell session on
            // the host**, which is what the terminal build has always meant by
            // it (`App::ssh_connect`, ssh.rs). The window used to answer the
            // same word by opening an SFTP listing in a pane, so the two
            // builds disagreed about what `:ssh` *was*: one gave you a prompt
            // on the far machine, the other gave you its files. Reported after
            // the first Windows session — "本来はシェルパネルでSSHするはずだ",
            // and quite right.
            //
            // The files are still one key away, and now under their own name:
            // `:sftp` opens a host from `init.lua` in the pane.
            "sshshell" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let rows = req.params["rows"].as_u64().unwrap_or(24) as u16;
                let cols = req.params["cols"].as_u64().unwrap_or(80) as u16;
                let cfg = cian_lua::load();
                let h = cfg
                    .ssh_hosts
                    .get(req.params["host"].as_u64().unwrap_or(0) as usize)
                    .ok_or_else(|| anyhow::anyhow!("そのホストはありません"))?;
                let u = h
                    .users
                    .get(req.params["user"].as_u64().unwrap_or(0) as usize)
                    .ok_or_else(|| anyhow::anyhow!("そのユーザはありません"))?;
                let line = cian_core::auth::ssh_command(&u.name, &h.host, h.port);
                let label = format!("{}@{}", u.name, h.name);
                // Resolved before the command is sent, so a slow `password_cmd`
                // cannot make us miss the prompt — the terminal build learned
                // this the same way.
                let secret = match (&u.password, &u.password_cmd) {
                    (Some(p), _) => Some(p.clone()),
                    (None, Some(cmd)) => {
                        let out = cian_core::proc::quiet(if cfg!(windows) { "cmd" } else { "sh" })
                            .args(if cfg!(windows) { ["/C", cmd.as_str()] } else { ["-c", cmd.as_str()] })
                            .output()?;
                        Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
                    }
                    (None, None) => None,
                };
                // A panel that is not open yet is opened, rather than an error
                // saying so: "connect to this host" is not a request that
                // becomes invalid because no shell happens to be running.
                if self.tabs.is_empty() {
                    let cwd = self.pane_mut(&which)?.cwd.clone();
                    let id = self.new_shell(&cwd, rows, cols)?;
                    self.tabs.push(ShellTab {
                        root: shell::Node::Leaf(id), focus: id,
                        sync: false, zoom: false, name: String::new(),
                        sync_members: Default::default(),
                    });
                    self.shell_at = self.tabs.len() - 1;
                }
                let Some(sh) = self.shell_now() else {
                    anyhow::bail!("シェルが開けませんでした");
                };
                sh.write(format!("{line}\n").as_bytes());
                if let Some(secret) = secret {
                    sh.arm_auth(secret);
                }
                let mut reply = self.shell_reply();
                reply["ran"] = serde_json::json!(line);
                reply["who"] = serde_json::json!(label);
                reply["keyed"] = serde_json::json!(u.password.is_none() && u.password_cmd.is_none());
                Ok(reply)
            }
            "sshhosts" => {
                let cfg = cian_lua::load();
                Ok(serde_json::json!({
                    "hosts": cfg.ssh_hosts.iter().enumerate().map(|(i, h)| serde_json::json!({
                        "at": i,
                        "name": h.name,
                        "host": h.host,
                        "port": h.port.unwrap_or(22),
                        "users": h.users.iter().enumerate().map(|(j, u)| serde_json::json!({
                            "at": j,
                            "name": u.name,
                            "stored": u.password.is_some() || u.password_cmd.is_some(),
                        })).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                }))
            }
            "connect" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                // Either spelled out, or named from the config. The named form
                // resolves the stored password (or runs password_cmd) here, so
                // the secret exists in the engine and nowhere else.
                let target = if let Some(hi) = req.params["preset_host"].as_u64() {
                    let cfg = cian_lua::load();
                    let h = cfg
                        .ssh_hosts
                        .get(hi as usize)
                        .ok_or_else(|| anyhow::anyhow!("そのホストはありません"))?;
                    let u = h
                        .users
                        .get(req.params["preset_user"].as_u64().unwrap_or(0) as usize)
                        .ok_or_else(|| anyhow::anyhow!("そのユーザはありません"))?;
                    let password = match (&u.password, &u.password_cmd) {
                        (Some(p), _) => p.clone(),
                        (None, Some(cmd)) => {
                            let out = cian_core::proc::quiet(
                                if cfg!(windows) { "cmd" } else { "sh" })
                                .args(if cfg!(windows) { ["/C", cmd.as_str()] } else { ["-c", cmd.as_str()] })
                                .output()?;
                            String::from_utf8_lossy(&out.stdout).trim_end().to_string()
                        }
                        (None, None) => req.params["password"].as_str().unwrap_or("").to_string(),
                    };
                    cian_scp::Target {
                        host: h.host.clone(),
                        port: h.port.unwrap_or(22),
                        user: u.name.clone(),
                        password,
                        key: u.key_path(),
                        key_pass: u.key_pass.clone(),
                    }
                } else {
                    cian_scp::Target {
                        host: req.params["host"].as_str().unwrap_or("").to_string(),
                        port: req.params["port"].as_u64().unwrap_or(22) as u16,
                        user: req.params["user"].as_str().unwrap_or("").to_string(),
                        password: req.params["password"].as_str().unwrap_or("").to_string(),
                        // Typed by hand: a key would have to be typed too, and
                        // the place to keep one is `init.lua`.
                        key: req.params["key"].as_str().map(std::path::PathBuf::from),
                        key_pass: req.params["key_pass"].as_str().map(str::to_string),
                    }
                };
                if target.host.is_empty() || target.user.is_empty() {
                    anyhow::bail!("ホストとユーザが要ります");
                }
                let start = req.params["path"].as_str().unwrap_or(".").to_string();
                let (resolved, entries) = cian_scp::list_dir(&target, &start)?;
                let label = format!("{}@{}", target.user, target.host);
                self.remotes.insert(which.clone(), target);
                let rows = remote_rows(&resolved, &entries);
                let pane = self.pane_mut(&which)?;
                pane.enter_remote(label.clone(), resolved.clone(), rows);
                Ok(serde_json::json!({
                    "host": label,
                    "path": resolved,
                    "pane": self.view(&which)?,
                }))
            }
            "remotelist" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let Some(target) = self.remotes.get(&which).cloned() else {
                    anyhow::bail!("このペインはサーバに繋がっていません");
                };
                let (label, here) = {
                    let pane = self.pane_mut(&which)?;
                    let Some((h, p)) = pane.remote_view() else {
                        anyhow::bail!("このペインはサーバを表示していません");
                    };
                    (h.to_string(), p.to_string())
                };
                // A named path, or the row under the cursor, or one level up.
                let want = match req.params["path"].as_str() {
                    Some(p) => p.to_string(),
                    None if req.params["up"].as_bool().unwrap_or(false) => {
                        cian_scp::remote_parent(&here)
                    }
                    None => {
                        let (path, _, is_dir) = self.selected(&which)?;
                        if !is_dir {
                            anyhow::bail!("ディレクトリではありません");
                        }
                        path.display().to_string()
                    }
                };
                let (resolved, entries) = cian_scp::list_dir(&target, &want)?;
                let rows = remote_rows(&resolved, &entries);
                let pane = self.pane_mut(&which)?;
                pane.enter_remote(label, resolved.clone(), rows);
                Ok(serde_json::json!({
                    "path": resolved,
                    "pane": self.view(&which)?,
                }))
            }
            // Copy across when one of the two panes is a server.
            //
            // `c` is `c` either way: the difference between a copy and an
            // upload is which pane you are standing in, and making it a
            // separate command would be asking the person to know something
            // the program already knows.
            "transfer" => {
                let from = req.params["pane"].as_str().unwrap_or("left").to_string();
                let to = if from == "left" { "right" } else { "left" };
                let paths = self.targets(&from)?;
                if paths.is_empty() {
                    anyhow::bail!("対象がありません");
                }
                let up = self.remotes.contains_key(to);
                let down = self.remotes.contains_key(&from);
                let target = self.remotes.get(if up { to } else { &from }).cloned();
                let Some(target) = target else {
                    anyhow::bail!("どちらのペインもサーバではありません");
                };
                // Server to server, by way of this machine.
                //
                // cian-tui does exactly this (`start_remote_to_remote`,
                // ssh.rs:695): download to a temporary, upload to the far
                // side, delete the temporary. There is no server-to-server
                // SFTP, and on a segmented corporate network the two hosts
                // usually cannot reach each other anyway — the machine in the
                // middle is the only route there is.
                if up && down {
                    let from_target = self.remotes.get(&from).cloned()
                        .ok_or_else(|| anyhow::anyhow!("送り元がサーバではありません"))?;
                    let to_target = self.remotes.get(to).cloned()
                        .ok_or_else(|| anyhow::anyhow!("送り先がサーバではありません"))?;
                    let dest = self.pane_mut(to)?.remote_view().map(|(_, p)| p.to_string())
                        .ok_or_else(|| anyhow::anyhow!("転送先が分かりません"))?;
                    let relay = std::env::temp_dir().join("cian-relay");
                    std::fs::create_dir_all(&relay)?;
                    let stop = std::sync::atomic::AtomicBool::new(false);
                    let out = self.out.clone();
                    let op = self.transfer_op;
                    let (mut ok, mut errors) = (0usize, Vec::new());
                    for (i, p) in paths.iter().enumerate() {
                        let name = p.file_name().map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| p.display().to_string());
                        let hop = relay.join(&name);
                        let started = std::time::Instant::now();
                        let shown = p.display().to_string();
                        let out2 = out.clone();
                        let mut report = |done: u64, total: u64| {
                            out2.event("progress", serde_json::json!({
                                "op": op, "done": i, "total": paths.len(),
                                "bytes": done, "bytes_total": total,
                                "ms": started.elapsed().as_millis() as u64,
                                "path": shown,
                            }));
                        };
                        let mut ctl = cian_scp::Ctl {
                            cancel: &stop, on_progress: &mut report, limit_bps: self.limit_bps,
                        };
                        let step = cian_scp::download(
                            &from_target, &p.display().to_string(), &hop, &mut ctl,
                        ).map(|_| ()).and_then(|()| {
                            cian_scp::upload(
                                &to_target, &hop, &cian_scp::remote_join(&dest, &name), None, &mut ctl,
                            ).map(|_| ())
                        });
                        // The temporary goes whether or not it arrived: it is
                        // somebody's file sitting in /tmp otherwise.
                        let _ = std::fs::remove_file(&hop);
                        match step {
                            Ok(()) => ok += 1,
                            Err(e) => errors.push(format!("{name}: {e}")),
                        }
                    }
                    out.event("done", serde_json::json!({
                        "op": op, "ok": ok, "skipped": 0, "ms": 0,
                        "errors": errors.clone(), "cancelled": false,
                    }));
                    self.transfer_op += 1;
                    return Ok(serde_json::json!({
                        "direction": "relay",
                        "op": op,
                        "ok": ok,
                        "errors": errors,
                        "left": PaneView::of(self.left.get()),
                        "right": PaneView::of(self.right.get()),
                    }));
                }
                let dest = if up {
                    self.pane_mut(to)?.remote_view().map(|(_, p)| p.to_string())
                        .ok_or_else(|| anyhow::anyhow!("転送先が分かりません"))?
                } else {
                    self.pane_mut(to)?.cwd.display().to_string()
                };
                let stop = std::sync::atomic::AtomicBool::new(false);
                let (mut ok, mut errors) = (0usize, Vec::new());
                // The same progress the local operations report, so a transfer
                // uses the bar that is already on screen rather than sitting
                // silent until it finishes. `on_progress` was a `noop` here
                // from the beginning: cian-scp has always counted the bytes
                // and nobody was listening to them.
                //
                // `op` is 0 — the bar is told about this one by the reply, and
                // the job queue is untouched: SFTP does not go through it, and
                // rebuilding the queue around a transfer I cannot test would
                // put the local copies at risk to move a progress bar.
                let out = self.out.clone();
                let total_files = paths.len();
                let op = self.transfer_op;
                out.event("progress", serde_json::json!({
                    "op": op, "done": 0, "total": total_files,
                    "bytes": 0, "bytes_total": 0, "ms": 0, "path": "",
                }));
                for (i, p) in paths.iter().enumerate() {
                    let name = p.file_name().map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.display().to_string());
                    let started = std::time::Instant::now();
                    let shown = p.display().to_string();
                    let out2 = out.clone();
                    let mut report = |done: u64, total: u64| {
                        out2.event("progress", serde_json::json!({
                            "op": op, "done": i, "total": total_files,
                            "bytes": done, "bytes_total": total,
                            "ms": started.elapsed().as_millis() as u64,
                            "path": shown,
                        }));
                    };
                    let mut ctl = cian_scp::Ctl {
                        cancel: &stop,
                        on_progress: &mut report,
                        limit_bps: self.limit_bps,
                    };
                    let r = if up {
                        cian_scp::upload(&target, p, &cian_scp::remote_join(&dest, &name), None, &mut ctl)
                            .map(|_| ())
                    } else {
                        // A remote row's `path` is the remote absolute path.
                        cian_scp::download(
                            &target,
                            &p.display().to_string(),
                            &std::path::Path::new(&dest).join(&name),
                            &mut ctl,
                        ).map(|_| ())
                    };
                    match r {
                        Ok(()) => {
                            // Read it back and compare checksums, when asked.
                            // cian-tui's `verify_transfer`: the local file
                            // hashed here, the remote one streamed through the
                            // same hasher, so a truncated upload is caught
                            // rather than reported as a success.
                            if self.verify_transfers {
                                let remote = if up {
                                    cian_scp::remote_join(&dest, &name)
                                } else {
                                    p.display().to_string()
                                };
                                let local = if up {
                                    p.clone()
                                } else {
                                    std::path::Path::new(&dest).join(&name)
                                };
                                match verify_transfer(&target, &remote, &local, &stop) {
                                    Ok(()) => ok += 1,
                                    Err(e) => errors.push(format!("{name}: {e}")),
                                }
                            } else {
                                ok += 1;
                            }
                        }
                        Err(e) => errors.push(format!("{name}: {e}")),
                    }
                }
                // Both sides may have changed; re-read whichever is local.
                for which in ["left", "right"] {
                    if !self.remotes.contains_key(which) {
                        let _ = self.pane_mut(which).map(|p| p.reload());
                    }
                }
                out.event("done", serde_json::json!({
                    "op": op, "ok": ok, "skipped": 0, "ms": 0,
                    "errors": errors.clone(), "cancelled": false,
                }));
                self.transfer_op += 1;
                Ok(serde_json::json!({
                    "direction": if up { "up" } else { "down" },
                    "op": op,
                    "ok": ok,
                    "errors": errors,
                    "left": PaneView::of(self.left.get()),
                    "right": PaneView::of(self.right.get()),
                }))
            }
            // Making, renaming and removing on the server.
            //
            // The same keys as here — `a`, `A`, `r`, `d` — because the rows
            // look the same and behave the same. The one difference is stated
            // rather than hidden: a remote delete is a delete, since SFTP has
            // no trash to put anything in.
            "remoteop" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let Some(target) = self.remotes.get(&which).cloned() else {
                    anyhow::bail!("このペインはサーバに繋がっていません");
                };
                let here = {
                    let pane = self.pane_mut(&which)?;
                    let Some((_, p)) = pane.remote_view() else {
                        anyhow::bail!("このペインはサーバを表示していません");
                    };
                    p.to_string()
                };
                let what = req.params["what"].as_str().unwrap_or("");
                let name = arg(req, "name");
                let said = match what {
                    "mkdir" | "touch" => {
                        if name.is_empty() {
                            anyhow::bail!("名前が空です");
                        }
                        let at = cian_scp::remote_join(&here, &name);
                        if what == "mkdir" {
                            cian_scp::make_dir(&target, &at)?;
                        } else {
                            cian_scp::make_file(&target, &at)?;
                        }
                        format!("{name} を作りました")
                    }
                    "rename" => {
                        if name.is_empty() {
                            anyhow::bail!("名前が空です");
                        }
                        let (from, was, _) = self.selected(&which)?;
                        let from = from.display().to_string();
                        cian_scp::rename(&target, &from, &cian_scp::remote_join(&here, &name))?;
                        format!("{was} → {name}")
                    }
                    "delete" => {
                        let paths = self.targets(&which)?;
                        if paths.is_empty() {
                            anyhow::bail!("対象がありません");
                        }
                        let mut gone = 0usize;
                        for p in &paths {
                            let path = p.display().to_string();
                            // The row knows whether it was a directory; SFTP
                            // needs telling, because rmdir and unlink are two
                            // calls and picking the wrong one just fails.
                            let is_dir = self
                                .pane_mut(&which)?
                                .entries
                                .iter()
                                .find(|e| e.path == *p)
                                .map(|e| e.is_dir)
                                .unwrap_or(false);
                            cian_scp::remove(&target, &path, is_dir)?;
                            gone += 1;
                        }
                        format!("{gone} 件を消しました")
                    }
                    // Move *within* the server: SFTP's rename does it, and
                    // it is the same call as a rename — the difference is
                    // only whether the new name has a directory in front of
                    // it. Crossing hosts is the transfer path above, not this.
                    "move" => {
                        let to_dir = arg(req, "to");
                        if to_dir.is_empty() {
                            anyhow::bail!("移動先が空です");
                        }
                        let paths = self.targets(&which)?;
                        if paths.is_empty() {
                            anyhow::bail!("対象がありません");
                        }
                        let mut moved = 0usize;
                        for p in &paths {
                            let from = p.display().to_string();
                            let name = from.rsplit('/').next().unwrap_or(&from).to_string();
                            cian_scp::rename(
                                &target,
                                &from,
                                &cian_scp::remote_join(&to_dir, &name),
                            )?;
                            moved += 1;
                        }
                        format!("{moved} 件を {to_dir} へ移しました")
                    }
                    other => anyhow::bail!("知らないリモート操作: {other}"),
                };
                let (_, entries) = cian_scp::list_dir(&target, &here)?;
                let label = format!("{}@{}", target.user, target.host);
                let rows = remote_rows(&here, &entries);
                let pane = self.pane_mut(&which)?;
                pane.enter_remote(label, here, rows);
                let mut reply = self.view(&which)?;
                reply["said"] = serde_json::json!(said);
                Ok(reply)
            }
            // Read a file that lives on the server: downloaded to a
            // temporary and opened from there, exactly the archive-member
            // arrangement — everything downstream works on a path, and the
            // engine remembers where the copy came from so Ctrl+S can put it
            // back. A temporary that has forgotten its origin can only be lost.
            "remoteview" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let Some(target) = self.remotes.get(&which).cloned() else {
                    anyhow::bail!("このペインはサーバに繋がっていません");
                };
                let (rpath, name, is_dir) = self.selected(&which)?;
                if is_dir {
                    anyhow::bail!("{name} はディレクトリです");
                }
                let remote_path = rpath.display().to_string();
                let dir = std::env::temp_dir().join(format!("cian-remote-{}", std::process::id()));
                std::fs::create_dir_all(&dir)?;
                let at = dir.join(&name);
                let stop = std::sync::atomic::AtomicBool::new(false);
                let mut noop = |_: u64, _: u64| {};
                let mut ctl = cian_scp::Ctl { cancel: &stop, on_progress: &mut noop, limit_bps: self.limit_bps };
                cian_scp::download(&target, &remote_path, &at, &mut ctl)?;
                self.remote_member = Some((target, remote_path, at.clone()));
                Ok(serde_json::json!({ "path": at.display().to_string(), "name": name }))
            }
            // Put the edited copy back where it came from.
            "remotesave" => {
                let Some((target, remote_path, at)) = self.remote_member.clone() else {
                    anyhow::bail!("サーバから開いたファイルがありません");
                };
                if let Some(lines) = lines_of(req) {
                    let original = cian_core::grepedit::read_text(&at)?;
                    let file = cian_core::grepedit::TextFile { lines, ..original };
                    cian_core::grepedit::write_text(&at, &file)?;
                }
                let stop = std::sync::atomic::AtomicBool::new(false);
                let mut noop = |_: u64, _: u64| {};
                let mut ctl = cian_scp::Ctl { cancel: &stop, on_progress: &mut noop, limit_bps: self.limit_bps };
                cian_scp::upload(&target, &at, &remote_path, None, &mut ctl)?;
                let name = remote_path.rsplit('/').next().unwrap_or(&remote_path).to_string();
                Ok(serde_json::json!({ "saved": name }))
            }
            // Local files up to the pane's remote directory — what a paste or
            // a desktop drop means when the pane is a server.
            "uploadpaths" | "uploadclip" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let Some(target) = self.remotes.get(&which).cloned() else {
                    anyhow::bail!("このペインはサーバに繋がっていません");
                };
                let dest = {
                    let pane = self.pane_mut(&which)?;
                    pane.remote_view()
                        .map(|(_, p)| p.to_string())
                        .ok_or_else(|| anyhow::anyhow!("転送先が分かりません"))?
                };
                let paths: Vec<std::path::PathBuf> = if req.method == "uploadclip" {
                    // The window never sees what the register holds; the
                    // register is here.
                    let held = self.clip.clone()
                        .ok_or_else(|| anyhow::anyhow!("クリップボードは空です"))?;
                    if !cian_core::clip::survives(held.op) {
                        self.clip = None;
                    }
                    held.paths
                } else {
                    paths_of(req)
                };
                if paths.is_empty() {
                    anyhow::bail!("送るものがありません");
                }
                let stop = std::sync::atomic::AtomicBool::new(false);
                let (mut ok, mut errors) = (0usize, Vec::new());
                for p in &paths {
                    let name = p.file_name().map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.display().to_string());
                    let mut noop = |_: u64, _: u64| {};
                    let mut ctl = cian_scp::Ctl { cancel: &stop, on_progress: &mut noop, limit_bps: self.limit_bps };
                    match cian_scp::upload(&target, p, &cian_scp::remote_join(&dest, &name), None, &mut ctl) {
                        Ok(_) => ok += 1,
                        Err(e) => errors.push(format!("{name}: {e}")),
                    }
                }
                // Re-list so the uploads appear where they landed.
                let (resolved, entries) = cian_scp::list_dir(&target, &dest)?;
                let label = format!("{}@{}", target.user, target.host);
                let rows = remote_rows(&resolved, &entries);
                let pane = self.pane_mut(&which)?;
                pane.enter_remote(label, resolved, rows);
                let mut reply = self.view(&which)?;
                reply["ok"] = serde_json::json!(ok);
                reply["errors"] = serde_json::json!(errors);
                Ok(reply)
            }
            "disconnect" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                self.remotes.remove(&which);
                let pane = self.pane_mut(&which)?;
                let home = pane.cwd.clone();
                *pane = Pane::new(home)?;
                self.view(&which)
            }
            // ---- What is remembered between sessions ----
            //
            // The terminal build's own state file, not a second one. A look
            // chosen in the window and not in the terminal would be two
            // programs wearing one name. `init.lua` stays untouched: it is
            // written by hand and read as code, and a program that rewrote it
            // would be reformatting somebody's comments.
            "settings" => {
                // Read once: `load()` opens and parses the files, and asking
                // it twice in one reply would be two answers to one question.
                let cfg = cian_lua::load();
                Ok(serde_json::json!({
                "look": cian_lua::state_get("gui_look"),
                // The palette's actual ground, and which way round it is.
                //
                // **`main.js` kept a table of three grounds and there are
                // eighteen palettes.** Fifteen of them opened the window on
                // hakuji's near-white and then repainted — a flash of the
                // wrong colour on every start, for anybody not using one of
                // the three that were written down. And the frame around it
                // is drawn by the OS: on Windows the title bar follows what
                // the app says its theme is, so a dark palette in a light
                // caption bar is the window disagreeing with itself.
                //
                // Answered from `cian_core::theme`, which is where the
                // palettes live, rather than from a copy kept somewhere else.
                // `is_light` reads the background's luminance rather than the
                // name — 宵闇 and 墨 are not dark because of how they are
                // spelled.
                "ground": ground_of(),
                "style": cian_lua::state_get("gui_editor"),
                // Whether `cian.ime{}` is configured at all.
                //
                // The terminal build herds the input method the moment it is
                // configured (`sync_ime` returns early only when `config.ime`
                // is None) — there is no switch to find. The window made you
                // type `:ime` first, so the same init.lua produced different
                // behaviour in the two builds, and the one place it matters
                // most is vim's normal mode.
                "ime": cfg.ime.is_some(),
                // Its own key, not `font_level`. That one is the terminal
                // emulator's point size, which cian-tui asks the emulator to
                // set because it cannot set it itself; this is a number of
                // pixels in a window this build owns. Same idea, different
                // number — and one key holding two meanings is a key that is
                // wrong for somebody.
                "font": cian_lua::state_get("gui_font"),
                "view": cian_lua::state_get("gui_view"),
                "hints": cian_lua::state_get("gui_hints"),
                // Where the two dividers were left. GUI-only keys: the
                // terminal build has `main_pct` and `panes_pct` too but does
                // not persist them, so there is nothing to share — only a
                // name to avoid colliding with.
                "main_pct": cian_lua::state_get("gui_main_pct"),
                "panes_pct": cian_lua::state_get("gui_panes_pct"),
                "theme": cian_lua::state_get("theme"),
                // What init.lua asked for. state.toml is where the app's own
                // choices live; these are the person's, and the window was
                // ignoring seventeen of the twenty settings cian-tui reads —
                // silently, which is the worst way to ignore a config.
                "cfg": {
                    "home": cfg.options.home,
                    "editor": cfg.options.editor,
                    "shell": cfg.options.shell,
                    "view": cfg.options.view,
                    "key_hints": cfg.options.key_hints,
                    "edit_style": cfg.options.edit_style,
                    "show_hidden": cfg.options.show_hidden,
                    "tab_width": cfg.options.tab_width,
                    "lang": cfg.options.lang,
                    "notify": cfg.options.notify,
                    "notify_min_secs": cfg.options.notify_min_secs,
                    "preview": cfg.options.preview,
                    "transfer_limit": cfg.options.transfer_limit,
                    // What the context menu's launcher rows need to know
                    // before it draws them. cian-tui asks the same three
                    // questions in menu.rs (`if ai`, `if !snippets
                    // .is_empty()`, `if !macros.is_empty()`) and leaves the
                    // row out when the answer is no; the window offered all
                    // three unconditionally, so on a machine with no init.lua
                    // the menu opened with three rows that led nowhere.
                    "ai": cfg.ai.is_some(),
                    "snippets": !cfg.snippets.is_empty(),
                    "ssh_hosts": !cfg.ssh_hosts.is_empty(),
                    "macros": cian_lua::config_read_path("macro.lua")
                        .as_ref()
                        .filter(|p| p.exists())
                        .and_then(|p| cian_lua::macros::load(p).ok())
                        .map(|m| !m.is_empty())
                        .unwrap_or(false),
                },
                // The keys the person bound in init.lua. The terminal build
                // reads the same list; a binding that works in one and not the
                // other would be two programs wearing one name.
                "keymaps": cfg.keymaps
                    .iter()
                    .map(|(spec, action)| serde_json::json!({ "key": spec, "action": action }))
                    .collect::<Vec<_>>(),
                // What Lua itself refused — a syntax error, a key spec it
                // could not read. The terminal build shows these; without
                // them a typo in init.lua is a setting that silently is not
                // there, and the person goes looking in the wrong place.
                "config_errors": cfg.errors,
                "where": cian_lua::config_read_path("state.toml")
                    .map(|p| p.display().to_string()),
                // What this platform will actually do, so the menu can leave
                // out a row that could only answer "not on this platform".
                // cian-tui asks the same question at the same place
                // (menu.rs `OsMenu`); the window was guessing from the user
                // agent, which knows the browser's platform, not the file
                // manager's.
                "os": {
                    "open_with": cian_core::os::open_with_supported(),
                    "properties": cian_core::os::properties_supported(),
                    "file_manager": cian_core::os::file_manager_name(),
                },
                // Where the synced libraries live on this disk. The menu tests
                // a path against these before it offers the two Office rows —
                // the same prefix test `cloud_url` does, run where the menu is
                // built rather than over the pipe, because a menu is drawn on
                // a keystroke and cannot wait for an answer.
                "sharepoint": cian_core::office::SyncMap::from_pairs(&cfg.sharepoint)
                    .iter()
                    .map(|m| m.local.display().to_string())
                    .collect::<Vec<_>>(),
            }))
            }
            "remember" => {
                let key = req.params["key"].as_str().unwrap_or("");
                let value = req.params["value"].as_str().unwrap_or("");
                // `theme` is the terminal build's own key, deliberately: a
                // palette chosen in the window is the palette the terminal
                // opens with, because they are one program.
                if !matches!(key,
                    "gui_look" | "gui_editor" | "gui_font" | "gui_view" | "gui_hints"
                    | "gui_main_pct" | "gui_panes_pct"
                    | "theme")
                {
                    anyhow::bail!("覚えられない項目です: {key}");
                }
                cian_lua::state_set(key, value);
                Ok(serde_json::json!({ "key": key, "value": value }))
            }
            // ---- The AI, where a site has configured one ----
            //
            // The prompts are the terminal build's, word for word. Two front
            // ends asking the same model differently would give two different
            // answers to the same question, which is the kind of difference
            // nobody can debug.
            "ai" => {
                let cfg = ai_config()?;
                if !cian_ai::available(&cfg) {
                    anyhow::bail!("AI を利用できません（python・パッケージ・サインインのいずれか）");
                }
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (system, user) = match req.params["what"].as_str().unwrap_or("") {
                    "cmd" => {
                        let want = req.params["text"].as_str().unwrap_or("");
                        if want.trim().is_empty() {
                            anyhow::bail!("やりたいことを書いてください");
                        }
                        // **Where is that shell, actually?** It starts as
                        // whatever cian launched and then somebody runs `ssh`,
                        // or `su`, or bash inside PowerShell — and the answer
                        // is *pasted at that prompt*, so writing PowerShell
                        // for a Linux server produces a line that cannot run
                        // in the one place it is going to end up.
                        //
                        // The terminal build has read the title, the `ssh`
                        // line in the scrollback and the prompt's shape since
                        // it had a shell panel; this build read none of them
                        // and sent `Platform: windows`. Both call
                        // `cian_core::shellwhere` now.
                        let (title, screen) = match self.shell_now() {
                            Some(sh) => (sh.title(), sh.contents()),
                            None => (None, None),
                        };
                        let started = self
                            .shell_now()
                            .map(|sh| sh.program.clone())
                            .unwrap_or_else(cian_pty::default_shell);
                        let started = std::path::Path::new(&started)
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or(started.clone());
                        let hosts = cian_lua::load().ssh_hosts;
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
                            &started,
                        );
                        // And what is in front of the person. "delete the old
                        // logs" is unanswerable without the names, and "zip
                        // these" means the marks.
                        let (cwd, remote, listing, marked) = {
                            let pane = self.pane_mut(&which)?;
                            let cwd = pane.cwd.display().to_string();
                            // A remote pane is somebody else's disk, and a
                            // command typed at the local prompt cannot touch
                            // it. The prompt has a rule for this; it needs the
                            // fact to apply it to.
                            let remote = pane
                                .remote_view()
                                .map(|(host, path)| format!("{host}:{path}"));
                            let listing: String = pane
                                .entries
                                .iter()
                                .filter(|e| !e.is_parent)
                                .take(80)
                                .map(|e| {
                                    format!("{}{}\n", e.name, if e.is_dir { "/" } else { "" })
                                })
                                .collect();
                            let marked: Vec<String> = pane
                                .entries
                                .iter()
                                .filter(|e| !e.is_parent && pane.marks.contains(&e.path))
                                .map(|e| e.name.clone())
                                .collect();
                            (cwd, remote, listing, marked)
                        };
                        let marks = if marked.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "\nMarked right now ({} of them) — \"these\", \"the selected ones\" means exactly this set:\n{}",
                                marked.len(),
                                marked.join("\n")
                            )
                        };
                        let where_ = match &remote {
                            Some(at) => format!(
                                "Pane: a REMOTE listing over SFTP at {at}. The shell is NOT on that machine."
                            ),
                            None => format!("Pane: local, {cwd}"),
                        };
                        (
                            cian_core::aiprompt::cmd(&target),
                            format!("{where_}\n\nIn the pane:\n{listing}{marks}\nTask: {want}"),
                        )
                    }
                    "log" => {
                        let (path, name, is_dir) = self.selected(&which)?;
                        if is_dir {
                            anyhow::bail!("ログファイルを選んでください");
                        }
                        // A log's meaning is at its end — read the tail.
                        let tail = read_tail(&path, 16_000);
                        if tail.trim().is_empty() {
                            anyhow::bail!("{name} は空です");
                        }
                        (
                            cian_core::aiprompt::LOG
                                .to_string(),
                            tail,
                        )
                    }
                    "text" => (
                        req.params["system"].as_str().unwrap_or("Answer concisely.").to_string(),
                        req.params["text"].as_str().unwrap_or("").to_string(),
                    ),
                    other => anyhow::bail!("知らない AI 依頼: {other}"),
                };
                // On a worker, and answered by an event.
                //
                // `chat` waits on a python subprocess talking to a server on
                // somebody else's network. Run here it would hold the whole
                // engine — every keystroke in the listing queued behind a
                // question about a log file. The first attempt did exactly
                // that and looked like a freeze.
                ai_in_background(self.out.clone(), cfg, system.to_string(), user.to_string(), |answer| Ok(serde_json::json!({ "answer": answer })));
                Ok(serde_json::json!({ "asked": true }))
            }
            // ---- Bookmarks ----
            //
            // The terminal build's own `shortcuts.lua`, read the same way and
            // written back through the same renderer. A second bookmark list
            // would be the worst of the two-programs problems: the folders you
            // saved would depend on which one you saved them from.
            "shortcuts" => {
                let path = cian_lua::config_read_path("shortcuts.lua");
                let nodes = path
                    .as_ref()
                    .filter(|p| p.exists())
                    .and_then(|p| cian_lua::shortcuts::load(p).ok())
                    .unwrap_or_default();
                fn flatten(nodes: &[cian_lua::shortcuts::Node], depth: usize, out: &mut Vec<serde_json::Value>) {
                    for n in nodes {
                        // Its position in this very walk, so the window can
                        // name a row back to the engine. Depth-first, and the
                        // editing below walks it the same way — one order, so
                        // "the third row" cannot mean two different nodes.
                        let at = out.len();
                        out.push(serde_json::json!({
                            "at": at,
                            "name": n.name,
                            "target": n.target,
                            "depth": depth,
                            "group": n.children.is_some(),
                        }));
                        if let Some(kids) = &n.children {
                            flatten(kids, depth + 1, out);
                        }
                    }
                }
                let mut rows = Vec::new();
                flatten(&nodes, 0, &mut rows);
                Ok(serde_json::json!({
                    "where": path.map(|p| p.display().to_string()),
                    "rows": rows,
                }))
            }
            // Edit the bookmarks: rename one, retarget one, delete one, or
            // add a folder. cian-tui does all of this from its shortcuts
            // popup (`a A d r p`), and the window could only ever append.
            "shortcutedit" => {
                let path = cian_lua::config_write_path("shortcuts.lua")
                    .ok_or_else(|| anyhow::anyhow!("設定の保存場所が分かりません"))?;
                let mut nodes = if path.exists() {
                    cian_lua::shortcuts::load(&path).unwrap_or_default()
                } else {
                    Vec::new()
                };
                let want = req.params["at"].as_u64().map(|v| v as usize);
                let value = arg(req, "value");
                // The same depth-first walk the listing above hands out, so a
                // row number means one node.
                fn at_mut<'a>(
                    nodes: &'a mut [cian_lua::shortcuts::Node],
                    want: usize,
                    seen: &mut usize,
                ) -> Option<&'a mut cian_lua::shortcuts::Node> {
                    for n in nodes.iter_mut() {
                        if *seen == want {
                            return Some(n);
                        }
                        *seen += 1;
                        if let Some(kids) = n.children.as_mut() {
                            if let Some(found) = at_mut(kids, want, seen) {
                                return Some(found);
                            }
                        }
                    }
                    None
                }
                fn remove_at(
                    nodes: &mut Vec<cian_lua::shortcuts::Node>,
                    want: usize,
                    seen: &mut usize,
                ) -> bool {
                    for i in 0..nodes.len() {
                        if *seen == want {
                            nodes.remove(i);
                            return true;
                        }
                        *seen += 1;
                        if nodes[i].children.is_some() {
                            let mut kids = nodes[i].children.take().unwrap_or_default();
                            let hit = remove_at(&mut kids, want, seen);
                            nodes[i].children = Some(kids);
                            if hit {
                                return true;
                            }
                        }
                    }
                    false
                }
                let said = match req.params["do"].as_str().unwrap_or("") {
                    "group" => {
                        if value.is_empty() {
                            anyhow::bail!("名前を入れてください");
                        }
                        nodes.push(cian_lua::shortcuts::Node {
                            name: value.clone(), target: None, children: Some(Vec::new()),
                        });
                        format!("{value} を作りました")
                    }
                    "rename" => {
                        if value.is_empty() {
                            anyhow::bail!("名前を入れてください");
                        }
                        let Some(at) = want else { anyhow::bail!("どれを直すか分かりません") };
                        let Some(n) = at_mut(&mut nodes, at, &mut 0) else {
                            anyhow::bail!("その行はありません")
                        };
                        n.name = value.clone();
                        format!("{value} に変えました")
                    }
                    "target" => {
                        let Some(at) = want else { anyhow::bail!("どれを直すか分かりません") };
                        let Some(n) = at_mut(&mut nodes, at, &mut 0) else {
                            anyhow::bail!("その行はありません")
                        };
                        if n.children.is_some() {
                            anyhow::bail!("まとめには行き先がありません");
                        }
                        n.target = Some(value.clone());
                        format!("行き先を {value} に変えました")
                    }
                    "delete" => {
                        let Some(at) = want else { anyhow::bail!("どれを消すか分かりません") };
                        if !remove_at(&mut nodes, at, &mut 0) {
                            anyhow::bail!("その行はありません");
                        }
                        "削除しました".to_string()
                    }
                    other => anyhow::bail!("知らない操作: {other}"),
                };
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                std::fs::write(&path, cian_lua::shortcuts::to_lua(&nodes))?;
                Ok(serde_json::json!({ "said": said }))
            }
            "bookmark" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                // A named place, or where the pane is standing. The history
                // screen bookmarks the row you are looking at, which is where
                // you notice that somewhere was worth keeping.
                let cwd = match req.params["path"].as_str() {
                    Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
                    _ => self.pane_mut(&which)?.cwd.clone(),
                };
                let name = arg(req, "name");
                let name = if name.is_empty() {
                    cwd.file_name().map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| cwd.display().to_string())
                } else {
                    name
                };
                let path = cian_lua::config_write_path("shortcuts.lua")
                    .ok_or_else(|| anyhow::anyhow!("設定の保存場所が分かりません"))?;
                let mut nodes = if path.exists() {
                    cian_lua::shortcuts::load(&path).unwrap_or_default()
                } else {
                    Vec::new()
                };
                let target = cwd.display().to_string();
                if nodes.iter().any(|n| n.target.as_deref() == Some(target.as_str())) {
                    anyhow::bail!("すでに登録されています");
                }
                nodes.push(cian_lua::shortcuts::Node::leaf(name.clone(), target.clone()));
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                std::fs::write(&path, cian_lua::shortcuts::to_lua(&nodes))?;
                Ok(serde_json::json!({ "name": name, "target": target }))
            }
            // ---- Tabs ----
            //
            // Each side keeps a list, and the active one *is* the pane
            // everywhere else. A new tab opens where you are standing, which
            // is what makes it useful: the reason to open one is almost always
            // "keep this, and go somewhere else for a moment".
            "tabnew" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let here = self.pane_mut(&which)?.cwd.clone();
                let fresh = Pane::new(here)?;
                let side = self.side_mut(&which)?;
                side.tabs.insert(side.at + 1, fresh);
                side.at += 1;
                Ok(serde_json::to_value(PaneView::of_side(side))?)
            }
            "tabclose" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let side = self.side_mut(&which)?;
                if side.tabs.len() <= 1 {
                    anyhow::bail!("最後のタブは閉じられません");
                }
                side.tabs.remove(side.at);
                side.at = side.at.min(side.tabs.len() - 1);
                Ok(serde_json::to_value(PaneView::of_side(side))?)
            }
            "tabgo" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let side = self.side_mut(&which)?;
                let n = side.tabs.len();
                side.at = match req.params["at"].as_i64() {
                    Some(at) => (at.rem_euclid(n as i64)) as usize,
                    None => {
                        let step = req.params["step"].as_i64().unwrap_or(1);
                        ((side.at as i64 + step).rem_euclid(n as i64)) as usize
                    }
                };
                Ok(serde_json::to_value(PaneView::of_side(side))?)
            }
            // What is running, and a way to stop one of them.
            "queue" => Ok(serde_json::json!({ "jobs": self.jobs.listing() })),
            // The files themselves onto the OS clipboard, so Finder or
            // Explorer pastes the files rather than their names. `p` puts the
            // path text there; conflating the two is how you end up pasting a
            // path into a folder.
            "clipfiles" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let paths = self.targets(&which)?;
                if paths.is_empty() {
                    anyhow::bail!("対象がありません");
                }
                cian_core::fileclip::put_files(&paths)?;
                Ok(serde_json::json!({ "count": paths.len() }))
            }
            // ---- Macros ----
            //
            // A layout macro in the terminal build builds a *grid* of shell
            // panes. There are no splits here yet, so each pane becomes a
            // tab. That is a real reduction and it is said out loud rather
            // than papered over: the shells, their commands and their scripted
            // steps all run, and only the arrangement is lost.
            "macros" => {
                // `macro.lua` *and* `macro/*.lua`. This read only the first,
                // so anybody who had split their macros one-per-file — which
                // is what the shipped examples do — opened an empty launcher.
                let (list, err) = cian_lua::macros::load_all();
                let path = cian_lua::config_read_path("macro.lua");
                Ok(serde_json::json!({
                    "where": path.map(|p| p.display().to_string()),
                    // Said out loud: a file that would not parse is why the
                    // list is short, and a short list with no reason sends
                    // people looking in the wrong place.
                    "error": err,
                    "macros": list.iter().map(|m| serde_json::json!({
                        "name": m.name,
                        "panes": m.panes.len(),
                        "script": m.is_script(),
                        "sync": m.sync,
                    })).collect::<Vec<_>>(),
                }))
            }
            "macrorun" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let want = req.params["name"].as_str().unwrap_or("").to_string();
                let rows = req.params["rows"].as_u64().unwrap_or(24) as u16;
                let cols = req.params["cols"].as_u64().unwrap_or(80) as u16;
                let (list, err) = cian_lua::macros::load_all();
                let Some(mac) = list.into_iter().find(|m| m.name == want) else {
                    if let Some(e) = err {
                        anyhow::bail!("{want} が見つかりません（{e}）");
                    }
                    anyhow::bail!("{want} というマクロはありません");
                };
                // A script macro is Lua that moves files about, not a shell
                // layout — so it runs here and answers with what it did,
                // rather than opening panels.
                //
                // **The window used to refuse it.** `macro.lua` is one file
                // holding both kinds, so half of somebody's macros worked and
                // half said "not yet" with no way to tell which was which
                // before pressing.
                if mac.is_script() {
                    let Some(src) = mac.script.clone() else {
                        anyhow::bail!("{want} に script がありません")
                    };
                    let dir = self.pane_mut(&which)?.cwd.clone();
                    let other = self.other_cwd(&which);
                    let marked = self.targets(&which).unwrap_or_default();
                    let cursor = self.selected(&which).ok().map(|(p, ..)| p);
                    let ctx = cian_lua::macro_script::Ctx { dir, other, marked, cursor };
                    let outcome = cian_lua::macro_script::run(&src, &want, ctx);
                    // Reloaded whatever happened: a macro that errored halfway
                    // has still moved whatever it moved before that, and file
                    // operations are not transactional.
                    if outcome.touched {
                        self.left.now().reload()?;
                        self.right.now().reload()?;
                    }
                    return Ok(serde_json::json!({
                        "script": true,
                        "messages": outcome.messages,
                        "error": outcome.error,
                        "left": PaneView::of(self.left.get()),
                        "right": PaneView::of(self.right.get()),
                    }));
                }
                let cwd = self.pane_mut(&which)?.cwd.clone();
                // The layout the macro actually asked for. `from` names an
                // earlier pane to split off (1-based), so a macro can build a
                // grid rather than a row — which is the whole reason the field
                // exists.
                let mut made: Vec<u64> = Vec::new();
                let mut root: Option<shell::Node> = None;
                let mut opened = 0usize;
                for step in &mac.panes {
                    let id = self.new_shell(&cwd, rows, cols)?;
                    match &mut root {
                        None => root = Some(shell::Node::Leaf(id)),
                        Some(tree) => {
                            let from = step
                                .from
                                .and_then(|n| made.get(n.saturating_sub(1)).copied())
                                .unwrap_or_else(|| *made.last().unwrap());
                            let down = matches!(step.dir, cian_lua::macros::Split::Down);
                            tree.split_at(from, id, down);
                        }
                    }
                    made.push(id);
                    let sh = self.shells.last().unwrap();
                    // The command and its scripted steps, on a worker: an
                    // `expect` waits for a prompt, and waiting here would hold
                    // the engine for as long as the login takes.
                    let cmd = step.cmd.clone();
                    let steps = step.steps.clone();
                    let handle = sh.handle();
                    std::thread::spawn(move || {
                        if let Some(cmd) = cmd {
                            handle.write(format!("{cmd}\n").as_bytes());
                        }
                        for s in &steps {
                            match s {
                                cian_lua::macros::Step::Send(line) => {
                                    handle.write(format!("{line}\n").as_bytes());
                                }
                                cian_lua::macros::Step::Wait(secs) => {
                                    std::thread::sleep(std::time::Duration::from_secs_f64(*secs));
                                }
                                cian_lua::macros::Step::Expect { text, timeout } => {
                                    handle.wait_for(text, *timeout);
                                }
                            }
                        }
                    });
                    opened += 1;
                }
                let Some(root) = root else {
                    anyhow::bail!("{want} にはペインがありません");
                };
                let focus = made[0];
                // A macro names its layout, and that name is exactly what the
                // tab is for — so it wears it.
                self.tabs.push(ShellTab {
                    root, focus, sync: mac.sync, zoom: mac.zoom, name: mac.name.clone(),
                    sync_members: Default::default(),
                });
                self.shell_at = self.tabs.len() - 1;
                let mut reply = self.shell_reply();
                reply["name"] = serde_json::json!(mac.name);
                reply["opened"] = serde_json::json!(opened);
                Ok(reply)
            }
            // Free space on the disk this pane is on.
            "df" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let at = self.pane_mut(&which)?.cwd.clone();
                let d = cian_core::inspect::disk_space(&at)?;
                Ok(serde_json::json!({
                    "where": at.display().to_string(),
                    "total": d.total,
                    "available": d.available,
                    "used": d.used(),
                }))
            }
            // Lines, words and bytes, for the selection.
            "wc" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let paths = self.targets(&which)?;
                let mut rows = Vec::new();
                for p in paths.iter().take(500) {
                    if p.is_dir() {
                        continue;
                    }
                    if let Ok(c) = cian_core::inspect::count(p) {
                        rows.push(serde_json::json!({
                            "name": p.file_name().map(|s| s.to_string_lossy().into_owned()),
                            "lines": c.lines, "words": c.words, "bytes": c.bytes,
                        }));
                    }
                }
                Ok(serde_json::json!({ "rows": rows }))
            }
            // Where cian reads and writes its settings — the question `:where`
            // exists to answer, because a portable copy beside the executable
            // wins and that is not where anybody looks first.
            "where" => Ok(serde_json::json!({
                "config": cian_lua::config_read_path("init.lua").map(|p| p.display().to_string()),
                "state": cian_lua::config_read_path("state.toml").map(|p| p.display().to_string()),
                "shortcuts": cian_lua::config_read_path("shortcuts.lua").map(|p| p.display().to_string()),
                "macros": cian_lua::config_read_path("macro.lua").map(|p| p.display().to_string()),
                "writes": cian_lua::config_write_path("init.lua").map(|p| p.display().to_string()),
                // Which build this is. The version number cannot say — it
                // moves once per release and builds happen all day — so
                // `:version` had no way to tell today's engine from last
                // week's, which is the first thing to establish when a fix
                // looks unapplied.
                "version": env!("CARGO_PKG_VERSION"),
                "commit": env!("CIAN_COMMIT"),
                "built_at": env!("CIAN_BUILT_AT").parse::<i64>().unwrap_or(0),
            })),
            // Mark by pattern. `*.rs` is what a person types; it becomes a
            // glob rather than a regex, because that is what the asterisk
            // means to everyone who is not writing one.
            "markglob" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let pattern = req.params["glob"].as_str().unwrap_or("*").to_string();
                let on = req.params["on"].as_bool().unwrap_or(true);
                let re = glob_to_regex(&pattern)?;
                let pane = self.pane_mut(&which)?;
                let hits: Vec<usize> = pane
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| !e.is_parent && re.is_match(&e.name.to_lowercase()))
                    .map(|(i, _)| i)
                    .collect();
                for i in &hits {
                    if on {
                        pane.set_mark_at(*i);
                    } else if pane.is_marked(*i) {
                        pane.toggle_mark_at(*i);
                    }
                }
                let n = hits.len();
                let mut reply = self.view(&which)?;
                reply["matched"] = serde_json::json!(n);
                Ok(reply)
            }
            // Copy or move to somewhere that is not the other pane.
            "copyto" | "moveto" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let dest = req.params["dest"].as_str().unwrap_or("").trim().to_string();
                if dest.is_empty() {
                    anyhow::bail!("行き先がありません");
                }
                let dest = std::path::PathBuf::from(shellexpand(&dest));
                if !dest.is_dir() {
                    anyhow::bail!("{} はディレクトリではありません", dest.display());
                }
                let paths = self.targets(&which)?;
                if paths.is_empty() {
                    anyhow::bail!("対象がありません");
                }
                let kind = if req.method == "copyto" { Kind::Copy } else { Kind::Move };
                let count = paths.len();
                let (op, queued) = self.jobs.start(
                    jobs::Plan::of(kind), paths, Some(dest.clone()), self.out.clone(),
                    self.undo.clone(), self.redo.clone(),
                );
                Ok(serde_json::json!({
                    "op": op, "count": count, "queued": queued,
                    "kind": if matches!(kind, Kind::Move) { "move" } else { "copy" },
                    "dest": dest.display().to_string(),
                }))
            }
            // Hand the file to the editor named in init.lua, or the one the
            // environment names.
            "editexternal" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (path, name, is_dir) = self.selected(&which)?;
                if is_dir {
                    anyhow::bail!("{name} はディレクトリです");
                }
                let editor = std::env::var("VISUAL")
                    .or_else(|_| std::env::var("EDITOR"))
                    .unwrap_or_else(|_| if cfg!(windows) { "notepad".into() } else { "vi".into() });
                cian_core::proc::quiet(&editor)
                    .arg(&path)
                    .spawn()
                    .map_err(|e| anyhow::anyhow!("{editor}: {e}"))?;
                Ok(serde_json::json!({ "editor": editor, "name": name }))
            }
            // ---- Office documents synced from a cloud drive ----
            //
            // `:office` opens the *cloud* copy in the web app; `:officelink`
            // makes a .url pointing at it. The distinction matters at work: a
            // local path in an email is a path only you can open.
            "office" | "officelink" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (path, name, _) = self.selected(&which)?;
                let cfg = cian_lua::load();
                let Some(url) = cian_core::office::cloud_url(&path, &cian_core::office::SyncMap::from_pairs(&cfg.sharepoint)) else {
                    anyhow::bail!("{name} のクラウド側が分かりません（init.lua の cian.sync{{…}}）");
                };
                if req.method == "officelink" {
                    let at = path.with_extension("url");
                    std::fs::write(&at, cian_core::office::url_shortcut(&url))?;
                    self.did(Undo::Created { path: at.clone() });
                    let pane = self.pane_mut(&which)?;
                    pane.reload()?;
                    let mut reply = self.view(&which)?;
                    reply["made"] = serde_json::json!(
                        at.file_name().map(|s| s.to_string_lossy().into_owned()));
                    return Ok(reply);
                }
                // The app's own URI where there is one — that opens Word
                // rather than a browser tab pretending to be Word.
                let target = cian_core::office::classify(&path)
                    .and_then(|doc| cian_core::office::app_uri(doc, &url))
                    .unwrap_or_else(|| url.clone());
                cian_core::proc::open_with_desktop(&target)?;
                Ok(serde_json::json!({ "opened": name, "url": url }))
            }
            // Re-read init.lua. Says what it could not change, rather than
            // pretending a restart is never needed.
            "reload" => {
                let cfg = cian_lua::load();
                Ok(serde_json::json!({
                    "ai": cfg.ai.is_some(),
                    "sync_maps": cfg.sharepoint.len(),
                    "ssh_hosts": cfg.ssh_hosts.len(),
                }))
            }
            // ---- Hex editing ----
            //
            // Overwrite only. Offsets never shift and the file cannot change
            // size, which is the difference between editing a binary and
            // corrupting one — an inserted byte moves every offset after it,
            // and in a binary those offsets are usually written down inside
            // the file itself.
            "hexset" => {
                let Some((_, view)) = self.hex.as_mut() else {
                    anyhow::bail!("16進で開いているファイルがありません");
                };
                let at = req.params["at"].as_u64().unwrap_or(0) as usize;
                let val = req.params["byte"].as_u64().unwrap_or(0) as u8;
                view.hex_set_byte(at, val);
                let line = at / 16;
                Ok(serde_json::json!({
                    "line": line,
                    "text": view.lines.get(line),
                }))
            }
            // Write the bytes back, keeping the original as `.bak`.
            //
            // The backup is not optional. A hex edit is the one change in here
            // nobody can read back to check, so the version that was working
            // has to survive it.
            "hexsave" => {
                let Some((path, view)) = self.hex.as_ref() else {
                    anyhow::bail!("16進で開いているファイルがありません");
                };
                let bak = path.with_extension(format!(
                    "{}.bak",
                    path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
                ));
                std::fs::copy(path, &bak)?;
                std::fs::write(path, view.raw_bytes())?;
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let bak_name = bak
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                for which in ["left", "right"] {
                    let _ = self.pane_mut(which).map(|p| p.reload());
                }
                Ok(serde_json::json!({ "saved": name, "backup": bak_name }))
            }
            // Who last changed each line of the open file.
            "blame" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let dir = self.pane_mut(&which)?.cwd.clone();
                let path = match self.open.as_ref() {
                    Some((p, ..)) => p.clone(),
                    None => self.selected(&which)?.0,
                };
                let lines = if cian_core::git::status(&dir).is_some() {
                    cian_core::git::blame(&dir, &path)
                } else {
                    cian_core::svn::blame(&dir, &path)
                };
                let Some(lines) = lines else {
                    anyhow::bail!("blame を取れませんでした");
                };
                Ok(serde_json::json!({
                    "lines": lines.iter().map(|b| serde_json::json!({
                        "hash": b.hash, "author": b.author, "date": b.date,
                    })).collect::<Vec<_>>(),
                }))
            }
            // Read the open file again in another encoding.
            //
            // The bytes are already here, so this decodes rather than reads —
            // which matters because the file on disk may be a log something is
            // still writing to, and re-reading it would show a different file
            // than the one being looked at.
            "encoding" => {
                let Some((path, view)) = self.shown.as_mut() else {
                    anyhow::bail!("開いているファイルがありません");
                };
                let want = match req.params["as"].as_str() {
                    Some("utf8") => cian_core::viewer::TextEncoding::Utf8,
                    Some("sjis") => cian_core::viewer::TextEncoding::ShiftJis,
                    Some("utf16le") => cian_core::viewer::TextEncoding::Utf16Le,
                    Some("utf16be") => cian_core::viewer::TextEncoding::Utf16Be,
                    // No name means the next one round, which is how the
                    // terminal build's `:enc` is used: press it until the
                    // mojibake stops.
                    _ => view.encoding.next(),
                };
                view.redecode(want);
                let path = path.clone();
                let lines = view.lines.clone();
                let enc = format!("{:?}", view.encoding);
                let eol = format!("{:?}", view.eol);
                // The editable copy has to agree, or a save would write back
                // through the encoding the file *was* read with.
                if let Ok(mut f) = cian_core::grepedit::read_text(&path) {
                    f.lines = lines.clone();
                    f.encoding = match want {
                        cian_core::viewer::TextEncoding::Utf8 => cian_core::viewer::TextEncoding::Utf8,
                        other => other,
                    };
                    let st = cian_core::stamp::of(&path);
                    self.open = Some((path, f, st));
                }
                Ok(serde_json::json!({ "lines": lines, "encoding": enc, "eol": eol }))
            }
            // `:s/old/new/` inside the open file.
            //
            // The same substitution language as the grep-wide replace, because
            // it is the same question asked of one file instead of many —
            // learning two spellings of `s///` would be absurd.
            "substitute" => {
                let spec = req.params["spec"].as_str().unwrap_or("");
                let lines: Vec<String> = lines_of(req).unwrap_or_default();
                let sub = cian_core::substitute::parse(spec).map_err(|e| anyhow::anyhow!(e))?;
                let hits = cian_core::substitute::find(&sub, &lines, None);
                if hits.is_empty() {
                    anyhow::bail!("見つかりません");
                }
                let out = cian_core::substitute::apply(&lines, &hits);
                Ok(serde_json::json!({ "lines": out, "changed": hits.len() }))
            }
            // Both sides of a comparison, as text, for an editor that shows
            // them next to each other and lets you change either.
            //
            // Separate from `compare`, which answers with rows for the report
            // screen. This is the same two files asked for differently: there,
            // "what differs"; here, "let me fix it".
            "twofiles" => {
                let (lp, ln, ld) = self.selected("left")?;
                let (rp, rn, rd) = self.selected("right")?;
                if ld || rd {
                    anyhow::bail!("ファイル同士でないと並べられません");
                }
                let l = cian_core::grepedit::read_text(&lp)?;
                let r = cian_core::grepedit::read_text(&rp)?;
                let lang = cian_core::highlight::detect(&lp).map(|x| format!("{x:?}"));
                // Both remembered, so `save` on either writes back through the
                // encoding it arrived with.
                self.open = Some((lp.clone(), l.clone(), cian_core::stamp::of(&lp)));
                self.pair = Some((rp.clone(), r.clone()));
                Ok(serde_json::json!({
                    "left": { "name": ln, "lines": l.lines, "encoding": format!("{:?}", l.encoding) },
                    "right": { "name": rn, "lines": r.lines, "encoding": format!("{:?}", r.encoding) },
                    "lang": lang,
                }))
            }
            // Save the right-hand side of a comparison.
            "savepair" => {
                let Some((path, original)) = self.pair.as_ref() else {
                    anyhow::bail!("並べているファイルがありません");
                };
                let lines: Vec<String> = lines_of(req).unwrap_or_default();
                let file = cian_core::grepedit::TextFile { lines, ..original.clone() };
                cian_core::grepedit::write_text(path, &file)?;
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let n = file.lines.len();
                self.pair = Some((path.clone(), file));
                Ok(serde_json::json!({ "saved": name, "lines": n }))
            }
            // One named file to one named directory. Used by the comparison
            // screen's `>` and `<`, where the two sides are not the two panes.
            // A single item copied or moved under a new name — the `r` answer
            // on the transfer sheet. The terminal build's shape (actions.rs,
            // TransferAs): a move is one rename; a copy lands under its own
            // name first and is then renamed, so the copy machinery stays one.
            "transferas" => {
                let src = std::path::PathBuf::from(req.params["src"].as_str().unwrap_or(""));
                let dest_dir = std::path::PathBuf::from(req.params["dest"].as_str().unwrap_or(""));
                let name = arg(req, "name");
                if name.is_empty() || name.contains('/') || name.contains('\\') {
                    anyhow::bail!("名前が正しくありません");
                }
                if !src.exists() {
                    anyhow::bail!("{} がありません", src.display());
                }
                let target = dest_dir.join(&name);
                if target.exists() {
                    anyhow::bail!("{} は既にあります", target.display());
                }
                if req.params["move"].as_bool().unwrap_or(false) {
                    std::fs::rename(&src, &target)?;
                    self.did(Undo::Moved { pairs: vec![(target.clone(), src)] });
                } else if src.is_dir() {
                    // A directory has to land under its own name first. If
                    // that spot is taken, going through it would overwrite a
                    // bystander on the way — refusing is the only safe answer.
                    let landed = dest_dir.join(src.file_name().unwrap_or_default());
                    if landed != target && landed.exists() {
                        anyhow::bail!("{} が既にあり、経由できません", landed.display());
                    }
                    cian_core::ops::copy_one(&src, &dest_dir, cian_core::ops::Conflict::Overwrite)?;
                    if landed != target {
                        std::fs::rename(&landed, &target)?;
                    }
                } else {
                    // A file goes straight to its new name — no stop at the
                    // old one, which may be occupied by something unrelated.
                    std::fs::copy(&src, &target)?;
                }
                for which in ["left", "right"] {
                    let _ = self.pane_mut(which).map(|p| p.reload());
                }
                Ok(serde_json::json!({ "to": target.display().to_string() }))
            }
            "copyone" => {
                let src = std::path::PathBuf::from(req.params["src"].as_str().unwrap_or(""));
                let dest = std::path::PathBuf::from(req.params["dest"].as_str().unwrap_or(""));
                if !src.exists() {
                    anyhow::bail!("{} がありません", src.display());
                }
                std::fs::create_dir_all(&dest)?;
                cian_core::ops::copy_one(&src, &dest, cian_core::ops::Conflict::Overwrite)?;
                for which in ["left", "right"] {
                    let _ = self.pane_mut(which).map(|p| p.reload());
                }
                Ok(serde_json::json!({ "copied": src.display().to_string() }))
            }
            // Write text the window composed — a comparison saved as a file.
            "writefile" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let name = arg(req, "name");
                if name.is_empty() || name.contains('/') || name.contains('\\') {
                    anyhow::bail!("名前が正しくありません");
                }
                let at = self.pane_mut(&which)?.cwd.join(&name);
                if at.exists() {
                    anyhow::bail!("{name} はすでにあります");
                }
                std::fs::write(&at, req.params["text"].as_str().unwrap_or(""))?;
                self.did(Undo::Created { path: at });
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                Ok(serde_json::json!({ "wrote": name }))
            }
            // `:g/re/d` and `:v/re/d` — keep or drop every matching line.
            //
            // The one line operation that is a *filter* rather than a
            // transform, and the one people reach for on a log: "everything
            // except the heartbeats", in one command.
            "grepdel" => {
                let lines: Vec<String> = lines_of(req).unwrap_or_default();
                let pattern = arg(req, "pattern");
                let keep = req.params["keep"].as_bool().unwrap_or(false);
                let re = regex::Regex::new(&pattern)
                    .map_err(|e| anyhow::anyhow!("正規表現が読めません: {e}"))?;
                let out: Vec<String> = lines
                    .iter()
                    .filter(|l| re.is_match(l) == keep)
                    .cloned()
                    .collect();
                Ok(serde_json::json!({
                    "lines": out,
                    "removed": lines.len() - out.len(),
                }))
            }
            // `:combine` — join lines onto one, with a space or without.
            "combine" => {
                let lines: Vec<String> = lines_of(req).unwrap_or_default();
                let at = req.params["at"].as_u64().unwrap_or(0) as usize;
                let count = (req.params["count"].as_u64().unwrap_or(2) as usize).max(2);
                let space = req.params["space"].as_bool().unwrap_or(true);
                if at >= lines.len() {
                    anyhow::bail!("行がありません");
                }
                let end = (at + count).min(lines.len());
                // Trimmed on the way in, because joining "foo   " to "  bar"
                // with a space is three spaces nobody asked for.
                let joined = lines[at..end]
                    .iter()
                    .map(|l| l.trim())
                    .collect::<Vec<_>>()
                    .join(if space { " " } else { "" });
                let mut out = lines[..at].to_vec();
                out.push(joined);
                out.extend_from_slice(&lines[end..]);
                Ok(serde_json::json!({ "lines": out, "joined": end - at }))
            }
            // Rectangular edits: cut, or put text down the left or right edge.
            "block" => {
                let lines: Vec<String> = lines_of(req).unwrap_or_default();
                let b = cian_core::textops::Block {
                    top: req.params["top"].as_u64().unwrap_or(0) as usize,
                    bottom: req.params["bottom"].as_u64().unwrap_or(0) as usize,
                    left: req.params["left"].as_u64().unwrap_or(0) as usize,
                    right: req.params["right"].as_u64().unwrap_or(0) as usize,
                };
                let text = arg(req, "text");
                use cian_core::textops as t;
                let out = match req.params["what"].as_str().unwrap_or("") {
                    "delete" => t::block_delete(&lines, b),
                    "insert" => t::block_insert(&lines, b, &text),
                    "append" => t::block_append(&lines, b, &text),
                    "replace" => t::block_replace(&lines, b, &text),
                    other => anyhow::bail!("知らない矩形操作: {other}"),
                };
                Ok(serde_json::json!({ "lines": out }))
            }
            // Read a file by path rather than by cursor.
            //
            // For a member extracted from an archive: the listing is inside
            // the archive, so the row's path names nothing on this disk and
            // the ordinary read cannot find it.
            // Which lines of what is on screen differ from what is on disk.
            //
            // **Neither build had this.** The editor knew it was dirty — one
            // bit, for the whole file — and a person who has been typing for
            // ten minutes wants to know *where*. Every other editor draws it
            // in the gutter and it is the cheapest orientation there is.
            //
            // Asked of the engine rather than worked out in the window,
            // because `cian_core::diff` is where lines are compared and a
            // second comparison written in JavaScript would disagree with the
            // first one eventually — the diff panel, the F7 hop and this would
            // then be three opinions about the same two files.
            "diskdiff" => {
                let path = std::path::PathBuf::from(arg(req, "path"));
                let now: Vec<String> = req.params["lines"]
                    .as_array()
                    .map(|a| {
                        a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect()
                    })
                    .unwrap_or_default();
                // A file that is not there yet — a new buffer, or one deleted
                // under us — is every line new rather than an error. The
                // gutter is orientation, and refusing to draw it is worse
                // than drawing "all of this is unsaved".
                let disk = match cian_core::grepedit::read_text(&path) {
                    Ok(text) => text.lines,
                    Err(_) => Vec::new(),
                };
                let (_, mine) = cian_core::diff::marks(&disk, &now);
                Ok(serde_json::json!({
                    "marks": mine
                        .iter()
                        .map(|m| match m {
                            cian_core::diff::Mark::Same => "same",
                            cian_core::diff::Mark::Changed => "changed",
                            cian_core::diff::Mark::Only => "new",
                        })
                        .collect::<Vec<_>>(),
                }))
            }
            "viewpath" => {
                let path = std::path::PathBuf::from(arg(req, "path"));
                if !path.is_file() {
                    anyhow::bail!("{} がありません", path.display());
                }
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let len = std::fs::metadata(&path)?.len();
                let shown = cian_core::viewer::view_file(&path)?;
                let binary = matches!(shown.kind, cian_core::viewer::ViewKind::Binary);
                let file = if binary { None } else { cian_core::grepedit::read_text(&path).ok() };
                let reply = serde_json::json!({
                    "name": name,
                    "path": path.display().to_string(),
                    "lines": shown.lines,
                    "bytes": len,
                    "binary": binary,
                    "truncated": shown.truncated,
                    "encoding": format!("{:?}", shown.encoding),
                    "eol": format!("{:?}", shown.eol),
                    "bom": shown.bom,
                    "lang": if binary {
                        None
                    } else {
                        cian_core::highlight::detect(&path).map(|l| format!("{l:?}"))
                    },
                });
                if binary {
                    self.open = None;
                    self.hex = Some((path.clone(), shown.clone()));
                } else {
                    self.hex = None;
                    self.open = file.map(|f| (path.clone(), f, cian_core::stamp::of(&path)));
                }
                self.shown = Some((path, shown));
                Ok(reply)
            }
            // Read a member of the archive being browsed.
            //
            // Extracted to a temporary file and opened from there, because
            // everything downstream — the viewer, the editor, the encoding
            // switch — works on a path. The temporary is remembered so a save
            // knows which member it came from.
            "archiveview" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (archive, sub) = {
                    let pane = self.pane_mut(&which)?;
                    let Some((a, s)) = pane.archive_view() else {
                        anyhow::bail!("アーカイブの中ではありません");
                    };
                    (a.to_path_buf(), s.to_string())
                };
                let (_, name, is_dir) = self.selected(&which)?;
                if is_dir {
                    anyhow::bail!("{name} はディレクトリです");
                }
                let member = format!("{sub}{name}");
                let dir = std::env::temp_dir().join(format!("cian-arc-{}", std::process::id()));
                std::fs::create_dir_all(&dir)?;
                let report = quietly(|ctl| cian_core::archive::extract(
                    &archive, std::slice::from_ref(&member), &dir, None, &sub, ctl));
                if report.ok == 0 {
                    anyhow::bail!("{name} を取り出せませんでした");
                }
                let at = dir.join(&name);
                let writable = zip_writable(&archive);
                self.member = Some((archive, member, at.clone()));
                Ok(serde_json::json!({
                    "path": at.display().to_string(),
                    "name": name,
                    "writable": writable,
                }))
            }
            // Put an edited member back into the archive it came from.
            //
            // Rebuilt rather than patched: a zip is a container with an index,
            // and rewriting one entry in place is how archives get corrupted.
            // cian-core's `zip_modify` writes a fresh one, which is slower and
            // is the only version that is safe to interrupt.
            "archivesave" => {
                let Some((archive, member, at)) = self.member.clone() else {
                    anyhow::bail!("アーカイブから開いたファイルがありません");
                };
                // zip only, and said so. `zip_modify` rebuilds a zip; pointed
                // at a tar it would write a zip with a tar's name, which is
                // strictly worse than refusing.
                if !zip_writable(&archive) {
                    anyhow::bail!("書き戻せるのは zip だけです（tar はまだ）");
                }
                // The window sends what it is holding, and that is written to
                // the temporary before it is packed. Relying on something else
                // having written the temporary already is how a save reports
                // success and repacks the file it extracted a moment ago —
                // which is exactly what the first version did.
                if let Some(lines) = lines_of(req) {
                    let original = cian_core::grepedit::read_text(&at)?;
                    let file = cian_core::grepedit::TextFile { lines, ..original };
                    cian_core::grepedit::write_text(&at, &file)?;
                }
                let prefix = member.rsplit_once('/').map(|(h, _)| format!("{h}/")).unwrap_or_default();
                let report = quietly(|ctl| cian_core::archive::zip_modify(
                    &archive, std::slice::from_ref(&member), &[], std::slice::from_ref(&at), &prefix, ctl));
                if !report.errors.is_empty() {
                    anyhow::bail!("{}", report.errors.join(" / "));
                }
                Ok(serde_json::json!({
                    "saved": member,
                    "archive": archive.file_name().map(|s| s.to_string_lossy().into_owned()),
                }))
            }
            // Rename or delete a member of the open zip.
            //
            // cian-tui's `r` and `d` while a listing is *inside* an archive
            // (arcview.rs) — the half that makes browsing one feel like
            // browsing a folder rather than looking at one through glass. Same
            // `zip_modify`, same reason it rebuilds rather than patches.
            // Add files to the zip the *other* pane is standing in. Split
            // from `archiveedit` (which works on a member of the archive you
            // are looking at) because the archive here is the destination,
            // not the thing under the cursor.
            // Redo a transfer that hit "permission denied" with administrator
            // rights (Windows UAC). The elevated process does the work itself,
            // so there is no progress to show — cian waits and reports.
            //
            // cian-tui has had this since the first Windows session; this
            // build said "permission denied" and stopped, which on a managed
            // machine is most of what a copy into Program Files ever says.
            "elevate" => {
                let dest = std::path::PathBuf::from(arg(req, "dest"));
                let move_after = req.params["kind"].as_str() == Some("move");
                let paths = paths_of(req);
                if paths.is_empty() || dest.as_os_str().is_empty() {
                    anyhow::bail!("やり直す対象がありません");
                }
                let items: Vec<cian_core::elevate::CopyItem> = paths
                    .iter()
                    .map(|src| cian_core::elevate::CopyItem {
                        src: src.clone(),
                        dest_dir: dest.clone(),
                    })
                    .collect();
                let n = items.len();
                cian_core::elevate::elevated_copy(&items, move_after)?;
                self.left.now().reload()?;
                self.right.now().reload()?;
                Ok(serde_json::json!({
                    "done": n,
                    "left": PaneView::of(self.left.get()),
                    "right": PaneView::of(self.right.get()),
                }))
            }
            // Does this path exist, and is it a folder?
            //
            // For the markdown preview: a README links to its neighbours, and
            // `./CONTRIBUTING.md` has to be told apart from `docs/` and from
            // a link that points at nothing — three different things to do,
            // and the window cannot read a disk to find out which.
            // Write the buffer somewhere else, leaving what is there alone.
            //
            // The way out of a conflict that keeps both: theirs stays where it
            // is, yours lands beside it under another name. Written through
            // the *open* file's encoding, BOM and line endings — a copy that
            // silently becomes UTF-8 is a copy that cannot be compared with
            // the original it was made from.
            "saveas" => {
                let to = std::path::PathBuf::from(arg(req, "path"));
                if to.as_os_str().is_empty() {
                    anyhow::bail!("保存場所が空です");
                }
                if to.exists() {
                    anyhow::bail!("{} は既にあります", to.display());
                }
                let Some((_, original, _)) = self.open.as_ref() else {
                    anyhow::bail!("開いているファイルがありません");
                };
                let lines: Vec<String> = lines_of(req).unwrap_or_default();
                let file = cian_core::grepedit::TextFile { lines, ..original.clone() };
                cian_core::grepedit::write_text(&to, &file)?;
                let name = to
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // The editor is now looking at the new file, not the old one:
                // a second Ctrl+S must not go back to the one we stepped away
                // from.
                let st = cian_core::stamp::of(&to);
                self.open = Some((to, file, st));
                Ok(serde_json::json!({ "saved": name }))
            }
            "stat" => {
                let path = std::path::PathBuf::from(arg(req, "path"));
                let meta = std::fs::metadata(&path).ok();
                Ok(serde_json::json!({
                    "exists": meta.is_some(),
                    "is_dir": meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                    "len": meta.as_ref().map(|m| m.len()).unwrap_or(0),
                }))
            }
            // What a transfer would actually move, before it is agreed to.
            //
            // **A folder confirmed as "1 件" is not a confirmation.** The sheet
            // named the rows and the rows were `proj/` — one line standing for
            // four thousand files and half a gigabyte over somebody else's
            // network, and no way to tell from the question.
            //
            // Counted with `plan_upload`, the same planner the transfer runs
            // on, so the sheet cannot promise a number the job then disagrees
            // with. Local sources only: walking a remote tree to answer a
            // dialog costs a round trip per directory, and the answer would
            // arrive after the person had already decided.
            "transferplan" => {
                let paths = paths_of(req);
                let mut files = 0usize;
                let mut bytes = 0u64;
                let mut rows = Vec::new();
                for p in &paths {
                    let plan = cian_scp::plan_upload(p, "/").unwrap_or_default();
                    let n = plan.files.len();
                    let b: u64 = plan
                        .files
                        .iter()
                        .filter_map(|(f, _)| std::fs::metadata(f).ok().map(|m| m.len()))
                        .sum();
                    files += n;
                    bytes += b;
                    rows.push(serde_json::json!({
                        "name": p.file_name().map(|s| s.to_string_lossy().into_owned()),
                        "is_dir": p.is_dir(),
                        "files": n,
                        "bytes": b,
                    }));
                }
                Ok(serde_json::json!({ "files": files, "bytes": bytes, "rows": rows }))
            }
            "zipadd" => {
                // Takes the *source* pane, like `copy` does, and finds the
                // archive on the other side itself. The window would otherwise
                // need the sub-path inside the zip, which it has never been
                // sent — and adding a field for one caller to hand straight
                // back is a second place for it to be wrong.
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let named = paths_of(req);
                let paths = if named.is_empty() { self.targets(&which)? } else { named };
                let (archive, sub) = {
                    let other = self.pane_mut(if which == "left" { "right" } else { "left" })?;
                    match other.archive_view() {
                        Some((a, sub)) => (a.to_path_buf(), sub.to_string()),
                        None => anyhow::bail!("反対のペインはアーカイブを開いていません"),
                    }
                };
                if paths.is_empty() {
                    anyhow::bail!("追加する対象がありません");
                }
                if !zip_writable(&archive) {
                    anyhow::bail!("これは書き換えられない形式です");
                }
                let report = archive_modify(&archive, &[], &[], &paths, &sub);
                if !report.errors.is_empty() {
                    anyhow::bail!("{}", report.errors.join(" / "));
                }
                // **Read the zip again, not the directory** — an archive view
                // is synthetic and `reload()` would only re-sort the rows it
                // already holds, so the addition would not appear until you
                // walked out and back in.
                let members = cian_core::archive::list(&archive)?;
                for side in ["left", "right"] {
                    let pane = self.pane_mut(side)?;
                    let here = pane.archive_view().map(|(a, s)| (a.to_path_buf(), s.to_string()));
                    if let Some((a, s)) = here {
                        if a == archive {
                            let rows = cian_core::archive::archive_rows(&a, &members, &s);
                            pane.enter_archive(a, s, rows);
                        }
                    }
                }
                Ok(serde_json::json!({
                    "added": report.ok,
                    "left": PaneView::of(self.left.get()),
                    "right": PaneView::of(self.right.get()),
                }))
            }
            "archiveedit" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let member = arg(req, "member");
                let to = arg(req, "to");
                let archive = {
                    let pane = self.pane_mut(&which)?;
                    let Some((a, _)) = pane.archive_view() else {
                        anyhow::bail!("アーカイブを開いていません");
                    };
                    a.to_path_buf()
                };
                if !zip_writable(&archive) {
                    anyhow::bail!("これは書き換えられない形式です");
                }
                if member.is_empty() {
                    anyhow::bail!("対象がありません");
                }
                let report = if to.is_empty() {
                    archive_modify(&archive, std::slice::from_ref(&member), &[], &[], "")
                } else {
                    archive_modify(&archive, &[], &[(member.clone(), to.clone())], &[], "")
                };
                if !report.errors.is_empty() {
                    anyhow::bail!("{}", report.errors.join(" / "));
                }
                // **Read the zip again, not the directory.** An archive view is
                // synthetic — its rows were made from the member list when you
                // walked in — so `reload()` only re-sorts and re-filters the
                // rows it already holds. The zip on disk had the new name and
                // the listing kept the old one until you left the directory
                // and came back, which is where "renamed it but the pane still
                // shows the old name" came from.
                let (archive, sub) = {
                    let pane = self.pane_mut(&which)?;
                    let Some((a, sub)) = pane.archive_view() else {
                        anyhow::bail!("アーカイブを開いていません");
                    };
                    (a.to_path_buf(), sub.to_string())
                };
                let members = cian_core::archive::list(&archive)?;
                let rows = cian_core::archive::archive_rows(&archive, &members, &sub);
                let pane = self.pane_mut(&which)?;
                let was = pane.cursor;
                pane.enter_archive(archive, sub, rows);
                // Stay where the hand was. A rebuilt listing that jumps to the
                // top makes renaming three files in a row three journeys.
                pane.cursor = was.min(pane.entries.len().saturating_sub(1));
                let mut reply = self.view(&which)?;
                reply["member"] = serde_json::json!(member);
                reply["to"] = serde_json::json!(to);
                Ok(reply)
            }
            // A link from the preview, in the desktop's browser.
            "openurl" => {
                let url = arg(req, "url");
                let lower = url.to_ascii_lowercase();
                // Checked again here, not only where the HTML was built. The
                // window is not the place a scheme should be trusted from.
                if !(lower.starts_with("http://") || lower.starts_with("https://")
                    || lower.starts_with("mailto:"))
                {
                    anyhow::bail!("開けないリンクです: {url}");
                }
                cian_core::proc::open_with_desktop(&url)?;
                Ok(serde_json::json!({ "opened": url }))
            }
            // A directory's entries, read without walking into it.
            //
            // For `:preview`, which shows what the cursor is on — and the
            // cursor passes over folders. Every other listing method *moves* a
            // pane, which is exactly what a preview must not do.
            "peekdir" => {
                let path = std::path::PathBuf::from(arg(req, "path"));
                if !path.is_dir() {
                    anyhow::bail!("ディレクトリではありません");
                }
                let mut rows: Vec<(bool, String)> = std::fs::read_dir(&path)?
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().into_string().ok()?;
                        Some((e.file_type().ok()?.is_dir(), name))
                    })
                    // A glance, not a browser. cian-tui caps this at 500 for
                    // the same reason (preview.rs `LIST_CAP`), and walking in
                    // is one keypress away.
                    .take(1000)
                    .collect();
                rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
                Ok(serde_json::json!({
                    "entries": rows.iter().take(500).map(|(d, n)| serde_json::json!({
                        "is_dir": d, "name": n,
                    })).collect::<Vec<_>>(),
                    "more": rows.len() > 500,
                }))
            }
            // The first or last lines of the selection, without opening it.
            "peek" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (path, name, is_dir) = self.selected(&which)?;
                if is_dir {
                    anyhow::bail!("{name} はディレクトリです");
                }
                let n = req.params["n"].as_u64().unwrap_or(10) as usize;
                let end = if req.params["tail"].as_bool().unwrap_or(false) {
                    cian_core::inspect::End::Tail
                } else {
                    cian_core::inspect::End::Head
                };
                let rows = cian_core::inspect::peek(&path, end, n)?;
                Ok(serde_json::json!({ "name": name, "rows": rows }))
            }
            // The transfer speed ceiling. `:limit 2m`, `:limit 500k`,
            // `:limit off`; bare `:limit` says what it is.
            "limit" => {
                let spec = arg(req, "spec");
                if !spec.is_empty() {
                    self.limit_bps = if matches!(spec.as_str(), "off" | "none" | "0") {
                        None
                    } else {
                        let (num, mul) = match spec.chars().last() {
                            Some('k' | 'K') => (&spec[..spec.len() - 1], 1024u64),
                            Some('m' | 'M') => (&spec[..spec.len() - 1], 1024 * 1024),
                            _ => (spec.as_str(), 1),
                        };
                        let n: u64 = num.trim().parse()
                            .map_err(|_| anyhow::anyhow!("読めません: {spec}（例 2m / 500k / off）"))?;
                        Some(n * mul)
                    };
                }
                Ok(serde_json::json!({ "bps": self.limit_bps }))
            }
            // The eighteen named palettes, from the same table the terminal
            // build reads. The window turns a spec into CSS properties; the
            // terminal turns it into ratatui colours; neither owns it.
            // The switches cian-tui keeps in its `T` menu and nowhere else.
            // Runtime-only in both builds: they are answers about *this
            // session*, and a verify that silently stayed on from last month
            // would be a surprise on the first big upload.
            "switches" => {
                if let Some(v) = req.params["verify"].as_bool() {
                    self.verify_transfers = v;
                }
                if let Some(v) = req.params["cloud"].as_bool() {
                    // Process-wide in cian-core, because every bulk reader
                    // asks it — the search, the checksum sweep, the duplicate
                    // finder. One flag rather than a parameter threaded
                    // through all of them.
                    cian_core::cloud::set_include(v);
                }
                Ok(serde_json::json!({
                    "verify": self.verify_transfers,
                    "cloud": cian_core::cloud::include(),
                }))
            }
            "themes" => Ok(serde_json::json!({
                "now": cian_lua::state_get("theme"),
                // The fourteen pane grounds, from the same table cian-tui
                // reads. `null` is "whatever the theme says", and is first —
                // the way back is the same list as the way in.
                "grounds": cian_core::theme::PANE_BG_PRESETS
                    .iter()
                    .map(|(name, rgb)| serde_json::json!({
                        "name": name,
                        "color": rgb.map(|c| format!("#{c:06x}")),
                    }))
                    .collect::<Vec<_>>(),
                "list": cian_core::theme::PRESETS
                    .iter()
                    .map(|(name, s)| serde_json::json!({
                        "name": name,
                        "light": s.is_light(),
                        "bg": format!("#{:06x}", s.bg),
                        "fg": format!("#{:06x}", s.fg),
                        "dim": format!("#{:06x}", s.dim),
                        "border": format!("#{:06x}", s.border),
                        "accent": format!("#{:06x}", s.accent),
                        "sel": format!("#{:06x}", s.sel),
                        "visual": format!("#{:06x}", s.visual),
                        "mark": format!("#{:06x}", s.mark),
                        "popup": format!("#{:06x}", s.popup),
                        "status": format!("#{:06x}", s.status),
                        "blue": format!("#{:06x}", s.blue),
                        "yellow": format!("#{:06x}", s.yellow),
                        "cyan": format!("#{:06x}", s.cyan),
                        "magenta": format!("#{:06x}", s.magenta),
                        "red": format!("#{:06x}", s.red),
                        "green": format!("#{:06x}", s.green),
                        "doc": format!("#{:06x}", s.doc),
                        // Derived here, by the terminal build's own rules, so
                        // the window is not left doing colour arithmetic that
                        // has to agree with somebody else's.
                        "on_accent": format!("#{:06x}", cian_core::theme::readable_on(s.accent)),
                        "on_sel": format!("#{:06x}", cian_core::theme::readable_on(s.sel)),
                        "accent_dim": format!("#{:06x}", cian_core::theme::toward(s.accent, s.bg, 0.85)),
                    }))
                    .collect::<Vec<_>>(),
            })),
            // Hand the file to Finder / Explorer, with it selected.
            //
            // The way out of cian into the rest of the machine: a file manager
            // that cannot say "show me this where the OS shows things" is a
            // place you have to leave by hand.
            // The three ways of handing a file to the desktop. All of them are
            // `cian_core::os` now, which is the terminal build's implementation
            // — this one used to be written here, and it built Explorer's
            // argument with `Command::arg`, so a path holding a space (a
            // OneDrive-redirected Desktop, say) was mis-quoted and Explorer
            // silently opened Documents. One verb, one implementation.
            "revealos" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (path, name, _) = self.selected(&which)?;
                cian_core::os::reveal(&path)
                    .map_err(|e| anyhow::anyhow!("開けませんでした: {e}"))?;
                Ok(serde_json::json!({ "revealed": name }))
            }
            // `:mermaid` — the file's diagrams, in a browser.
            //
            // The window draws them in the preview already; this is the other
            // half of what cian-tui's `:mermaid` is for, which is a diagram big
            // enough to read. Same extractor, same page, and the same
            // preference for a local `mermaid.min.js` beside the config over
            // the CDN — an offline machine has no CDN.
            "mermaid" => {
                let text = req.params["text"].as_str().unwrap_or("");
                let lines: Vec<String> = text.lines().map(str::to_string).collect();
                let blocks = cian_core::mermaid::extract_blocks(&lines);
                if blocks.is_empty() {
                    anyhow::bail!("mermaid ブロックがありません");
                }
                // The window draws them itself; it only needs the blocks, and
                // taking them from here keeps one extractor rather than a
                // second one in JavaScript that would disagree about what a
                // fence is the first time somebody used `~~~`.
                if !req.params["open"].as_bool().unwrap_or(true) {
                    return Ok(serde_json::json!({ "blocks": blocks }));
                }
                let dir = std::env::temp_dir().join("cian-mermaid");
                std::fs::create_dir_all(&dir)?;
                let local = cian_lua::config_read_path("mermaid.min.js").filter(|p| p.exists());
                let script = match &local {
                    Some(js) => {
                        let _ = std::fs::copy(js, dir.join("mermaid.min.js"));
                        "<script src=\"mermaid.min.js\"></script>\n<script>mermaid.initialize({startOnLoad:true});</script>".to_string()
                    }
                    None => "<script type=\"module\">import mermaid from \"https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs\";mermaid.initialize({startOnLoad:true});</script>".to_string(),
                };
                let page = dir.join("diagram.html");
                std::fs::write(&page, cian_core::mermaid::page(&blocks, &script))?;
                cian_core::proc::open_with_desktop(page.display().to_string())?;
                Ok(serde_json::json!({
                    "blocks": blocks.len(),
                    "offline": local.is_some(),
                }))
            }
            "openwith" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (path, name, _) = self.selected(&which)?;
                cian_core::os::open_with(&path)
                    .map_err(|e| anyhow::anyhow!("開けませんでした: {e}"))?;
                Ok(serde_json::json!({ "opened": name }))
            }
            "properties" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let (path, name, _) = self.selected(&which)?;
                cian_core::os::properties(&path)
                    .map_err(|e| anyhow::anyhow!("開けませんでした: {e}"))?;
                Ok(serde_json::json!({ "shown": name }))
            }
            // The input method, herded.
            //
            // The one thing a vim grammar cannot survive is an IME that stays
            // on in normal mode: `j` becomes かな and nothing moves. The
            // terminal build drives a helper (`macism`, `im-select`, cian-ime)
            // named in `cian.ime{…}`; the same helper works from here, and
            // the same three verbs cover it — off when keys become commands,
            // restore when they become text again, and the answer to "is this
            // even configured".
            "ime" => {
                let cfg = cian_lua::load();
                let Some(ime) = cfg.ime else {
                    anyhow::bail!("IME 連携が設定されていません（init.lua の cian.ime{{…}}）");
                };
                let run = |cmd: &str| -> anyhow::Result<String> {
                    let out = cian_core::proc::quiet(if cfg!(windows) { "cmd" } else { "sh" })
                        .args(if cfg!(windows) { ["/C", cmd] } else { ["-c", cmd] })
                        .output()?;
                    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
                };
                match req.params["do"].as_str().unwrap_or("") {
                    // Keys are about to be commands: remember what was on,
                    // switch to the no-IME source.
                    "off" => {
                        let Some(off) = ime.off.clone() else {
                            anyhow::bail!("off の入力ソースが設定されていません（cian.ime{{ off = … }}）");
                        };
                        if let Some(q) = ime.query_cmd() {
                            let now = run(&q)?;
                            if !now.is_empty() && now != off {
                                self.ime_saved = Some(now);
                            }
                        }
                        if let Some(cmd) = ime.set_cmd(&off) {
                            run(&cmd)?;
                        }
                        Ok(serde_json::json!({ "off": true }))
                    }
                    // Keys are text again: put back whatever was on.
                    "restore" => {
                        if let Some(saved) = self.ime_saved.take() {
                            if let Some(cmd) = ime.set_cmd(&saved) {
                                run(&cmd)?;
                            }
                            return Ok(serde_json::json!({ "restored": saved }));
                        }
                        Ok(serde_json::json!({ "restored": serde_json::Value::Null }))
                    }
                    _ => Ok(serde_json::json!({
                        "configured": true,
                        "current": ime.query_cmd().and_then(|q| run(&q).ok()),
                    })),
                }
            }
            // What just went wrong in the shell, explained.
            "aierror" => {
                let cfg = ai_config()?;
                let Some(text) = self.shell_now().and_then(|sh| sh.contents()) else {
                    anyhow::bail!("シェルが開いていません");
                };
                if text.trim().is_empty() {
                    anyhow::bail!("シェルにまだ何もありません");
                }
                let body: String = text.chars().take(8_000).collect();
                let system = cian_core::aiprompt::shell_error(cian_core::aiprompt::os_name());
                ai_in_background(self.out.clone(), cfg, system.to_string(), body.to_string(), |answer| Ok(serde_json::json!({ "answer": answer })));
                Ok(serde_json::json!({ "asked": true }))
            }
            // ---- The AI extension family ----
            //
            // Metadata only — names, kinds, sizes. File *contents* never leave
            // the machine from any of these, which is the terminal build's
            // rule and the only rule that makes them usable at work. Prompts
            // are the terminal build's, word for word; the reply is parsed and
            // validated here, against the real names, before the window sees
            // it — a model that invents a filename must not reach a delete key.
            "aijunk" | "aistructure" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let cfg = ai_config()?;
                let dir = self.pane_cwd(&which);
                // **Not on a remote pane.** `cwd` on one of those is whatever
                // local directory was there before the connection — so this
                // would walk *this* machine and label the result as the far
                // one's contents. Wrong quietly, which is the worst way to be
                // wrong: every row would look plausible.
                if self.pane_mut(&which)?.remote_view().is_some() {
                    anyhow::bail!(
                        "リモートペインでは使えません（この機能は手元のディスクを読みます）"
                    );
                }
                let junk = req.method == "aijunk";
                // **Junk nests and tidying does not.** A `node_modules` two
                // folders down is the commonest thing anybody wants gone, and
                // the old survey saw one level, so it was invisible. A
                // structure proposal, on the other hand, only ever moves the
                // loose entries of *this* directory — going deeper would show
                // the model files it is not allowed to touch.
                let limits = if junk {
                    cian_core::survey::Limits { depth: 4, rows: 800, hidden: false, ..Default::default() }
                } else {
                    cian_core::survey::Limits { depth: 1, rows: 600, hidden: false, ..Default::default() }
                };
                let (system, what) = if junk {
                    (cian_core::aiprompt::JUNK, "junk")
                } else {
                    (cian_core::aiprompt::STRUCTURE, "structure")
                };
                ai_survey_in_background(self.out.clone(), cfg, SurveyAsk {
                    head: format!("Directory: {}", dir.display()),
                    dir,
                    limits,
                    system,
                    what,
                    key: "name",
                });
                Ok(serde_json::json!({ "asked": true }))
            }
            "airename" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let instruction = arg(req, "instruction");
                if instruction.is_empty() {
                    anyhow::bail!("どうリネームするかを書いてください");
                }
                let cfg = ai_config()?;
                let paths = self.targets(&which)?;
                let rows: Vec<(String, String, bool, u64)> = paths
                    .iter()
                    .filter(|p| p.is_file())
                    .map(|p| (
                        p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                        p.display().to_string(),
                        false,
                        0,
                    ))
                    .collect();
                if rows.is_empty() {
                    anyhow::bail!("対象がありません");
                }
                let listing: String = rows.iter().take(400).map(|(n, ..)| format!("{n}\n")).collect();
                let system = cian_core::aiprompt::RENAME
                    .to_string();
                let user = format!("Instruction: {instruction}\n\nFiles:\n{listing}");
                ai_in_background(self.out.clone(), cfg, system.clone(), user.clone(), move |answer| {
                    let v = parse_ai_reply(&answer, &rows, "name")?;
                    Ok(serde_json::json!({ "what": "rename", "rows": v }))
                });
                Ok(serde_json::json!({ "asked": true }))
            }
            "aisearch" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let query = arg(req, "query");
                if query.is_empty() {
                    anyhow::bail!("何を探すかを書いてください");
                }
                if self.pane_mut(&which)?.remote_view().is_some() {
                    anyhow::bail!(
                        "リモートペインでは使えません（この機能は手元のディスクを読みます）"
                    );
                }
                let cfg = ai_config()?;
                let root = self.pane_cwd(&which);
                // Hidden included: somebody looking for "the eslint config"
                // means `.eslintrc`, and a search that cannot see dotfiles
                // fails on exactly the files people cannot remember the name
                // of. Breadth first, so a cap loses the deepest rather than
                // everything after the first big folder.
                let limits =
                    cian_core::survey::Limits { depth: 6, rows: 2000, hidden: true, ..Default::default() };
                ai_survey_in_background(self.out.clone(), cfg, SurveyAsk {
                    dir: root,
                    limits,
                    head: format!("Question: {query}"),
                    system: cian_core::aiprompt::SEARCH,
                    what: "search",
                    key: "path",
                });
                Ok(serde_json::json!({ "asked": true }))
            }
            // Carry out the structure plan the person approved: make the
            // folders, move the files, remember the moves for `u`.
            "organizeapply" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let cwd = self.pane_cwd(&which);
                let rows = req.params["rows"].as_array().cloned().unwrap_or_default();
                let mut moved: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
                let mut errors: Vec<String> = Vec::new();
                for row in &rows {
                    let (Some(path), Some(folder)) = (row["path"].as_str(), row["folder"].as_str())
                    else { continue };
                    // The model was told "no ..", and the engine checks anyway:
                    // instructions constrain the honest, not the confused.
                    if folder.contains("..") || std::path::Path::new(folder).is_absolute() {
                        errors.push(format!("{folder}: そこへは動かせません"));
                        continue;
                    }
                    let from = std::path::PathBuf::from(path);
                    let dest = cwd.join(folder);
                    if let Err(e) = std::fs::create_dir_all(&dest) {
                        errors.push(format!("{folder}: {e}"));
                        continue;
                    }
                    match cian_core::ops::move_one(&from, &dest, cian_core::ops::Conflict::Skip) {
                        Ok(_) => {
                            let name = from.file_name().map(|s| s.to_os_string()).unwrap_or_default();
                            moved.push((dest.join(name), from.clone()));
                        }
                        Err(e) => errors.push(format!("{}: {e}", from.display())),
                    }
                }
                if !moved.is_empty() {
                    self.did(Undo::Moved { pairs: moved.clone() });
                }
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                Ok(serde_json::json!({
                    "moved": moved.len(), "errors": errors, "pane": self.view(&which)?,
                }))
            }
            // The commit message, drafted from the staged diff. The prompt is
            // the terminal build's, word for word.
            "aicommit" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let cfg = ai_config()?;
                let dir = self.pane_cwd(&which);
                let Some(diff) = cian_core::git::staged_diff(&dir) else {
                    anyhow::bail!("git リポジトリではありません");
                };
                if diff.trim().is_empty() {
                    anyhow::bail!("ステージされていません。先に `git add`（:stage でも）");
                }
                let diff: String = diff.chars().take(12_000).collect();
                let system = cian_core::aiprompt::COMMIT;
                ai_in_background(self.out.clone(), cfg, system.to_string(), diff.to_string(), |answer| Ok(serde_json::json!({ "answer": answer })));
                Ok(serde_json::json!({ "asked": true }))
            }
            // Run the commit, with the message the person approved.
            "commit" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let message = arg(req, "message");
                if message.is_empty() {
                    anyhow::bail!("コミットメッセージがありません");
                }
                let dir = self.pane_cwd(&which);
                cian_core::git::commit(&dir, &message)?;
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                Ok(serde_json::json!({ "committed": true, "pane": self.view(&which)? }))
            }
            // The snippets init.lua declares, for Ctrl+Shift+Enter.
            "snippets" => {
                let cfg = cian_lua::load();
                Ok(serde_json::json!({
                    "rows": cfg.snippets.iter().map(|sn| serde_json::json!({
                        // `confirm` was read from init.lua and then dropped
                        // here, so a snippet marked "ask me first" was sent
                        // straight to the shell — the one flag whose whole
                        // purpose is to stop that.
                        "name": sn.name, "cmd": sn.cmd, "enter": sn.enter,
                        "confirm": sn.confirm,
                    })).collect::<Vec<_>>(),
                }))
            }

            // Tick or untick one task in the open Markdown file, by the line
            // it is on — `markdown::set_check`. Lines in, lines out: the
            // window puts them back into the editor and saves the ordinary
            // way, so a checkbox pressed in the preview goes through the same
            // conflict check as typing does.
            "check" => {
                let lines: Vec<String> = match lines_of(req) {
                    Some(l) => l,
                    None => match self.open.as_ref() {
                        Some((_, f, _)) => f.lines.clone(),
                        None => anyhow::bail!("開いているファイルがありません"),
                    },
                };
                let line = req.params["line"].as_u64().unwrap_or(0) as usize;
                let done = req.params["done"].as_bool().unwrap_or(false);
                let out = cian_core::markdown::set_check(&lines.join("\n"), line, done);
                Ok(serde_json::json!({
                    "lines": out.lines().map(str::to_string).collect::<Vec<_>>(),
                }))
            }
            // The open file as HTML, for the preview.
            //
            // Rendered here because the reading is here — the same parse the
            // terminal build draws, turned into a document instead of into
            // styled lines. The window is handed markup it did not have to
            // understand, which is also what keeps a README from running.
            "markdown" => {
                let lines: Vec<String> = match lines_of(req) {
                    Some(l) => l,
                    None => match self.open.as_ref() {
                        Some((_, f, _)) => f.lines.clone(),
                        None => anyhow::bail!("開いているファイルがありません"),
                    },
                };
                Ok(serde_json::json!({ "html": cian_core::markdown::to_html(&lines) }))
            }
            // Leave a flat listing and go back to the directory it came from.
            "leaveflat" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let pane = self.pane_mut(&which)?;
                if !pane.is_flat() {
                    anyhow::bail!("一覧はもともとのディレクトリです");
                }
                pane.leave_flat()?;
                self.view(&which)
            }
            // Where this pane has been. Its own history, not a shared one —
            // the two panes are two places at once, which is the point of two.
            "back" | "forward" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let pane = self.pane_mut(&which)?;
                let moved = if req.method == "back" { pane.go_back()? } else { pane.go_forward()? };
                if !moved {
                    anyhow::bail!(
                        "{}に履歴がありません",
                        if req.method == "back" { "前" } else { "先" }
                    );
                }
                self.view(&which)
            }
            "history" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let pane = self.pane_mut(&which)?;
                Ok(serde_json::json!({
                    "cwd": pane.cwd.display().to_string(),
                    "back": pane.history.iter().take(40)
                        .map(|p| p.display().to_string()).collect::<Vec<_>>(),
                    "forward": pane.forward.iter().take(40)
                        .map(|p| p.display().to_string()).collect::<Vec<_>>(),
                }))
            }
            // Rename in place. The name is a bare filename, never a path —
            // moving something is what `move` is for, and a rename that could
            // also move would make one confirm dialog have to explain two
            // things.
            "rename" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let to = req.params["name"].as_str().unwrap_or("").trim().to_string();
                if to.is_empty() {
                    anyhow::bail!("名前が空です");
                }
                if to.contains('/') || to.contains('\\') {
                    anyhow::bail!("名前に区切り文字は使えません: {to}");
                }
                let (from, _, _) = self.selected(&which)?;
                let dest = from.with_file_name(&to);
                if dest.exists() {
                    anyhow::bail!("{to} はすでにあります");
                }
                cian_core::ops::rename_in_place(&from, &to)?;
                self.did(Undo::Rename { from: from.clone(), to: dest });
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                self.view(&which)
            }
            // A new file or a new directory, in the pane being looked at.
            "create" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let name = arg(req, "name");
                let dir = req.params["dir"].as_bool().unwrap_or(false);
                if name.is_empty() {
                    anyhow::bail!("名前が空です");
                }
                let pane = self.pane_mut(&which)?;
                let at = pane.cwd.clone();
                let made = if dir && req.params["deep"].as_bool().unwrap_or(false) {
                    // `:mkdir -p a/b/c`. The undo remembers the *outermost*
                    // one made, because removing that removes the chain — and
                    // remembering the innermost would leave the rest behind.
                    let full = at.join(&name);
                    std::fs::create_dir_all(&full)?;
                    let first = name.split(['/', '\\']).next().unwrap_or(&name);
                    at.join(first)
                } else if dir {
                    cian_core::ops::create_dir(&at, &name)?
                } else if req.params["touch"].as_bool().unwrap_or(false) && at.join(&name).exists() {
                    // `:touch` on something that is already there bumps its
                    // time rather than failing, which is what touch means.
                    let full = at.join(&name);
                    std::fs::OpenOptions::new().append(true).open(&full)?;
                    filetime_now(&full)?;
                    full
                } else {
                    cian_core::ops::create_file(&at, &name)?
                };
                self.did(Undo::Created { path: made });
                let pane = self.pane_mut(&which)?;
                pane.reload()?;
                self.view(&which)
            }
            // One step back, whatever it was.
            "undo" | "redo" => {
                let taking = if req.method == "undo" { &self.undo } else { &self.redo };
                let Some(step) = taking.pop() else {
                    anyhow::bail!(
                        "{}操作はありません",
                        if req.method == "undo" { "取り消せる" } else { "やり直せる" }
                    );
                };
                if req.method == "undo" {
                    if let Some(back) = step.inverted() {
                        self.redo.push(back);
                    }
                } else if let Some(back) = step.inverted() {
                    self.undo.push(back);
                }
                let said = step.describe(req.method == "undo");
                match &step {
                    Undo::Rename { from, to } => {
                        let name = from
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        cian_core::ops::rename_in_place(to, &name)?;
                    }
                    Undo::Created { path } => {
                        // Straight off the disk rather than into the trash. It
                        // was made a moment ago and never had anything in it;
                        // putting it in the bin would leave litter to explain.
                        if path.is_dir() {
                            std::fs::remove_dir(path)?;
                        } else {
                            std::fs::remove_file(path)?;
                        }
                    }
                    Undo::Moved { pairs } => {
                        for (now, was) in pairs {
                            if let Some(parent) = was.parent() {
                                cian_core::ops::move_one(
                                    now,
                                    parent,
                                    cian_core::ops::Conflict::Skip,
                                )?;
                            }
                        }
                    }
                    Undo::Copied { paths } => {
                        // Only what is still there. The list was drawn up
                        // before the copy ran, so a file it never managed to
                        // write is on it — and `delete_many` would report a
                        // missing one as an error the person cannot act on.
                        let here: Vec<_> =
                            paths.iter().filter(|p| p.exists()).cloned().collect();
                        // To the trash, which is the point: this is the one
                        // step that undoes by deleting, and it stays a step
                        // that can itself be walked back.
                        let r = cian_core::ops::delete_many(
                            &here,
                            cian_core::ops::DeleteMode::Trash,
                        );
                        if let Some(first) = r.errors.first().cloned() {
                            // **Put back what could not be taken.** The step
                            // came off the stack before it ran, so bailing
                            // here would spend the only chance to undo this
                            // copy on an attempt that did nothing — and the
                            // commonest cause is a permission macOS is
                            // withholding, which is exactly the case where
                            // the person fixes it and presses the key again.
                            // Only the survivors: re-listing a file already
                            // in the trash would fail the retry every time.
                            let left: Vec<_> =
                                here.into_iter().filter(|p| p.exists()).collect();
                            if !left.is_empty() {
                                self.undo.push(Undo::Copied { paths: left });
                            }
                            anyhow::bail!("{first}");
                        }
                    }
                    Undo::Navigated { pane, from } => {
                        let p = self.pane_mut(pane)?;
                        *p = Pane::new(from.clone())?;
                    }
                }
                self.left.now().reload()?;
                self.right.now().reload()?;
                Ok(serde_json::json!({
                    "said": said,
                    "left": PaneView::of(self.left.get()),
                    "right": PaneView::of(self.right.get()),
                }))
            }
            // Narrow the listing to names containing this. Case-insensitive,
            // and it scopes everything downstream — marks, operations, the
            // count on the status line — because they all work off what is
            // shown rather than off what is there.
            "filter" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let text = req.params["text"].as_str().unwrap_or("").to_string();
                let pane = self.pane_mut(&which)?;
                if text.is_empty() {
                    pane.clear_filter();
                } else {
                    pane.set_filter(text);
                }
                self.view(&which)
            }
            "hidden" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let pane = self.pane_mut(&which)?;
                let now = !pane.show_hidden;
                pane.set_show_hidden(now);
                Ok(serde_json::json!({
                    "pane": PaneView::of(pane),
                    "showing": now,
                }))
            }
            "sort" => {
                use cian_core::{Sort, SortKey};
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let key = match req.params["key"].as_str().unwrap_or("name") {
                    "size" => SortKey::Size,
                    "date" | "modified" => SortKey::Modified,
                    "ext" | "extension" => SortKey::Extension,
                    _ => SortKey::Name,
                };
                let pane = self.pane_mut(&which)?;
                // The same key twice turns it round, which is what a column
                // heading does everywhere and what the hand expects — but a
                // caller that wants a *particular* order says so instead. The
                // cian view opens newest-first, and a toggle would have given
                // it oldest-first every second time you entered it.
                let reverse = match req.params["reverse"].as_bool() {
                    Some(want) => want,
                    None => pane.sort.key == key && !pane.sort.reverse,
                };
                pane.set_sort(Sort { key, reverse });
                Ok(serde_json::json!({
                    "pane": PaneView::of(pane),
                    "by": key.label(),
                    "reverse": reverse,
                }))
            }
            // Everything under the pane's directory, walked on a worker.
            // The picker opens on nothing and fills in; it never waits.
            "find" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let root = match which.as_str() {
                    "left" => self.left.get().cwd.clone(),
                    _ => self.right.get().cwd.clone(),
                };
                self.find.start(root.clone(), self.out.clone());
                Ok(serde_json::json!({ "root": root.display().to_string() }))
            }
            // The best of what has been found so far, for what has been typed
            // so far. Ranked here so there is one fuzzy matcher and not two.
            "rank" => {
                let query = req.params["query"].as_str().unwrap_or("");
                let limit = req.params["limit"].as_u64().unwrap_or(200) as usize;
                let rows: Vec<_> = self
                    .find
                    .rank(query, limit)
                    .into_iter()
                    .map(|h| {
                        serde_json::json!({
                            "rel": h.rel,
                            "path": h.full.display().to_string(),
                            "is_dir": h.is_dir,
                        })
                    })
                    .collect();
                Ok(serde_json::json!({ "rows": rows, "of": self.find.found() }))
            }
            // Take a pane to a found path — into it if it is a directory, to
            // its folder with the cursor on it if it is a file.
            "reveal" => {
                let which = req.params["pane"].as_str().unwrap_or("left").to_string();
                let path = PathBuf::from(req.params["path"].as_str().unwrap_or(""));
                let (dir, name) = if path.is_dir() {
                    (path.clone(), None)
                } else {
                    (
                        path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
                        path.file_name().map(|s| s.to_string_lossy().into_owned()),
                    )
                };
                let pane = self.pane_mut(&which)?;
                let was = pane.cwd.clone();
                *pane = Pane::new(dir)?;
                if let Some(n) = name {
                    if let Some(i) = pane.entries.iter().position(|e| e.name == n) {
                        pane.cursor = i;
                    }
                }
                if pane.cwd != was {
                    self.did(Undo::Navigated { pane: which.clone(), from: was });
                }
                self.view(&which)
            }
            "cancel" => {
                let op = req.params["op"].as_u64().unwrap_or(0);
                Ok(serde_json::json!({ "stopping": self.jobs.cancel(op) }))
            }
            other => Err(anyhow::anyhow!("no such method: {other}")),
        }
    }
}

fn main() -> anyhow::Result<()> {
    let start = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let out = Out::start();
    let mut session = Session::new(start, out.clone())?;

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // A line that is not a request at all still gets an answer, because a
        // front end waiting on an id it will never be sent is the worst way
        // for this to go wrong.
        match serde_json::from_str::<Request>(&line) {
            Ok(req) => match session.handle(&req) {
                Ok(ok) => out.reply(req.id, ok),
                Err(e) => out.fail(req.id, e),
            },
            Err(e) => out.send(serde_json::json!({
                "id": serde_json::Value::Null,
                "error": format!("bad request: {e}"),
            })),
        }
    }
    Ok(())
}

/// A path, safe to hand to a shell.
///
/// Single quotes with the single quote itself escaped the only way `sh`
/// accepts — close, escape, reopen. A space in a path is the common case and
/// an apostrophe in a filename is not rare enough to leave broken.
fn quote(s: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// What a browser will call this.
///
/// Only the ones a window can actually draw. Anything else is not offered as
/// an image at all, rather than handed over to be shown as a broken one.
fn mime_of(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

/// Base64, written out rather than pulled in.
///
/// One dependency avoided is one fewer crate in the offline bundle, and this
/// is a dozen lines that have not changed since 1987.
fn b64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Remote entries as pane rows.
///
/// The row's `path` holds the *remote* absolute path, which is what every
/// remote operation needs and what nothing on this disk should ever be asked
/// to open. The `..` row is synthetic; navigation intercepts it.
fn remote_rows(dir: &str, entries: &[cian_scp::RemoteEntry]) -> Vec<cian_core::Entry> {
    let mut rows = vec![cian_core::Entry::remote("..", dir.to_string(), true, 0, true)];
    for e in entries {
        rows.push(cian_core::Entry::remote(
            e.name.clone(),
            cian_scp::remote_join(dir, &e.name),
            e.is_dir,
            e.size,
            false,
        ));
    }
    rows
}

/// The last `cap` bytes of a file, decoded loosely.
///
/// A log's meaning is at its end: the head of a hundred-megabyte log is the
/// day it was created, and the question is always about today.
fn read_tail(path: &std::path::Path, cap: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len > cap {
        let _ = f.seek(SeekFrom::Start(len - cap));
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Set a file's modification time to now.
///
/// `:touch` on a file that already exists means "say this changed", which is
/// what half the uses of touch are for — a build that keys off mtime, a script
/// waiting on a marker.
fn filetime_now(path: &std::path::Path) -> std::io::Result<()> {
    let f = std::fs::OpenOptions::new().append(true).open(path)?;
    f.set_modified(std::time::SystemTime::now())
}

/// A shell-style glob as a regex, anchored.
///
/// `*.rs` is what a person types and `.*\.rs` is what they mean; treating the
/// input as a regex would make `.` match anything and quietly mark the wrong
/// files. Only `*` and `?` are special, which is the whole of what a filename
/// pattern has ever meant.
fn glob_to_regex(pattern: &str) -> anyhow::Result<regex::Regex> {
    let mut out = String::from("^");
    for c in pattern.to_lowercase().chars() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    out.push('$');
    Ok(regex::Regex::new(&out)?)
}

/// `~` at the start means home. Nothing else is expanded: a path typed into a
/// file manager is a path, not a shell line.
fn shellexpand(path: &str) -> String {
    // A SharePoint address is a path once Windows has been told how to read
    // it — `\\host@SSL\DavWWWRoot\…`, over WebDAV. Converted here because
    // this is the one door every typed path comes through, so `z`, the config
    // and anything later that takes a folder all understand one without each
    // having to learn how.
    let path = &cian_core::sharepoint::to_unc(path);
    let Some(rest) = path.strip_prefix('~') else { return path.to_string() };
    match std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        Some(home) => format!("{}{rest}", home.to_string_lossy()),
        None => path.to_string(),
    }
}

/// The `lines` argument, when a request carries one.
///
/// Four handlers had written this out — the text operations, the two saves and
/// the substitute — and all four mean the same thing: the window's current
/// idea of the file's contents, which may be ahead of what the engine last
/// read. Written four times it is four chances for one of them to start
/// treating a missing `lines` differently from the rest.
fn lines_of(req: &Request) -> Option<Vec<String>> {
    req.params["lines"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
}

/// A trimmed string argument, or empty when it was not given.
///
/// The trim is the point: a name typed into a prompt picks up whatever the
/// person's finger left on the end of it, and `"report.txt "` is a filename
/// nobody meant and every filesystem accepts.
fn arg(req: &Request, key: &str) -> String {
    req.params[key].as_str().unwrap_or("").trim().to_string()
}

/// Run an archive operation with a progress handle that reports to nobody.
///
/// The four archive calls all want a `Ctl`, and none of them has anywhere to
/// report: they answer in the time it takes to answer, so there is no bar to
/// feed. Passed in rather than returned, because a `Ctl` borrows the flag and
/// the closure it was built from and cannot outlive either.
fn quietly<T>(f: impl FnOnce(&mut cian_core::progress::Ctl) -> T) -> T {
    let stop = std::sync::atomic::AtomicBool::new(false);
    let mut noop = |_: &cian_core::progress::Progress| {};
    let mut ctl = cian_core::progress::Ctl { cancel: &stop, on_progress: &mut noop };
    f(&mut ctl)
}

/// Whether an edited member can go back into this archive.
///
/// `zip_modify` rebuilds zips; handed a tar it would write a zip under a
/// tar's name. The terminal build draws the same line ("F3 inside a *zip* …
/// it goes back into the zip"), so the answer is by container, up front.
/// Can this archive be written back to at all?
///
/// Every readable kind, now that a tarball can be rewritten — the name stays
/// `zip_writable` at the call sites' request rather than being renamed
/// everywhere at once. What it excludes is what `cian_core::archive::kind`
/// does not recognise.
/// The ground the window should open on, and which way round it is.
///
/// **Two axes, and I conflated them.** `gui_look` is the window's own look —
/// 白磁 / 陰翳 / 端末譲り, three of them — while `theme` is one of the
/// eighteen palettes in `cian_core::theme`. `by_name("hakuji")` is `None`
/// because hakuji is not a palette, and asking for it with `expect` **killed
/// the engine on a `settings` call**: the window then never opened at all.
/// A question about a colour must not be able to end the process.
///
/// The precedence is the renderer's own (`setPalette` resets `gui_look` to
/// 白磁, so a surviving non-白磁 look is by definition the later choice).
fn ground_of() -> serde_json::Value {
    const HAKUJI: (u32, bool) = (0x00f7_f8f8, true);
    let look = cian_lua::state_get("gui_look");
    let (bg, light) = match look.as_deref() {
        Some("inei") => (0x0014_110f, false),
        Some("terminal") => (0x000c_0c0c, false),
        // 白磁 — and then a palette, if one was chosen after it.
        _ => cian_lua::state_get("theme")
            .as_deref()
            .and_then(cian_core::theme::by_name)
            .map(|s| (s.bg, s.is_light()))
            .unwrap_or(HAKUJI),
    };
    serde_json::json!({ "bg": format!("#{bg:06x}"), "light": light })
}

fn zip_writable(archive: &std::path::Path) -> bool {
    cian_core::archive::is_archive(archive)
}

/// Change an archive, whichever kind it is.
///
/// **`tar` used to be the answer "no".** Every path into archive editing said
/// 「書き換えられるのは zip だけです（tar はまだ）」, which is a refusal that
/// gets written once and read for months. One function so the four call sites
/// do not each have to learn the difference.
fn archive_modify(
    archive: &std::path::Path,
    drop_members: &[String],
    rename_members: &[(String, String)],
    add_sources: &[std::path::PathBuf],
    add_prefix: &str,
) -> cian_core::ops::OpReport {
    let tar = matches!(
        cian_core::archive::kind(archive),
        Some(cian_core::archive::Kind::Tar | cian_core::archive::Kind::TarGz)
    );
    quietly(|ctl| {
        if tar {
            cian_core::archive::tar_modify(
                archive, drop_members, rename_members, add_sources, add_prefix, ctl,
            )
        } else {
            cian_core::archive::zip_modify(
                archive, drop_members, rename_members, add_sources, add_prefix, ctl,
            )
        }
    })
}

/// The `paths` argument. Four handlers take one — panelize, drop, the
/// desktop-drop upload and its clipboard twin — and all four mean the same
/// list of local files chosen in the window.
fn paths_of(req: &Request) -> Vec<std::path::PathBuf> {
    req.params["paths"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).map(std::path::PathBuf::from).collect())
        .unwrap_or_default()
}

/// A model's JSON reply, validated against the names that were actually sent.
///
/// The model was told to use names exactly as given, and the engine checks
/// anyway: instructions constrain the honest, not the confused, and an
/// invented filename must never reach a delete key. Rows that name nothing
/// real are dropped, not guessed at. `key` is which field carries the name —
/// the junk and rename prompts answer by `"name"`, the search by `"path"`.
/// The rows in the shape `parse_ai_reply` matches against: the identifier the
/// model was shown, then the absolute path the front end will act on.
fn survey_rows(rows: &[cian_core::survey::Row]) -> Vec<(String, String, bool, u64)> {
    rows.iter()
        .map(|r| (r.rel.clone(), r.path.display().to_string(), r.is_dir, r.size))
        .collect()
}

fn parse_ai_reply(
    answer: &str,
    rows: &[(String, String, bool, u64)],
    key: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let body = answer
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(body).map_err(|e| format!("AI の返事が読めません: {e}"))?;
    let mut out = Vec::new();
    for item in parsed {
        let Some(given) = item[key].as_str() else { continue };
        let Some((_, full, ..)) = rows.iter().find(|(n, ..)| n == given) else { continue };
        let mut v = item.clone();
        // Both spellings ride along so the window never joins names to paths
        // itself: `path` for the name-keyed answers, `full` for the path-keyed.
        v["path"] = serde_json::json!(full);
        v["full"] = serde_json::json!(full);
        out.push(v);
    }
    Ok(out)
}

/// Survey a tree *and* ask about it, both on a worker.
///
/// **The walk belongs out here for the same reason the chat does.** Totalling
/// subtree sizes over a Rust checkout is a second of `read_dir` — it was in
/// the request handler, where a second means every keystroke in the listing
/// queues behind it, which is the exact failure the note on
/// [`ai_in_background`] was written about. The old code read `pane.entries`
/// and was instant; making it recursive made it the slowest thing the engine
/// does synchronously, and nothing about the request said so.
///
/// The person is told the request started before any of this happens, which is
/// also why the "how much did it see" note now arrives with the *answer*
/// rather than with the acknowledgement: at acknowledgement time nobody has
/// looked yet.
struct SurveyAsk {
    /// Where to look.
    dir: std::path::PathBuf,
    limits: cian_core::survey::Limits,
    /// The first line of the user message — "Directory: …" or "Question: …".
    head: String,
    system: &'static str,
    /// The tag the front end switches on.
    what: &'static str,
    /// Which field of each returned object names a row: the tidy-up features
    /// answer with `name`, the search with `path`.
    key: &'static str,
}

fn ai_survey_in_background(out: Out, cfg: cian_ai::AiConfig, ask: SurveyAsk) {
    let SurveyAsk { dir, limits, head, system, what, key } = ask;
    std::thread::spawn(move || {
        let stop = std::sync::atomic::AtomicBool::new(false);
        let found = cian_core::survey::survey(&dir, limits, &stop);
        if found.rows.is_empty() {
            out.event("ai", serde_json::json!({ "error": "スキャンする対象がありません" }));
            return;
        }
        let rows = survey_rows(&found.rows);
        // To the person, as numbers: the front end says it in their language.
        // `limit_note` phrases the same fact in English for the model, and the
        // two must not become one string.
        let partial = found.partial().then(|| {
            serde_json::json!({
                "whole_to": found.whole_to(),
                "stopped": found.stopped_at.is_some(),
                "unopened": found.unopened,
            })
        });
        let user = cian_core::aiprompt::survey_user(&head, &found, std::time::SystemTime::now());
        match cian_ai::chat(&cfg, system, &user, &[]) {
            Ok(answer) => match parse_ai_reply(&answer, &rows, key) {
                Ok(v) => out.event(
                    "ai",
                    serde_json::json!({ "what": what, "rows": v, "partial": partial }),
                ),
                Err(e) => out.event("ai", serde_json::json!({ "error": e })),
            },
            Err(e) => out.event("ai", serde_json::json!({ "error": e.to_string() })),
        }
    });
}

/// One AI request, off the main loop, answered by an event.
///
/// Every AI command is this shape — chat on a worker, wrap the answer,
/// emit "ai" — and it was written out six times before the audit counted
/// them. `wrap` turns the raw answer into the event's payload and may
/// refuse it.
fn ai_in_background(
    out: Out,
    cfg: cian_ai::AiConfig,
    system: String,
    user: String,
    wrap: impl FnOnce(String) -> Result<serde_json::Value, String> + Send + 'static,
) {
    std::thread::spawn(move || match cian_ai::chat(&cfg, &system, &user, &[]) {
        Ok(answer) => match wrap(answer) {
            Ok(v) => out.event("ai", v),
            Err(e) => out.event("ai", serde_json::json!({ "error": e })),
        },
        Err(e) => out.event("ai", serde_json::json!({ "error": e.to_string() })),
    });
}

/// The AI config, or the one sentence that says how to get one.
fn ai_config() -> anyhow::Result<cian_ai::AiConfig> {
    cian_ai::AiConfig::from_lua(&cian_lua::load())
        .ok_or_else(|| anyhow::anyhow!("AI が設定されていません（init.lua の cian.ai{{…}}）"))
}

/// Read a transferred file back and compare it with what was sent.
///
/// cian-tui's own (`ssh.rs`), copied rather than re-invented: the local file
/// is hashed here and the remote one is streamed through the same hasher, so
/// nothing has to be downloaded twice to answer the question. An upload that
/// arrived short is a success as far as SFTP is concerned, and this is the
/// only thing that notices.
fn verify_transfer(
    target: &cian_scp::Target,
    remote_path: &str,
    local_path: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    use cian_core::attrs::{hash_file, HashKind, Hasher};
    let kind = HashKind::Sha256;
    let local = match hash_file(local_path, kind, cancel) {
        Ok(Some(h)) => h,
        Ok(None) => return Err("ベリファイを中止しました".into()),
        Err(e) => return Err(format!("ベリファイ: 手元のファイルが読めません: {e}")),
    };
    let mut hasher = Hasher::new(kind);
    if let Err(e) = cian_scp::remote_read(target, remote_path, cancel, &mut |b| hasher.update(b)) {
        return Err(format!("ベリファイできません: {e}"));
    }
    let remote = hasher.finish();
    if remote == local {
        Ok(())
    } else {
        let short = |s: &str| s.chars().take(12).collect::<String>();
        Err(format!("中身が違います — 手元 {}… ≠ 向こう {}…", short(&local), short(&remote)))
    }
}

/// The two stacks moved together. Free-standing so the worker in `jobs.rs`
/// and the tests reach the same rule the session uses.
fn did_step(undo: &crate::undo::Stack, redo: &crate::undo::Stack, step: Undo) {
    undo.push(step);
    redo.clear();
}

/// Ctrl+A: everything marked, always, as the terminal build has it
/// (actions.rs `mark_all`). It used to clear when anything was marked, so
/// Ctrl+A on a partial selection *unselected* — the opposite of what was
/// asked. Esc is the clearing gesture; `..` is skipped by `set_mark_at`.
fn mark_all(pane: &mut Pane) {
    for i in 0..pane.entries.len() {
        pane.set_mark_at(i);
    }
}

#[cfg(test)]
mod markall_tests {
    use super::*;

    #[test]
    fn a_partial_selection_becomes_everything_not_nothing() {
        let dir = std::env::temp_dir().join(format!("cian-markall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.join(n), "x").unwrap();
        }
        let mut pane = Pane::new(dir.clone()).unwrap();
        // One thing marked by hand — the state the toggle used to punish.
        let at = pane.entries.iter().position(|e| !e.is_parent).unwrap();
        pane.set_mark_at(at);
        mark_all(&mut pane);
        assert_eq!(pane.mark_count(), 3, "all three, not zero");
        // And `..` never rides along into an operation.
        assert!(pane.entries.iter().filter(|e| e.is_parent)
            .all(|e| !pane.marks.contains(&e.path)));
        // A second press keeps them — clearing is Esc's job.
        mark_all(&mut pane);
        assert_eq!(pane.mark_count(), 3);
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod undo_stack_tests {
    use super::*;

    #[test]
    fn something_new_empties_the_redo_side() {
        // The failure this pins: undo a move, then rename something, then
        // Ctrl+R — the stale branch must be gone, not replayed on top of the
        // rename. `Stack::push` does not clear it; `Session::did` must.
        let undo = crate::undo::Stack::default();
        let redo = crate::undo::Stack::default();
        redo.push(Undo::Rename { from: "/a".into(), to: "/b".into() });
        did_step(&undo, &redo, Undo::Created { path: "/c".into() });
        assert!(redo.pop().is_none(), "the undone branch is gone");
        assert!(undo.pop().is_some());
    }
}

#[cfg(test)]
mod ai_reply_tests {
    use super::*;

    fn rows() -> Vec<(String, String, bool, u64)> {
        vec![
            ("a.log".into(), "/tmp/x/a.log".into(), false, 10),
            ("b.txt".into(), "/tmp/x/b.txt".into(), false, 20),
        ]
    }

    #[test]
    fn a_fenced_reply_still_parses() {
        // Models fence JSON no matter how firmly they are told not to.
        let v = parse_ai_reply(
            "```json\n[{\"name\": \"a.log\", \"reason\": \"log\"}]\n```",
            &rows(),
            "name",
        )
        .unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0]["path"], "/tmp/x/a.log");
    }

    #[test]
    fn an_invented_name_is_dropped_not_guessed() {
        // The whole point of validating here: a hallucinated filename must
        // never reach a delete key with a real path attached.
        let v = parse_ai_reply(
            "[{\"name\": \"important.doc\", \"reason\": \"junk\"}, {\"name\": \"b.txt\", \"reason\": \"tmp\"}]",
            &rows(),
            "name",
        )
        .unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0]["name"], "b.txt");
    }

    #[test]
    fn prose_instead_of_json_is_an_error_not_a_crash() {
        assert!(parse_ai_reply("I think a.log is junk.", &rows(), "name").is_err());
    }
}
