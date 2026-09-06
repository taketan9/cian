//! Work that outlives the call that asked for it.
//!
//! Copying four thousand files takes long enough that the front end must stay
//! answerable while it runs — the cursor still moves, the other pane still
//! reads. So the call returns an operation number at once and the work goes to
//! a thread, which reports against that number until it is finished.
//!
//! **One runs at a time, and the rest wait in line.** A second operation used
//! to be refused outright ("実行中です") — but the answer to "I have started a
//! long copy and now want to move something else" is not *no*, and the
//! terminal build has said so since it grew `:queue`. The queue is the reason
//! that key exists: a file manager copying ten thousand files should be able
//! to say which ten thousand, and let you drop one without stopping the rest.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cian_core::ops::{self, Conflict, DeleteMode, OpReport};
use cian_core::progress::{self, Ctl, Progress};

use crate::undo::{Stack, Undo};
use crate::wire::Out;

/// How often a running operation is allowed to speak.
///
/// A copy of ten thousand files that reported each one would put ten thousand
/// lines through the pipe for a bar that moves in pixels. Once every this many
/// milliseconds is enough to look continuous and cheap enough to ignore.
const REPORT_EVERY_MS: u128 = 80;

/// What a running operation is doing, for the front end's benefit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Move,
    Delete,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Copy => "copy",
            Kind::Move => "move",
            Kind::Delete => "delete",
        }
    }
}

/// What to do and how to do it — the operation and the caller's answers to
/// the two questions the confirmation sheet asks: what happens to a name that
/// is already there, and where a delete sends things.
#[derive(Clone, Copy)]
pub struct Plan {
    pub kind: Kind,
    pub conflict: Conflict,
    pub delete: DeleteMode,
}

impl Plan {
    /// The terminal build's defaults: skip what exists, trash what is deleted.
    pub fn of(kind: Kind) -> Self {
        Plan { kind, conflict: Conflict::Skip, delete: DeleteMode::Trash }
    }
}

/// Everything an operation needs, before it has a thread of its own.
struct Pending {
    op: u64,
    plan: Plan,
    paths: Vec<PathBuf>,
    dest: Option<PathBuf>,
    out: Out,
    undo: Stack,
    redo: Stack,
}

impl Pending {
    /// How many files, and where to — what `:queue` shows about a job that
    /// has not started and so has no progress to report.
    fn line(&self, state: &'static str) -> serde_json::Value {
        serde_json::json!({
            "op": self.op,
            "kind": self.plan.kind.name(),
            "total": self.paths.len(),
            "dest": self.dest.as_ref().map(|p| p.display().to_string()),
            "state": state,
            "stopping": false,
        })
    }
}

/// The operation actually on a thread right now.
struct Live {
    op: u64,
    kind: &'static str,
    total: usize,
    dest: Option<String>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Inner {
    next: AtomicU64,
    /// At most one. The terminal build runs one operation at a time — two
    /// copies competing for the same disk finish no sooner together and make
    /// the progress of each unreadable.
    running: Mutex<Option<Live>>,
    waiting: Mutex<VecDeque<Pending>>,
}

/// The operations in flight, so they can be numbered, listed and called off.
#[derive(Default)]
pub struct Jobs {
    inner: Arc<Inner>,
}

impl Jobs {
    /// Start one, or put it in line. Returns the number it will report under
    /// — immediately, before any file has been touched — and how many jobs
    /// are waiting ahead of it (`0` means it started).
    pub fn start(
        &self,
        plan: Plan,
        paths: Vec<PathBuf>,
        dest: Option<PathBuf>,
        out: Out,
        undo: Stack,
        redo: Stack,
    ) -> (u64, usize) {
        let op = self.inner.next.fetch_add(1, Ordering::Relaxed) + 1;
        let job = Pending { op, plan, paths, dest, out, undo, redo };
        // The two locks are never held at once, here or anywhere below.
        let busy = self.inner.running.lock().unwrap().is_some();
        if busy {
            let mut waiting = self.inner.waiting.lock().unwrap();
            waiting.push_back(job);
            return (op, waiting.len());
        }
        run(Arc::clone(&self.inner), job);
        (op, 0)
    }

    /// The line, runner first — for `:queue`.
    pub fn listing(&self) -> Vec<serde_json::Value> {
        let mut rows = Vec::new();
        if let Some(j) = self.inner.running.lock().unwrap().as_ref() {
            rows.push(serde_json::json!({
                "op": j.op,
                "kind": j.kind,
                "total": j.total,
                "dest": j.dest,
                "state": "running",
                "stopping": j.cancel.load(Ordering::Relaxed),
            }));
        }
        rows.extend(self.inner.waiting.lock().unwrap().iter().map(|j| j.line("waiting")));
        rows
    }

    /// Ask an operation to stop, or take it out of the line before it starts.
    ///
    /// A running one stops between files, never inside one — a half-copied
    /// file is worse than a slow cancel. A waiting one simply never happens,
    /// and says so, so nothing is left watching for a job that will not run.
    pub fn cancel(&self, op: u64) -> bool {
        if let Some(j) = self.inner.running.lock().unwrap().as_ref() {
            if j.op == op {
                j.cancel.store(true, Ordering::Relaxed);
                return true;
            }
        }
        let dropped = {
            let mut waiting = self.inner.waiting.lock().unwrap();
            match waiting.iter().position(|j| j.op == op) {
                Some(at) => waiting.remove(at),
                None => None,
            }
        };
        match dropped {
            Some(j) => {
                j.out.event(
                    "done",
                    serde_json::json!({
                        "op": op, "ok": 0, "skipped": 0,
                        "errors": [], "cancelled": true, "ms": 0,
                    }),
                );
                true
            }
            None => false,
        }
    }
}

/// Put one job on a thread, and start the next when it is done.
fn run(inner: Arc<Inner>, job: Pending) {
    let cancel = Arc::new(AtomicBool::new(false));
    *inner.running.lock().unwrap() = Some(Live {
        op: job.op,
        kind: job.plan.kind.name(),
        total: job.paths.len(),
        dest: job.dest.as_ref().map(|p| p.display().to_string()),
        cancel: Arc::clone(&cancel),
    });

    std::thread::spawn(move || {
        let Pending { op, plan, paths, dest, out, undo, redo } = job;
        let total = paths.len();
        out.event(
            "started",
            serde_json::json!({ "op": op, "kind": plan.kind.name(), "total": total }),
        );
        let began = std::time::Instant::now();

        // Both transfers can be undone, and both are derived before the work,
        // as the terminal build does (actions.rs finish_transfer) — afterwards
        // the destination looks the same either way. A move ends each target
        // at dest/<name> and undo moves it back; a copy is additive, so undo
        // removes what it added and `copy_creates` decides what that is. A
        // delete went to the trash, which has its own way back.
        let undo_step = match (plan.kind, dest.as_ref()) {
            (Kind::Move, Some(d)) => Some(Undo::Moved {
                pairs: paths
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| (d.join(n), p.clone())))
                    .collect(),
            }),
            (Kind::Copy, Some(d)) => match ops::copy_creates(&paths, d) {
                // Everything it was asked to copy is already there. Whatever
                // the conflict rule does next, none of it is this copy's to
                // take back, so there is no step rather than an empty one.
                made if made.is_empty() => None,
                // The sources and where they went ride along, so `Ctrl+R`
                // can run the same copy again.
                made => Some(Undo::Copied {
                    srcs: paths.clone(),
                    dest: d.clone(),
                    paths: made,
                }),
            },
            _ => None,
        };

        // Rate-limited, and always truthful about which file it is on. The
        // chunked copy calls back on every megabyte; forwarding all of that
        // would repaint far more often than a screen can show.
        let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let out_for_progress = out.clone();
        let mut on_progress = |p: &Progress| {
            if last.elapsed().as_millis() < REPORT_EVERY_MS {
                return;
            }
            last = std::time::Instant::now();
            out_for_progress.event(
                "progress",
                serde_json::json!({
                    "op": op,
                    "done": p.files_done,
                    "total": p.files_total.max(total),
                    "bytes": p.bytes_done,
                    "bytes_total": p.bytes_total,
                    "path": p.current,
                    "ms": began.elapsed().as_millis() as u64,
                }),
            );
        };
        let mut ctl = Ctl { cancel: &cancel, on_progress: &mut on_progress };

        // Through cian-core, which counts bytes as it goes. It was a loop of
        // `copy_one` here — the per-file calls that cannot say where inside a
        // file they have got to, so a single four-gigabyte file showed "1 / 1"
        // and then nothing for two minutes.
        let report: OpReport = match (plan.kind, dest.as_ref()) {
            (Kind::Copy, Some(d)) => progress::copy_many(&paths, d, plan.conflict, &mut ctl),
            (Kind::Move, Some(d)) => progress::move_many(&paths, d, plan.conflict, &mut ctl),
            (Kind::Delete, _) => ops::delete_many(&paths, plan.delete),
            // A copy or move with nowhere to go. Caught before starting, so
            // this is only here to make the match total.
            _ => {
                let mut r = OpReport::default();
                r.note_error("no destination");
                r
            }
        };

        let stopped = cancel.load(Ordering::Relaxed);
        if !stopped && report.ok > 0 {
            if let Some(step) = undo_step {
                crate::did_step(&undo, &redo, step);
            }
        }
        out.event(
            "done",
            serde_json::json!({
                "op": op,
                "ok": report.ok,
                "skipped": report.skipped,
                "errors": report.errors,
                "cancelled": stopped,
                // **Windows: offer the administrator retry.** cian-tui raises
                // `ConfirmElevate` here (actions.rs:3206); this build reported
                // "permission denied" and stopped, which on a managed machine
                // is most of what a copy into Program Files ever says. The
                // sources and the destination ride along so the window can ask
                // without having to remember what it just tried.
                "elevate": (report.permission_denied && cfg!(windows)).then(|| {
                    serde_json::json!({
                        "kind": if matches!(plan.kind, Kind::Move) { "move" } else { "copy" },
                        "paths": paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                        "dest": dest.as_ref().map(|d| d.display().to_string()),
                        // The answer already given rides along: a retry that
                        // silently overwrote what "skip" had just spared would
                        // be the one outcome the confirmation exists to stop.
                        "conflict": if matches!(plan.conflict, Conflict::Skip) { "skip" } else { "overwrite" },
                    })
                }),
                // How long it took, so the front end can say so and so that
                // "no progress was reported" can be told apart from "it was
                // over before there was anything to report".
                "ms": began.elapsed().as_millis() as u64,
            }),
        );

        // Out of the runner's seat before the next one takes it. Written here
        // rather than in a `Drop` guard because the next job must not start
        // while this thread still holds the lock.
        *inner.running.lock().unwrap() = None;
        let next = inner.waiting.lock().unwrap().pop_front();
        if let Some(next) = next {
            run(inner, next);
        }
    });
}

/// A transfer that crosses the local/remote boundary.
///
/// **`c` between a local pane and a remote one used to copy locally.** A
/// remote pane's `cwd` still points at the directory it walked in from, so
/// `other_cwd()` handed back a path on this machine, the file went there, and
/// the job said "1 copied". The server was never touched and nothing on
/// screen said so. cian-tui has had `try_remote_pane_transfer` since it grew a
/// remote pane; this is the same four cases for the engine.
pub enum Remote {
    /// Local files up to a directory on the server.
    Up { target: cian_scp::Target, files: Vec<PathBuf>, dest: String },
    /// Remote files down to a local directory.
    Down { target: cian_scp::Target, files: Vec<String>, dest: PathBuf },
    /// Server to server, relayed through this machine.
    ///
    /// There is no server-to-server SFTP, and a segmented network usually
    /// could not do A→B directly even if there were.
    Across {
        src: cian_scp::Target,
        dst: cian_scp::Target,
        files: Vec<String>,
        dest: String,
    },
}

impl Remote {
    fn count(&self) -> usize {
        match self {
            Remote::Up { files, .. } => files.len(),
            Remote::Down { files, .. } | Remote::Across { files, .. } => files.len(),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Remote::Up { .. } => "upload",
            Remote::Down { .. } => "download",
            Remote::Across { .. } => "relay",
        }
    }
}

impl Jobs {
    /// Start a cross-boundary transfer. It reports through the same
    /// `started` / `progress` / `done` events as a local operation, so the
    /// window's bar needs to know nothing about SFTP — and it takes an op
    /// number from the same counter, so `:queue` and cancel keep working.
    ///
    /// Off the local queue on purpose: a copy over the network and a copy over
    /// the disk are not competing for the same thing, and making one wait for
    /// the other would be a rule with no reason behind it.
    pub fn start_remote(
        &self,
        plan: Remote,
        cut: bool,
        limit: Option<u64>,
        out: Out,
    ) -> (u64, usize) {
        let op = self.inner.next.fetch_add(1, Ordering::Relaxed) + 1;
        // What was *asked for*. A folder becomes many files once the plan is
        // built, and the bar is told the real number then.
        let total = plan.count();
        let kind = plan.kind();
        let cancel = Arc::new(AtomicBool::new(false));
        // Registered as the running job so `:queue` lists it and Ctrl+C can
        // stop it. A transfer nobody can call off is the one thing worse than
        // a slow transfer.
        *self.inner.running.lock().unwrap() = Some(Live {
            op,
            kind,
            total,
            dest: None,
            cancel: Arc::clone(&cancel),
        });
        let inner = Arc::clone(&self.inner);
        std::thread::spawn(move || {
            out.event("started", serde_json::json!({ "op": op, "kind": kind, "total": total }));
            let began = std::time::Instant::now();
            let mut total = total;
            let mut ok = 0usize;
            let mut errors: Vec<String> = Vec::new();
            // Rate limited the same way a local copy is: the callback fires on
            // every chunk and a screen cannot show that.
            let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
            // `of` (the running total) is a parameter rather than something
            // the closure captures: the plan rewrites it once the tree is
            // walked, and a closure holding a borrow of it could not be called
            // afterwards.
            let mut report = |done: usize, all: usize, name: &str, bytes: u64, of: u64, force: bool| {
                if !force && last.elapsed().as_millis() < REPORT_EVERY_MS {
                    return;
                }
                last = std::time::Instant::now();
                out.event(
                    "progress",
                    serde_json::json!({
                        "op": op,
                        "done": done,
                        "total": all,
                        "bytes": bytes,
                        "bytes_total": of,
                        "path": name,
                        "ms": began.elapsed().as_millis() as u64,
                    }),
                );
            };
            match &plan {
                Remote::Up { target, files, dest } => {
                    // **A folder is a plan, not a transfer.** SFTP has no
                    // recursive put: every directory is its own `mkdir` and
                    // every file its own call, so the whole tree is worked out
                    // first — which is also what makes the count on the bar
                    // true from the start rather than growing as it goes.
                    let mut steps: Vec<(std::path::PathBuf, String)> = Vec::new();
                    for f in files {
                        match cian_scp::plan_upload(f, dest) {
                            Ok(p) => {
                                for d in &p.dirs {
                                    // An existing directory is not a failure:
                                    // this is `mkdir -p` said one level at a
                                    // time, and the second run of a transfer
                                    // finds every one of them already there.
                                    let _ = cian_scp::make_dir(target, d);
                                }
                                steps.extend(p.files);
                            }
                            Err(e) => errors.push(format!("{}: {e}", f.display())),
                        }
                    }
                    total = steps.len();
                    for (i, (from, at)) in steps.iter().enumerate() {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let shown = at.rsplit('/').next().unwrap_or(at).to_string();
                        let mut on = |b: u64, of: u64| report(i, total, &shown, b, of, false);
                        let mut ctl = cian_scp::Ctl { cancel: &cancel, on_progress: &mut on, limit_bps: limit };
                        match cian_scp::upload(target, from, at, None, &mut ctl) {
                            Ok(_) => ok += 1,
                            Err(e) => errors.push(format!("{shown}: {e}")),
                        }
                    }
                    // The sources go only once everything has landed, and only
                    // if everything did. A move that deletes as it goes leaves
                    // a half-moved tree on the far side and a half-deleted one
                    // here, and no way to tell which halves.
                    if cut && errors.is_empty() && !cancel.load(Ordering::Relaxed) {
                        for f in files {
                            let r = if f.is_dir() {
                                std::fs::remove_dir_all(f)
                            } else {
                                std::fs::remove_file(f)
                            };
                            if let Err(e) = r {
                                errors.push(format!("{}: {e}", f.display()));
                            }
                        }
                    }
                }
                Remote::Down { target, files, dest } => {
                    let mut steps: Vec<(std::path::PathBuf, String)> = Vec::new();
                    // Which of the named things turned out to be directories,
                    // so a move knows which call removes each of them.
                    let mut dirs: Vec<bool> = Vec::new();
                    for f in files {
                        match cian_scp::plan_download(target, f, dest) {
                            Ok(p) => {
                                dirs.push(!p.dirs.is_empty());
                                for d in &p.dirs {
                                    let _ = std::fs::create_dir_all(d);
                                }
                                steps.extend(p.files);
                            }
                            Err(e) => {
                                dirs.push(false);
                                errors.push(format!("{f}: {e}"));
                            }
                        }
                    }
                    total = steps.len();
                    for (i, (at, from)) in steps.iter().enumerate() {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let shown = from.rsplit('/').next().unwrap_or(from).to_string();
                        let mut on = |b: u64, of: u64| report(i, total, &shown, b, of, false);
                        let mut ctl = cian_scp::Ctl { cancel: &cancel, on_progress: &mut on, limit_bps: limit };
                        match cian_scp::download(target, from, at, &mut ctl) {
                            Ok(_) => ok += 1,
                            Err(e) => errors.push(format!("{shown}: {e}")),
                        }
                    }
                    // Only once everything has landed, and only if it all
                    // did. `remove` recurses for a directory, so one call per
                    // named item is the whole cleanup.
                    if cut && errors.is_empty() && !cancel.load(Ordering::Relaxed) {
                        for (f, was_dir) in files.iter().zip(dirs.iter()) {
                            if let Err(e) = cian_scp::remove(target, f, *was_dir) {
                                errors.push(format!("{f}: {e}"));
                            }
                        }
                    }
                }
                Remote::Across { src, dst, files, dest } => {
                    for (i, f) in files.iter().enumerate() {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let name = f.rsplit('/').next().unwrap_or(f).to_string();
                        // Through a temporary on this machine, and the
                        // temporary is removed whichever way the second leg
                        // goes — a relay that leaves its halves behind fills
                        // a disk nobody is watching.
                        let tmp = std::env::temp_dir()
                            .join(format!("cian-relay-{}-{}-{}", std::process::id(), op, i));
                        let shown = name.clone();
                        let step = {
                            let mut on = |b: u64, of: u64| report(i, total, &shown, b, of, false);
                            let mut ctl =
                                cian_scp::Ctl { cancel: &cancel, on_progress: &mut on, limit_bps: None };
                            cian_scp::download(src, f, &tmp, &mut ctl).and_then(|_| {
                                let at = cian_scp::remote_join(dest, &name);
                                cian_scp::upload(dst, &tmp, &at, None, &mut ctl)
                            })
                        };
                        let _ = std::fs::remove_file(&tmp);
                        match step {
                            Ok(_) => {
                                ok += 1;
                                if cut {
                                    if let Err(e) = cian_scp::remove(src, f, false) {
                                        errors.push(format!("{name}: {e}"));
                                    }
                                }
                            }
                            Err(e) => errors.push(format!("{name}: {e}")),
                        }
                    }
                }
            }
            let stopped = cancel.load(Ordering::Relaxed);
            report(ok, total, "", 0, 0, true);
            out.event(
                "done",
                serde_json::json!({
                    "op": op,
                    "ok": ok,
                    "skipped": 0,
                    "errors": errors,
                    "cancelled": stopped,
                    "elevate": serde_json::Value::Null,
                    "ms": began.elapsed().as_millis() as u64,
                }),
            );
            let mut running = inner.running.lock().unwrap();
            if running.as_ref().map(|j| j.op) == Some(op) {
                *running = None;
            }
        });
        (op, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Line;
    use std::sync::mpsc::Receiver;

    /// A sandbox with one source file big enough that copying it cannot be
    /// over before the next statement runs.
    ///
    /// Not a synchronisation primitive, and it does not pretend to be: the
    /// two `start` calls are microseconds apart and this copy takes tens of
    /// milliseconds anywhere, a margin of three orders of magnitude. What the
    /// test actually guarantees is the end state — both jobs finish, and the
    /// files land — which is what a lost or deadlocked queue would break.
    fn sandbox(name: &str) -> (PathBuf, PathBuf, Vec<PathBuf>) {
        let root = std::env::temp_dir().join(format!("cian-jobs-{}-{}", name, std::process::id()));
        let from = root.join("from");
        let to = root.join("to");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        let big = vec![7u8; 48 * 1024 * 1024];
        let mut paths = Vec::new();
        for i in 0..2 {
            let at = from.join(format!("big-{i}.bin"));
            std::fs::write(&at, &big).unwrap();
            paths.push(at);
        }
        (root, to, paths)
    }

    /// Every `done` event, in order, until `want` of them have arrived.
    fn wait_done(rx: &Receiver<Line>, want: usize) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while out.len() < want && std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_secs(30)) {
                Ok(line) => {
                    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                    if v["event"] == "done" {
                        out.push(v);
                    }
                }
                Err(_) => break,
            }
        }
        out
    }

    #[test]
    fn a_second_operation_waits_its_turn_and_then_runs() {
        // The failure this pins: the front end used to be told 実行中です and
        // the second operation simply did not happen. Starting one while
        // another runs must queue it — and the queue must actually drain.
        let (root, to, paths) = sandbox("queue");
        let (out, rx) = Out::piped();
        let (undo, redo) = (Stack::default(), Stack::default());
        let jobs = Jobs::default();

        let (first, ahead) = jobs.start(
            Plan::of(Kind::Copy), paths.clone(), Some(to.clone()),
            out.clone(), undo.clone(), redo.clone(),
        );
        assert_eq!(ahead, 0, "the first one starts");
        let second_dest = root.join("to2");
        std::fs::create_dir_all(&second_dest).unwrap();
        let (second, ahead) = jobs.start(
            Plan::of(Kind::Copy), paths.clone(), Some(second_dest.clone()),
            out.clone(), undo, redo,
        );
        assert_eq!(ahead, 1, "the second one waits rather than being refused");

        // And says so while it waits: one running, one queued behind it.
        let listing = jobs.listing();
        assert_eq!(listing.len(), 2);
        assert_eq!(listing[0]["state"], "running");
        assert_eq!(listing[1]["state"], "waiting");
        assert_eq!(listing[1]["op"], second);

        let dones = wait_done(&rx, 2);
        assert_eq!(dones.len(), 2, "both finished");
        assert_eq!(dones[0]["op"], first);
        assert_eq!(dones[1]["op"], second);
        for d in &dones {
            assert_eq!(d["ok"], 2);
            assert_eq!(d["cancelled"], false);
        }
        // The queue emptied itself; nothing is left holding the seat.
        assert!(jobs.listing().is_empty());
        assert!(second_dest.join("big-0.bin").exists(), "the queued copy really ran");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cancelling_a_waiting_job_takes_it_out_of_the_line() {
        // A job dropped before it starts still has to say `done`, or the
        // front end waits for ever for something that will never run.
        let (root, to, paths) = sandbox("cancel");
        let (out, rx) = Out::piped();
        let (undo, redo) = (Stack::default(), Stack::default());
        let jobs = Jobs::default();

        jobs.start(
            Plan::of(Kind::Copy), paths.clone(), Some(to.clone()),
            out.clone(), undo.clone(), redo.clone(),
        );
        let never = root.join("never");
        std::fs::create_dir_all(&never).unwrap();
        let (second, _) = jobs.start(
            Plan::of(Kind::Copy), paths.clone(), Some(never.clone()),
            out.clone(), undo, redo,
        );
        assert!(jobs.cancel(second), "a waiting job can be called off");

        let dones = wait_done(&rx, 2);
        let called_off = dones.iter().find(|d| d["op"] == second).expect("it said done");
        assert_eq!(called_off["cancelled"], true);
        assert!(!never.join("big-0.bin").exists(), "and never ran");
        std::fs::remove_dir_all(&root).ok();
    }
}
