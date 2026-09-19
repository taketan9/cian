//! The Windows resources for `cian-tui.exe`: its icon and its Properties tab.
//!
//! **An icon file beside the program is not the program's icon.** `cian.ico`
//! travels in the zip and `main.js` hands it to Electron at runtime, which is
//! what the window's own taskbar button uses — but Explorer, the Start menu,
//! Alt+Tab and a pinned shortcut all read the icon out of the executable
//! itself, and a Rust exe has none. Until this file existed, every cian binary
//! on Windows wore the generic beige-window icon that means "some program".
//!
//! It has to live here rather than in `crates/cian-tui`, which is where the
//! rest of the terminal build is: a build script contributes resources to the
//! binaries of *its own* package, and the exe is produced by this one. The
//! same reasoning puts a copy in `cian-server`.
//!
//! Nothing happens off Windows — `winresource` is a Windows-only
//! build-dependency, so this whole module is absent from a Mac build rather
//! than being compiled and skipped.
//!
//! **Host, not target.** Cargo reads `[target.'cfg(windows)'.build-dependencies]`
//! against the machine doing the building, and `#[cfg(windows)]` here means the
//! same thing — so a Windows exe cross-compiled *from* Linux would come out
//! with no icon and nothing would say so. The release workflow builds Windows
//! on `windows-latest`, where host and target are the same; if that ever stops
//! being true, this is the line that has to change first.

fn main() {
    stamp();
    commit();
}

#[cfg(not(windows))]
fn stamp() {}

#[cfg(windows)]
fn stamp() {
    // From the repository root, where `packaging/icon.py` writes it and the
    // release workflow copies it from.
    let icon = concat!(env!("CARGO_MANIFEST_DIR"), "/../../cian.ico");
    // Loudly. The font taught this lesson once already: a packaging step that
    // prints a note and carries on ships a build that is quietly missing
    // something, and nothing downstream can tell.
    assert!(
        std::path::Path::new(icon).exists(),
        "cian.ico is missing from the repository root — run `python3 packaging/icon.py`"
    );
    println!("cargo:rerun-if-changed={icon}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon)
        // What the Properties tab says. Version and copyright come from
        // Cargo.toml on their own; these two do not, and without them the
        // dialog names the crate rather than the program.
        .set("ProductName", "cian")
        .set("FileDescription", "cian — the terminal build");
    res.compile().expect("stamping cian-tui.exe with its icon");
}

/// Bake the commit in, so `cian-tui --version` can identify a build. Without
/// it there is no way to tell which build is running, which has already cost
/// one debugging session chasing "missing features" that turned out to be an
/// older exe still on PATH.
fn commit() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    if let Ok(head) = std::fs::read_to_string("../../.git/HEAD") {
        if let Some(git_ref) = head.strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed=../../.git/{}", git_ref.trim());
        }
    }
}
