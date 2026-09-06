//! Built-in remote file transfer over pure-Rust russh.
//!
//! cian's shell can already open an SSH session; this adds moving files across
//! one without shelling out to `scp`. Two wire protocols are supported and
//! chosen automatically over a single authenticated connection:
//!
//!   * **SFTP** — the modern subsystem (what today's `scp` uses under the hood).
//!     Tried first.
//!   * **SCP** — the classic `rcp`-style protocol, driven by exec'ing
//!     `scp -t`/`scp -f` on the server. Used as a fallback when the SFTP
//!     subsystem is disabled (some appliances and locked-down sshd configs),
//!     which is the whole reason a file manager still needs it.
//!
//! Each transfer runs from cian's ordinary worker threads: it spins a tiny
//! current-thread tokio runtime, reports progress through a callback and
//! watches a cancel flag, exactly like the local file operations.
//!
//! Host-key verification is not done yet — the server's key is accepted
//! unseen. That is a known gap (TeraTerm would prompt); it is called out at the
//! call site so a future change can add a known-hosts check.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, Context, Result};
use russh::client::{self, AuthResult, Handler};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Where to connect and who as. The password is resolved by the caller (from
/// the configured value or `password_cmd`) before we get here.
#[derive(Clone)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    /// A private key file to offer, if there is one.
    ///
    /// **cian could only ever log in with a password**, which is not how most
    /// people reach most servers — and on a host that takes keys only (a
    /// bastion, an appliance, anything hardened) there was no way in at all.
    /// `cian.ssh_hosts{ users = { { name = "…", key = "~/.ssh/id_ed25519" } } }`.
    pub key: Option<std::path::PathBuf>,
    /// The passphrase on that key, if it has one.
    pub key_pass: Option<String>,
}

/// Cancellation and progress, mirroring `cian_core::progress::Ctl`.
pub struct Ctl<'a> {
    pub cancel: &'a AtomicBool,
    /// Called with `(bytes_done, bytes_total)` as the transfer advances.
    pub on_progress: &'a mut dyn FnMut(u64, u64),
    /// Ceiling on the transfer rate, in bytes a second. `None` is as fast as
    /// the link will go.
    ///
    /// A copy to a server is a copy over somebody else's network, and a file
    /// manager that takes all of it is a file manager you cannot run during
    /// the day. See [`Pacer`].
    pub limit_bps: Option<u64>,
}

/// Holds a transfer to a rate by making it wait.
///
/// No token bucket and no averaging window: the honest question is "by now,
/// how long *should* this many bytes have taken?", and the answer is the only
/// thing to sleep for. Bursts are allowed to the size of one chunk, which is
/// what makes the first write immediate and the rate exact over any longer
/// stretch.
pub struct Pacer {
    limit: Option<u64>,
    started: std::time::Instant,
    sent: u64,
}

impl Pacer {
    pub fn new(limit_bps: Option<u64>) -> Self {
        Self { limit: limit_bps.filter(|b| *b > 0), started: std::time::Instant::now(), sent: 0 }
    }

    /// How long to wait after sending `n` more bytes. `None` means carry on.
    pub fn wait_after(&mut self, n: u64) -> Option<std::time::Duration> {
        let limit = self.limit?;
        self.sent += n;
        let owed = std::time::Duration::from_secs_f64(self.sent as f64 / limit as f64);
        let spent = self.started.elapsed();
        (owed > spent).then(|| owed - spent)
    }
}

/// Which wire protocol carried a transfer, so the UI can say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Sftp,
    Scp,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Transport::Sftp => "SFTP",
            Transport::Scp => "SCP",
        }
    }
}

/// How much to move per read/write; big enough to keep the link busy, small
/// enough that progress and cancel stay responsive.
const CHUNK: usize = 64 * 1024;

/// Accepts the server's key without checking it — see the module note.
struct BlindClient;

impl Handler for BlindClient {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Upload a local file to `remote_path` on the server. Tries SFTP, then falls
/// back to the classic SCP protocol if the SFTP subsystem is unavailable.
/// Returns which transport actually carried it.
/// Upload `local` to `remote_path`. `mode` (Unix permission bits, e.g. `0o777`)
/// is applied to the uploaded file when given.
pub fn upload(
    target: &Target,
    local: &Path,
    remote_path: &str,
    mode: Option<u32>,
    ctl: &mut Ctl,
) -> Result<Transport> {
    on_runtime(|| async {
        let handle = connect(target).await?;
        let total = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
        match open_sftp(&handle).await {
            Ok(sftp) => {
                sftp_upload(&sftp, local, remote_path, total, mode, ctl).await?;
                Ok(Transport::Sftp)
            }
            Err(_) => {
                scp_upload(&handle, local, remote_path, total, mode, ctl).await?;
                Ok(Transport::Scp)
            }
        }
    })
}

/// Download `remote_path` from the server to a local file. Tries SFTP, then
/// falls back to the classic SCP protocol. Returns which transport carried it.
pub fn download(
    target: &Target,
    remote_path: &str,
    local: &Path,
    ctl: &mut Ctl,
) -> Result<Transport> {
    on_runtime(|| async {
        let handle = connect(target).await?;
        match open_sftp(&handle).await {
            Ok(sftp) => {
                sftp_download(&sftp, remote_path, local, ctl).await?;
                Ok(Transport::Sftp)
            }
            Err(_) => {
                scp_download(&handle, remote_path, local, ctl).await?;
                Ok(Transport::Scp)
            }
        }
    })
}

/// Stream a remote file's bytes through `on_bytes`, so a caller can verify a
/// transfer by re-reading the file and hashing it on its own side.
///
/// SFTP only: the classic SCP path is driven by exec'ing a one-shot command and
/// is not a dependable second reader, so an [`Err`] here means "verification
/// unavailable" (no SFTP subsystem, or the read failed) rather than "the file is
/// bad". Honours the cancel flag between chunks.
pub fn remote_read(
    target: &Target,
    remote_path: &str,
    cancel: &AtomicBool,
    on_bytes: &mut dyn FnMut(&[u8]),
) -> Result<()> {
    on_runtime(|| async {
        let handle = connect(target).await?;
        let sftp = open_sftp(&handle)
            .await
            .context("this server has no SFTP subsystem, so a transfer cannot be verified")?;
        let mut src = sftp
            .open(remote_path)
            .await
            .with_context(|| format!("open remote {} for verify", remote_path))?;
        let mut buf = vec![0u8; CHUNK];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("cancelled"));
            }
            let n = src.read(&mut buf).await.context("read remote")?;
            if n == 0 {
                break;
            }
            on_bytes(&buf[..n]);
        }
        let _ = sftp.close().await;
        Ok(())
    })
}

/// One entry in a remote directory listing (for the download browser).
#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// The name was a symlink, and `is_dir` describes what it points at.
    ///
    /// Kept so the listing can say so; navigation only cares about `is_dir`.
    pub link: bool,
}

/// Join a remote directory and a name, POSIX-style — the only separator SFTP
/// knows, whatever the local platform uses.
///
/// Public because the GUI's engine builds its remote rows too, and a remote
/// path assembled with a backslash on Windows names nothing on the server.
pub fn remote_join(dir: &str, name: &str) -> String {
    join(dir, name)
}

/// One level up, POSIX-style. The root is its own parent, so climbing past it
/// stays put rather than producing an empty path nothing will list.
pub fn remote_parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", _)) | None => "/".to_string(),
        Some((head, _)) => head.to_string(),
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Everything a transfer will actually do, worked out before it starts.
///
/// **SFTP has no recursive put or get.** Every file is its own call and every
/// directory has to be made first, so a folder is not one transfer — it is a
/// plan. Both builds refused folders outright rather than half-sending one,
/// which was the right refusal and the wrong feature.
///
/// Worked out up front for the same reason a local copy counts its files up
/// front: a progress bar that learns its own total halfway is a bar that goes
/// backwards.
#[derive(Debug, Default, Clone)]
pub struct Plan {
    /// Directories to create, shallowest first — a parent has to exist before
    /// its child, and `mkdir -p` is not a thing SFTP offers either.
    pub dirs: Vec<String>,
    /// `(local, remote)` for an upload, in the order they should go.
    pub files: Vec<(std::path::PathBuf, String)>,
}

/// What uploading `src` into the remote directory `dest_dir` involves.
///
/// A file is one entry and no directories. A folder is every file beneath it,
/// with the folder itself (and each sub-folder) listed to be made. Symlinks
/// are skipped rather than followed: a link that resolves outside the tree
/// would copy something the person did not name, and one that resolves inside
/// it would copy the same bytes twice.
pub fn plan_upload(src: &Path, dest_dir: &str) -> Result<Plan> {
    let mut plan = Plan::default();
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("{}: unusable name", src.display()))?;
    let meta = std::fs::symlink_metadata(src)
        .with_context(|| format!("read {}", src.display()))?;
    if meta.file_type().is_symlink() {
        return Ok(plan);
    }
    if meta.is_file() {
        plan.files.push((src.to_path_buf(), join(dest_dir, name)));
        return Ok(plan);
    }
    // Breadth first, so `dirs` comes out with parents before children and the
    // caller can create them in order without sorting.
    let root = join(dest_dir, name);
    plan.dirs.push(root.clone());
    let mut queue = std::collections::VecDeque::from([(src.to_path_buf(), root)]);
    while let Some((here, there)) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(&here) else { continue };
        let mut kids: Vec<_> = rd.flatten().collect();
        // A stable order, because a plan that shuffles between runs cannot be
        // compared with the last one when something goes wrong halfway.
        kids.sort_by_key(|e| e.file_name());
        for e in kids {
            let Some(kid) = e.file_name().to_str().map(str::to_string) else { continue };
            let ft = match e.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue;
            }
            let at = join(&there, &kid);
            if ft.is_dir() {
                plan.dirs.push(at.clone());
                queue.push_back((e.path(), at));
            } else {
                plan.files.push((e.path(), at));
            }
        }
    }
    Ok(plan)
}

/// The same for a download: what comes off the server, and which local
/// directories have to exist first.
///
/// `dirs` here are local paths as strings, for symmetry with [`Plan`]; the
/// caller turns them back into paths. One SFTP round trip per directory, which
/// is why the plan is built once and kept.
pub fn plan_download(target: &Target, src: &str, dest_dir: &Path) -> Result<Plan> {
    let mut plan = Plan::default();
    let name = src.rsplit('/').next().unwrap_or(src).to_string();
    if name.is_empty() {
        return Err(anyhow!("{src}: unusable name"));
    }
    // Is it a directory at all? `list_dir` on a file fails, and that failure is
    // the answer rather than an error to report.
    let Ok((_, entries)) = list_dir(target, src) else {
        plan.files.push((dest_dir.join(&name), src.to_string()));
        return Ok(plan);
    };
    let root = dest_dir.join(&name);
    plan.dirs.push(root.display().to_string());
    let mut queue = std::collections::VecDeque::from([(src.to_string(), root, entries)]);
    while let Some((there, here, entries)) = queue.pop_front() {
        for e in entries {
            if e.link {
                continue;
            }
            let at_remote = join(&there, &e.name);
            let at_local = here.join(&e.name);
            if e.is_dir {
                plan.dirs.push(at_local.display().to_string());
                if let Ok((_, kids)) = list_dir(target, &at_remote) {
                    queue.push_back((at_remote, at_local, kids));
                }
            } else {
                plan.files.push((at_local, at_remote));
            }
        }
    }
    Ok(plan)
}

/// List a remote directory over SFTP (browsing needs the SFTP subsystem; the
/// classic SCP protocol cannot enumerate). Directories sort first, then by name.
///
/// Returns the *canonical absolute* path alongside the entries: the caller may
/// pass a relative path like "." (the login home), and resolving it to e.g.
/// `/home/userA` is what lets the browser climb up past the home directory all
/// the way to `/`.
pub fn list_dir(target: &Target, remote_path: &str) -> Result<(String, Vec<RemoteEntry>)> {
    on_runtime(|| async {
        let handle = connect(target).await?;
        let sftp = open_sftp(&handle)
            .await
            .context("this server has no SFTP subsystem, so remote browsing is unavailable")?;
        // Resolve "." / relative paths to an absolute path so parent navigation
        // has something to climb; fall back to the input if the server refuses.
        let canon = sftp
            .canonicalize(remote_path)
            .await
            .unwrap_or_else(|_| remote_path.to_string());
        let read = sftp.read_dir(remote_path).await.context("read remote directory")?;
        let mut out = Vec::new();
        for entry in read {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let meta = entry.metadata();
            let mut is_dir = meta.is_dir();
            let mut size = meta.size.unwrap_or(0);
            // READDIR reports what `lstat` would: a symlink is a symlink, even
            // when it points at a directory. Left at that, `/var/www` and every
            // other linked directory on a server listed as a file — the row
            // could not be entered, and Enter on it did nothing at all. So each
            // link is followed once, with `stat`, and described by what it
            // actually is. A dangling one keeps the answer readdir gave.
            let link = entry.file_type().is_symlink();
            if link {
                if let Ok(target) = sftp.metadata(join(&canon, &name)).await {
                    is_dir = target.is_dir();
                    size = target.size.unwrap_or(size);
                }
            }
            out.push(RemoteEntry { is_dir, size, name, link });
        }
        out.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let _ = sftp.close().await;
        Ok((canon, out))
    })
}

/// Create a directory on the server (single level; the parent must exist).
pub fn make_dir(target: &Target, path: &str) -> Result<()> {
    on_runtime(|| async {
        let handle = connect(target).await?;
        let sftp = open_sftp(&handle).await.context("SFTP subsystem unavailable")?;
        let r = sftp.create_dir(path).await.with_context(|| format!("mkdir {path}"));
        let _ = sftp.close().await;
        r
    })
}

/// Create an empty file on the server (touch). Refuses to clobber an existing
/// path.
pub fn make_file(target: &Target, path: &str) -> Result<()> {
    on_runtime(|| async {
        let handle = connect(target).await?;
        let sftp = open_sftp(&handle).await.context("SFTP subsystem unavailable")?;
        let r = async {
            if sftp.metadata(path).await.is_ok() {
                anyhow::bail!("{path} already exists");
            }
            sftp.create(path).await.with_context(|| format!("touch {path}"))?;
            Ok(())
        }
        .await;
        let _ = sftp.close().await;
        r
    })
}

/// Rename / move a remote entry within the server.
/// Set the mode of a remote file. The octal a person typed, as SFTP wants it.
///
/// Only the permission bits: SFTP's `set_metadata` takes a whole attribute
/// block, and sending anything else in it would quietly change something the
/// person did not ask about.
pub fn chmod(target: &Target, path: &str, mode: u32) -> Result<()> {
    on_runtime(|| async {
        let handle = connect(target).await?;
        let sftp = open_sftp(&handle).await.context("SFTP subsystem unavailable")?;
        let attrs = russh_sftp::protocol::FileAttributes {
            permissions: Some(mode & 0o7777),
            ..Default::default()
        };
        let r = sftp
            .set_metadata(path, attrs)
            .await
            .with_context(|| format!("chmod {mode:o} {path}"));
        let _ = sftp.close().await;
        r
    })
}

pub fn rename(target: &Target, from: &str, to: &str) -> Result<()> {
    on_runtime(|| async {
        let handle = connect(target).await?;
        let sftp = open_sftp(&handle).await.context("SFTP subsystem unavailable")?;
        let r = sftp.rename(from, to).await.with_context(|| format!("rename {from} → {to}"));
        let _ = sftp.close().await;
        r
    })
}

/// Remove a remote file, or a remote directory **and everything inside it**
/// (recursively — SFTP `rmdir` only removes an empty directory, so the tree is
/// walked and emptied first). All within one SFTP session.
pub fn remove(target: &Target, path: &str, is_dir: bool) -> Result<()> {
    on_runtime(|| async {
        let handle = connect(target).await?;
        let sftp = open_sftp(&handle).await.context("SFTP subsystem unavailable")?;
        let r = if is_dir {
            remove_tree(&sftp, path).await.with_context(|| format!("remove {path}"))
        } else {
            sftp.remove_file(path).await.with_context(|| format!("remove {path}"))
        };
        let _ = sftp.close().await;
        r
    })
}

/// Delete a remote directory tree: gather it depth-first, unlinking files as we
/// go, then remove the now-empty directories deepest-first. Iterative (no async
/// recursion), reusing the one session.
async fn remove_tree(sftp: &SftpSession, root: &str) -> Result<()> {
    let mut to_scan = vec![root.to_string()];
    let mut dirs: Vec<String> = Vec::new(); // parents before children
    while let Some(dir) = to_scan.pop() {
        let entries = sftp.read_dir(&dir).await.with_context(|| format!("read {dir}"))?;
        dirs.push(dir.clone());
        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child = format!("{}/{}", dir.trim_end_matches('/'), name);
            if entry.metadata().is_dir() {
                to_scan.push(child);
            } else {
                sftp.remove_file(&child).await.with_context(|| format!("remove {child}"))?;
            }
        }
    }
    // Children appear after their parent, so removing in reverse empties the
    // deepest directories first.
    for dir in dirs.iter().rev() {
        sftp.remove_dir(dir).await.with_context(|| format!("rmdir {dir}"))?;
    }
    Ok(())
}

// ── SFTP ────────────────────────────────────────────────────────────────────

async fn sftp_upload(
    sftp: &SftpSession,
    local: &Path,
    remote_path: &str,
    total: u64,
    mode: Option<u32>,
    ctl: &mut Ctl<'_>,
) -> Result<()> {
    let mut src = tokio::fs::File::open(local)
        .await
        .with_context(|| format!("open {}", local.display()))?;
    let mut dst = sftp
        .create(remote_path)
        .await
        .with_context(|| format!("create remote {}", remote_path))?;

    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    let mut pacer = Pacer::new(ctl.limit_bps);
    (ctl.on_progress)(0, total);
    loop {
        if ctl.cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("cancelled"));
        }
        let n = src.read(&mut buf).await.context("read local")?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await.context("write remote")?;
        done += n as u64;
        (ctl.on_progress)(done, total);
        // Held to the rate, if there is one — see [`Pacer`].
        if let Some(wait) = pacer.wait_after(n as u64) {
            tokio::time::sleep(wait).await;
        }
    }
    dst.shutdown().await.context("finish remote file")?;
    // Apply the requested permission bits (e.g. 0o777), and carry the local
    // file's date across — an upload is a copy, and a copy keeps its date (see
    // `cian_core::ops::copy_times`). Both in one round trip; both best-effort,
    // since a server may refuse either and the bytes are already there.
    let mtime = local_mtime_secs(local);
    if mode.is_some() || mtime.is_some() {
        let attrs = russh_sftp::protocol::FileAttributes {
            permissions: mode,
            // SFTP sets the two together: sending only mtime would zero the
            // access time on servers that read the pair.
            atime: mtime,
            mtime,
            ..Default::default()
        };
        let _ = sftp.set_metadata(remote_path, attrs).await;
    }
    let _ = sftp.close().await;
    Ok(())
}

/// A local file's mtime as SFTP wants it: seconds since the epoch.
fn local_mtime_secs(path: &Path) -> Option<u32> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    u32::try_from(secs).ok()
}

/// Give a downloaded file the date it has on the server.
fn set_local_mtime_secs(path: &Path, secs: u32) {
    let t = filetime::FileTime::from_unix_time(i64::from(secs), 0);
    let _ = filetime::set_file_mtime(path, t);
}

async fn sftp_download(
    sftp: &SftpSession,
    remote_path: &str,
    local: &Path,
    ctl: &mut Ctl<'_>,
) -> Result<()> {
    let meta = sftp.metadata(remote_path).await.ok();
    let total = meta.as_ref().and_then(|m| m.size).unwrap_or(0);
    // Read before the transfer: the same call already answers the size, and
    // the date has to land on the file after it is written and flushed.
    let remote_mtime = meta.as_ref().and_then(|m| m.mtime);
    let mut src = sftp
        .open(remote_path)
        .await
        .with_context(|| format!("open remote {}", remote_path))?;
    let mut dst = tokio::fs::File::create(local)
        .await
        .with_context(|| format!("create {}", local.display()))?;

    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    let mut pacer = Pacer::new(ctl.limit_bps);
    (ctl.on_progress)(0, total);
    loop {
        if ctl.cancel.load(Ordering::Relaxed) {
            // Don't leave a half file masquerading as the real download.
            let _ = tokio::fs::remove_file(local).await;
            return Err(anyhow!("cancelled"));
        }
        let n = src.read(&mut buf).await.context("read remote")?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await.context("write local")?;
        done += n as u64;
        (ctl.on_progress)(done, total);
        // Held to the rate, if there is one — see [`Pacer`].
        if let Some(wait) = pacer.wait_after(n as u64) {
            tokio::time::sleep(wait).await;
        }
    }
    dst.flush().await.context("finish local file")?;
    // After the handle is done with it, or the write would move the date back.
    drop(dst);
    if let Some(secs) = remote_mtime {
        set_local_mtime_secs(local, secs);
    }
    let _ = sftp.close().await;
    Ok(())
}

// ── classic SCP ───────────────────────────────────────────────────────────────
//
// The protocol (see OpenSSH's scp.c): one side runs `scp -t DIR` (sink, accepts
// what we push) or `scp -f FILE` (source, streams to us). Control messages and
// file bytes share the channel; each step is acknowledged with a status byte
// (0 = ok, 1 = warning, 2 = fatal), warnings/errors carrying a text line.

/// Read one SCP acknowledgement byte, turning a non-zero status into an error
/// that includes the server's message.
async fn read_ack<S: AsyncReadExt + Unpin>(stream: &mut S) -> Result<()> {
    let mut b = [0u8; 1];
    stream.read_exact(&mut b).await.context("read scp ack")?;
    match b[0] {
        0 => Ok(()),
        code => {
            let msg = read_line(stream).await.unwrap_or_default();
            Err(anyhow!("scp remote {}: {}", if code == 1 { "warning" } else { "error" }, msg.trim()))
        }
    }
}

/// Read bytes up to and including a `\n`, returning the line without it.
async fn read_line<S: AsyncReadExt + Unpin>(stream: &mut S) -> Result<String> {
    let mut out = Vec::new();
    let mut b = [0u8; 1];
    loop {
        stream.read_exact(&mut b).await.context("read scp line")?;
        if b[0] == b'\n' {
            break;
        }
        out.push(b[0]);
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Single-quote a path for the remote shell, since `scp -t/-f`'s argument is
/// expanded by it. Embedded single quotes are closed, escaped and reopened.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

async fn scp_upload(
    handle: &client::Handle<BlindClient>,
    local: &Path,
    remote_path: &str,
    total: u64,
    mode: Option<u32>,
    ctl: &mut Ctl<'_>,
) -> Result<()> {
    // `remote_path` is the full destination file path; scp -t wants a target
    // and the C-line carries the name, so split them.
    let (dir, name) = match remote_path.rsplit_once('/') {
        Some((d, n)) if !n.is_empty() => (if d.is_empty() { "/" } else { d }, n.to_string()),
        _ => (".", remote_path.to_string()),
    };
    let channel = handle.channel_open_session().await.context("open channel")?;
    channel
        .exec(true, format!("scp -t {}", shell_quote(dir)))
        .await
        .context("start remote scp -t")?;
    let mut stream = channel.into_stream();

    let mut src = tokio::fs::File::open(local)
        .await
        .with_context(|| format!("open {}", local.display()))?;
    scp_send(&mut stream, &name, total, mode, &mut src, ctl).await
}

/// Drive the SCP "sink" protocol on an established stream: announce the file,
/// stream `src`, and confirm. Generic over the transport so it can be tested
/// against an in-memory pipe.
async fn scp_send<S, R>(
    stream: &mut S,
    name: &str,
    total: u64,
    mode: Option<u32>,
    src: &mut R,
    ctl: &mut Ctl<'_>,
) -> Result<()>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
    R: AsyncReadExt + Unpin,
{
    read_ack(stream).await?; // remote ready
    // The C-line's mode governs the created file's permissions (default 0644).
    let header = format!("C{:04o} {} {}\n", mode.unwrap_or(0o644) & 0o7777, total, name);
    stream.write_all(header.as_bytes()).await.context("send scp header")?;
    stream.flush().await.ok();
    read_ack(stream).await?; // header accepted

    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    let mut pacer = Pacer::new(ctl.limit_bps);
    (ctl.on_progress)(0, total);
    loop {
        if ctl.cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("cancelled"));
        }
        let n = src.read(&mut buf).await.context("read local")?;
        if n == 0 {
            break;
        }
        stream.write_all(&buf[..n]).await.context("send file bytes")?;
        done += n as u64;
        (ctl.on_progress)(done, total);
        // Held to the rate, if there is one — see [`Pacer`].
        if let Some(wait) = pacer.wait_after(n as u64) {
            tokio::time::sleep(wait).await;
        }
    }
    stream.write_all(&[0u8]).await.context("finish file")?; // end-of-file ack
    stream.flush().await.ok();
    read_ack(stream).await?; // stored ok
    stream.shutdown().await.ok();
    Ok(())
}

async fn scp_download(
    handle: &client::Handle<BlindClient>,
    remote_path: &str,
    local: &Path,
    ctl: &mut Ctl<'_>,
) -> Result<()> {
    let channel = handle.channel_open_session().await.context("open channel")?;
    channel
        .exec(true, format!("scp -f {}", shell_quote(remote_path)))
        .await
        .context("start remote scp -f")?;
    let mut stream = channel.into_stream();

    let mut dst = tokio::fs::File::create(local)
        .await
        .with_context(|| format!("create {}", local.display()))?;
    match scp_recv(&mut stream, &mut dst, ctl).await {
        Ok(()) => Ok(()),
        Err(e) => {
            // Don't leave a half or empty file masquerading as the download.
            drop(dst);
            let _ = tokio::fs::remove_file(local).await;
            Err(e)
        }
    }
}

/// Drive the SCP "source" protocol on an established stream: request the file,
/// read its C-line and payload into `dst`. Generic for in-memory testing.
async fn scp_recv<S, W>(stream: &mut S, dst: &mut W, ctl: &mut Ctl<'_>) -> Result<()>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    stream.write_all(&[0u8]).await.context("scp start")?; // tell remote to proceed
    stream.flush().await.ok();

    // Skip any leading directory/time messages until the file's C-line arrives.
    let line = loop {
        let l = read_line(stream).await?;
        match l.as_bytes().first() {
            Some(b'C') => break l,
            Some(b'T') => {
                stream.write_all(&[0u8]).await.ok(); // ack mtime line, keep going
                stream.flush().await.ok();
            }
            Some(1) | Some(2) => return Err(anyhow!("scp remote: {}", l[1..].trim())),
            _ => return Err(anyhow!("unexpected scp reply: {:?}", l)),
        }
    };
    // C<mode> <size> <name>
    let mut parts = line[1..].splitn(3, ' ');
    let _mode = parts.next().unwrap_or("");
    let total: u64 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    stream.write_all(&[0u8]).await.context("ack C-line")?; // start sending
    stream.flush().await.ok();

    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    let mut pacer = Pacer::new(ctl.limit_bps);
    (ctl.on_progress)(0, total);
    while done < total {
        if ctl.cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("cancelled"));
        }
        let want = ((total - done) as usize).min(buf.len());
        let n = stream.read(&mut buf[..want]).await.context("read remote bytes")?;
        if n == 0 {
            return Err(anyhow!("scp: connection closed mid-file"));
        }
        dst.write_all(&buf[..n]).await.context("write local")?;
        done += n as u64;
        (ctl.on_progress)(done, total);
        // Held to the rate, if there is one — see [`Pacer`].
        if let Some(wait) = pacer.wait_after(n as u64) {
            tokio::time::sleep(wait).await;
        }
    }
    read_ack(stream).await?; // trailing status after the payload
    stream.write_all(&[0u8]).await.ok(); // final ack
    stream.flush().await.ok();
    dst.flush().await.context("finish local file")?;
    stream.shutdown().await.ok();
    Ok(())
}

// ── connection ────────────────────────────────────────────────────────────────

/// Run a future to completion on a private current-thread runtime, so the async
/// client can be driven from a plain (non-async) worker thread.
fn on_runtime<F, Fut, T>(f: F) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("start async runtime")?;
    rt.block_on(f())
}

/// Connect and authenticate with a password. The returned handle owns the SSH
/// connection every channel rides on, so it must outlive the transfer.
async fn connect(target: &Target) -> Result<client::Handle<BlindClient>> {
    let config = std::sync::Arc::new(client::Config::default());
    let mut handle = client::connect(config, (target.host.as_str(), target.port), BlindClient)
        .await
        .with_context(|| format!("connect {}:{}", target.host, target.port))?;

    // The key first when there is one, and the password after it — `ssh`'s own
    // order, and the one that behaves: a key that is configured was configured
    // on purpose. A key that is *refused* still falls through to the password
    // rather than ending the attempt, because the commonest reason a key fails
    // is that this particular server has never been given it.
    if let Some(path) = &target.key {
        let key = russh::keys::load_secret_key(path, target.key_pass.as_deref())
            .with_context(|| format!("read the key {}", path.display()))?;
        let with_alg = russh::keys::PrivateKeyWithHashAlg::new(
            std::sync::Arc::new(key),
            // SHA-256 where the key is RSA (ignored for every other kind).
            // russh's default is SHA-1, which a current sshd refuses outright.
            Some(russh::keys::HashAlg::Sha256),
        );
        if let AuthResult::Success = handle
            .authenticate_publickey(target.user.clone(), with_alg)
            .await
            .context("authenticate with the key")?
        {
            return Ok(handle);
        }
        if target.password.is_empty() {
            return Err(anyhow!(
                "the key {} was refused, and there is no password to try",
                path.display()
            ));
        }
    }
    match handle
        .authenticate_password(target.user.clone(), target.password.clone())
        .await
        .context("authenticate")?
    {
        AuthResult::Success => {}
        AuthResult::Failure { .. } => {
            return Err(anyhow!("authentication failed (wrong password?)"))
        }
    }
    Ok(handle)
}

/// Open an SFTP session on an authenticated connection. Fails (so the caller can
/// fall back to SCP) when the server has no SFTP subsystem.
async fn open_sftp(handle: &client::Handle<BlindClient>) -> Result<SftpSession> {
    let channel = handle.channel_open_session().await.context("open channel")?;
    channel.request_subsystem(true, "sftp").await.context("request sftp subsystem")?;
    let sftp = SftpSession::new(channel.into_stream()).await.context("start sftp")?;
    Ok(sftp)
}

#[cfg(test)]
mod tests {
    //! The SCP wire protocol runs against a fake "remote" on the other end of an
    //! in-memory duplex, so the framing (acks, C-line, payload) is exercised
    //! without a real SSH server — which cian can't stand up in CI anyway.
    use super::*;

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn shell_quote_wraps_and_escapes() {
        assert_eq!(shell_quote("/tmp/x"), "'/tmp/x'");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn scp_send_speaks_the_sink_protocol() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let payload = b"hello scp world".to_vec();
            let (mut ours, mut remote) = tokio::io::duplex(4096);

            // The fake `scp -t` running on the "server".
            let expect = payload.clone();
            let server = tokio::spawn(async move {
                remote.write_all(&[0u8]).await.unwrap(); // ready
                let header = read_line(&mut remote).await.unwrap();
                remote.write_all(&[0u8]).await.unwrap(); // header ok
                let size: usize = header[1..].split(' ').nth(1).unwrap().parse().unwrap();
                let mut body = vec![0u8; size];
                remote.read_exact(&mut body).await.unwrap();
                let mut z = [0u8; 1];
                remote.read_exact(&mut z).await.unwrap(); // end-of-file zero
                assert_eq!(z[0], 0);
                remote.write_all(&[0u8]).await.unwrap(); // stored ok
                (header, body, expect)
            });

            let cancel = no_cancel();
            let mut prog = |_a: u64, _b: u64| {};
            let mut ctl = Ctl { cancel: &cancel, on_progress: &mut prog, limit_bps: None };
            let mut src = std::io::Cursor::new(payload.clone());
            scp_send(&mut ours, "file.txt", payload.len() as u64, Some(0o777), &mut src, &mut ctl)
                .await
                .unwrap();

            let (header, body, expect) = server.await.unwrap();
            assert_eq!(header, format!("C0777 {} file.txt", expect.len()));
            assert_eq!(body, expect);
        });
    }

    #[test]
    fn scp_recv_reads_the_source_protocol() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let payload = b"downloaded bytes!!".to_vec();
            let (mut ours, mut remote) = tokio::io::duplex(4096);

            // The fake `scp -f` running on the "server".
            let sent = payload.clone();
            let server = tokio::spawn(async move {
                let mut z = [0u8; 1];
                remote.read_exact(&mut z).await.unwrap(); // client says go
                // A leading T (mtime) line must be tolerated and acked.
                remote.write_all(b"T1700000000 0 1700000000 0\n").await.unwrap();
                remote.read_exact(&mut z).await.unwrap(); // ack of T
                let header = format!("C0644 {} dl.bin\n", sent.len());
                remote.write_all(header.as_bytes()).await.unwrap();
                remote.read_exact(&mut z).await.unwrap(); // ack of C
                remote.write_all(&sent).await.unwrap();
                remote.write_all(&[0u8]).await.unwrap(); // trailing status
                remote.read_exact(&mut z).await.unwrap(); // final ack
            });

            let cancel = no_cancel();
            let mut prog = |_a: u64, _b: u64| {};
            let mut ctl = Ctl { cancel: &cancel, on_progress: &mut prog, limit_bps: None };
            let mut dst: Vec<u8> = Vec::new();
            scp_recv(&mut ours, &mut dst, &mut ctl).await.unwrap();

            server.await.unwrap();
            assert_eq!(dst, payload);
        });
    }

    #[test]
    fn scp_recv_surfaces_a_remote_error() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (mut ours, mut remote) = tokio::io::duplex(1024);
            let server = tokio::spawn(async move {
                let mut z = [0u8; 1];
                remote.read_exact(&mut z).await.unwrap();
                // status 1 + message line = warning/error the client must report.
                remote.write_all(&[1u8]).await.unwrap();
                remote.write_all(b"scp: /nope: No such file\n").await.unwrap();
            });
            let cancel = no_cancel();
            let mut prog = |_a: u64, _b: u64| {};
            let mut ctl = Ctl { cancel: &cancel, on_progress: &mut prog, limit_bps: None };
            let mut dst: Vec<u8> = Vec::new();
            let err = scp_recv(&mut ours, &mut dst, &mut ctl).await.unwrap_err();
            assert!(err.to_string().contains("No such file"), "got: {err}");
            server.await.unwrap();
        });
    }

    /// A folder is a plan: every file under it, and every directory that has
    /// to exist first, parents before children.
    #[test]
    fn an_upload_plan_covers_the_whole_tree() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("proj");
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir(root.join("docs")).unwrap();
        std::fs::write(root.join("README.md"), b"r").unwrap();
        std::fs::write(root.join("src/main.rs"), b"m").unwrap();
        std::fs::write(root.join("src/deep/x.rs"), b"x").unwrap();

        let plan = plan_upload(&root, "/srv/app").unwrap();
        assert_eq!(
            plan.dirs,
            vec!["/srv/app/proj", "/srv/app/proj/docs", "/srv/app/proj/src", "/srv/app/proj/src/deep"],
            "parents before children"
        );
        let remote: Vec<&str> = plan.files.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(
            remote,
            vec![
                "/srv/app/proj/README.md",
                "/srv/app/proj/src/main.rs",
                "/srv/app/proj/src/deep/x.rs",
            ]
        );
    }

    /// A plain file is one entry and makes no directories — the same call has
    /// to serve both, because the caller does not know which it has until it
    /// asks the disk.
    #[test]
    fn a_file_is_a_plan_of_one() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("note.txt");
        std::fs::write(&f, b"hi").unwrap();
        let plan = plan_upload(&f, "/srv").unwrap();
        assert!(plan.dirs.is_empty());
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].1, "/srv/note.txt");
    }

    /// **Symlinks are skipped, not followed.** One pointing outside the tree
    /// would send something nobody named; one pointing inside would send the
    /// same bytes twice, and on a loop would not finish at all.
    #[test]
    fn a_link_is_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("proj");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("real.txt"), b"r").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("real.txt"), root.join("link.txt")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        let plan = plan_upload(&root, "/srv").unwrap();
        let names: Vec<&str> = plan.files.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(names, vec!["/srv/proj/real.txt"], "{plan:?}");
        assert_eq!(plan.dirs, vec!["/srv/proj"], "the loop was not entered");
    }
}
