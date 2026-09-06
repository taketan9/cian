//! Walking the last few things back.
//!
//! The shape is the terminal build's, arrived at again rather than invented:
//! five things are undoable, and one deliberately is not.
//!
//! **A copy is undone by deleting**, which needed an argument before it could
//! be trusted. The argument is in [`cian_core::ops::copy_creates`]: the step
//! remembers only the destination names that did not exist a moment earlier,
//! so it can never reach something the copy did not put there — and what it
//! does reach goes to the trash, not off the disk. A copy that landed on an
//! existing name is simply not on the stack.
//!
//! **A delete is not undone** — it went to the trash, which is the system's
//! own undo and already has a window for it.
//!
//! **Where you are is not on this stack.** It was, once, "in the order things
//! happened" — and in use that put every walk into a folder between your hand
//! and the file operation you wanted back. `u` is for what happened to your
//! files; the breadcrumb arrows walk the history.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// One step back.
#[derive(Debug, Clone)]
pub enum Undo {
    /// Rename `to` back to `from`.
    Rename { from: PathBuf, to: PathBuf },
    /// Remove what was just made. **Not redoable** — undoing it destroys it,
    /// and nothing here remembers what was inside.
    Created { path: PathBuf },
    /// Move each `.0` (where it is now) back to `.1` (where it was).
    Moved { pairs: Vec<(PathBuf, PathBuf)> },
    /// Send these to the trash: they are what a copy brought into being, and
    /// nothing else.
    ///
    /// `srcs` and `dest` are what the copy was told to do, kept so it can be
    /// done again. This used not to be redoable, on the grounds that the
    /// sources were not remembered — so they are remembered. Unlike
    /// [`Undo::Created`], nothing was destroyed: the sources are where they
    /// always were, and redoing is copying.
    Copied { srcs: Vec<PathBuf>, dest: PathBuf, paths: Vec<PathBuf> },
}

impl Undo {
    /// What just happened, for the person who pressed the key.
    ///
    /// Naming it is the point: `u` that says "done" leaves you wondering which
    /// of the last three things it took back. And the direction is a parameter
    /// rather than assumed, because Ctrl+R applies the very same step and
    /// saying "戻しました" there would describe the opposite of what happened.
    pub fn describe(&self, undoing: bool) -> String {
        let verb = if undoing { "戻しました" } else { "やり直しました" };
        match self {
            Undo::Rename { from, to } => {
                format!("{} → {} を{}", name_of(to), name_of(from), verb)
            }
            Undo::Created { path } => format!("{} を取り消しました", name_of(path)),
            Undo::Moved { pairs } => format!("{} 件の移動を{}", pairs.len(), verb),
            // Named as a trip to the trash rather than as a deletion, because
            // that is where they went and it is the difference that matters
            // to somebody who pressed the key by mistake.
            Undo::Copied { srcs, paths, .. } => {
                if undoing {
                    format!("{} 件のコピーを取り消しました（ゴミ箱へ）", paths.len())
                } else {
                    format!("{} 件をもう一度コピーしました", srcs.len())
                }
            }
        }
    }
}

fn name_of(p: &std::path::Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

/// The stack, shared with whatever thread finishes a move.
///
/// A move reports what it actually shifted only once it is over, and that
/// happens on a worker — so the stack cannot live behind the session's own
/// borrow.
#[derive(Clone, Default)]
pub struct Stack(Arc<Mutex<Vec<Undo>>>);

/// How far back `u` reaches. Deep enough to cover a wrong turn and the two
/// things before it, shallow enough that it is never a substitute for a
/// backup — which is a promise this cannot make.
const DEPTH: usize = 32;

impl Stack {
    pub fn push(&self, step: Undo) {
        let mut v = self.0.lock().unwrap();
        v.push(step);
        if v.len() > DEPTH {
            v.remove(0);
        }
    }

    pub fn pop(&self) -> Option<Undo> {
        self.0.lock().unwrap().pop()
    }

    /// Rewrite the step on top, if it is one.
    ///
    /// For the one case that needs it: a redone copy has to leave behind a
    /// fresh list of what *this* run created, and that is only knowable once
    /// the copy is under way — after the step has already been pushed.
    pub fn amend_top(&self, f: impl FnOnce(&mut Undo)) {
        if let Some(top) = self.0.lock().unwrap().last_mut() {
            f(top);
        }
    }

    /// Empty it. The redo stack is cleared whenever something *new* lands on
    /// the undo stack: once you have done something else, the branch you
    /// undid is gone, and replaying it would put files back on top of work
    /// done since.
    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

/// The other direction.
///
/// Not a second kind of step: what `u` undoes is described by the step it took
/// off the stack, and putting it back is the same description read the other
/// way — a rename swaps its two names, a move swaps its two places. Only the
/// two that cannot be undone at all have nothing to redo, and they never reach
/// here.
impl Undo {
    /// This step inverted, for the redo stack. `None` where undoing it
    /// destroyed what redoing it would need — a created file that has just
    /// been removed cannot be brought back with what is remembered here.
    pub fn inverted(&self) -> Option<Undo> {
        match self {
            Undo::Rename { from, to } => Some(Undo::Rename { from: to.clone(), to: from.clone() }),
            Undo::Moved { pairs } => Some(Undo::Moved {
                pairs: pairs.iter().map(|(now, was)| (was.clone(), now.clone())).collect(),
            }),
            // Its own inverse: the step carries both what to take back and
            // what to do again, and which of the two runs is decided by the
            // stack it is sitting on.
            Undo::Copied { srcs, dest, paths } => Some(Undo::Copied {
                srcs: srcs.clone(),
                dest: dest.clone(),
                paths: paths.clone(),
            }),
            Undo::Created { .. } => None,
        }
    }
}
