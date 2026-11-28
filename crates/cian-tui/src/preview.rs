//! Cursor-follow preview (`:preview`): while it is on and a file pane has
//! focus, the shell panel's area shows what the cursor is on — text with
//! syntax colour, images (real pixels where the terminal can), directory and
//! archive listings, Office/PDF text. Focusing the shell (Shift+J / click)
//! shows the real shell again; the PTY runs underneath the whole time, only
//! the pixels are borrowed.
//!
//! Loading is synchronous but bounded (the viewer's own 4MB cap, listing
//! caps below) and cached by path, so holding `j` costs one load per file,
//! not one per frame.

use std::path::{Path, PathBuf};

use crate::{tr, App, FocusedPane};

/// How many rows a directory / archive listing shows at most. A preview is a
/// glance, not a browser — entering it is one keypress away.
const LIST_CAP: usize = 500;

/// What the preview pane is showing for [`PreviewState::path`].
pub(crate) enum PreviewBody {
    /// Text lines with per-char highlight categories (empty when plain).
    Text { lines: Vec<String>, hl: Vec<Vec<cian_core::highlight::Category>> },
    /// A directory or archive listing, pre-rendered as rows.
    List { rows: Vec<String>, truncated: bool },
    /// An image; pixels are rendered at draw time (protocol or half-blocks).
    Image,
    /// A one-line explanation instead of content.
    Note(String),
}

/// How long the cursor rests on a heavy file before it is read.
///
/// Short enough not to be felt when landing on one deliberately, long enough
/// that holding a key down a folder never reads any of them.
const PREVIEW_SETTLE: std::time::Duration = std::time::Duration::from_millis(140);

/// Is reading this worth waiting for the cursor to settle?
///
/// By what it costs, not what it is: a picture is decoded whole before a
/// thumbnail of it exists, and anything of a few megabytes is read the same
/// way. A small text file is neither.
fn heavy(path: &Path) -> bool {
    const BIG: u64 = 1 << 20;
    let big = std::fs::metadata(path).map(|m| m.len() >= BIG).unwrap_or(false);
    big || cian_core::image::is_image(path)
}

pub(crate) struct PreviewState {
    pub path: PathBuf,
    pub body: PreviewBody,
    /// Half-block fallback thumbnail, cached by the box it was rendered for.
    pub thumb: Option<(u16, u16, cian_core::image::Thumb)>,
    /// The decoded picture, kept so a resize scales rather than decodes.
    pub decoded: Option<image::DynamicImage>,
}

impl App {
    /// The picture for `path` if the decoder thread has finished with it —
    /// and, if nothing is decoding it yet, start.
    ///
    /// Returns `None` while it is still going, which is a frame that draws
    /// "reading…" rather than a frame that waits.
    pub(crate) fn take_decoded(&mut self, path: &Path) -> Option<image::DynamicImage> {
        // A job for some other file is stale the moment the cursor moves on.
        if self.preview_decode.as_ref().is_some_and(|(p, _)| p != path) {
            self.preview_decode = None;
        }
        if let Some((_, rx)) = self.preview_decode.as_ref() {
            match rx.try_recv() {
                Ok(img) => {
                    self.preview_decode = None;
                    return img;
                }
                // Still working; the sender is alive because the thread is.
                Err(std::sync::mpsc::TryRecvError::Empty) => return None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.preview_decode = None;
                    return None;
                }
            }
        }
        // Already have it, in one form or the other.
        if self.preview_gfx.as_ref().is_some_and(|(p, _)| p == path)
            || self.preview.as_ref().is_some_and(|s| s.path == *path && s.decoded.is_some())
        {
            return None;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let owned = path.to_path_buf();
        std::thread::Builder::new()
            .name("cian-decode".into())
            .spawn(move || {
                let _ = tx.send(image::open(&owned).ok());
            })
            .ok()?;
        self.preview_decode = Some((path.to_path_buf(), rx));
        None
    }

    /// Does a picture drawn by the current protocol survive the cells under
    /// it being redrawn?
    ///
    /// Kitty places graphics over the grid and keeps them until told
    /// otherwise; iTerm2 writes them as cell content, and half-blocks *are*
    /// cells. Only the first needs the screen wiped when a picture goes away.
    pub(crate) fn needs_clear_after_image(&self) -> bool {
        use ratatui_image::picker::ProtocolType as P;
        self.gfx_picker
            .as_ref()
            .is_some_and(|p| matches!(p.protocol_type(), P::Kitty | P::Sixel))
    }

    /// `:preview` — flip the cursor-follow preview.
    pub(crate) fn toggle_preview(&mut self) {
        self.preview_on = !self.preview_on;
        if !self.preview_on {
            if self.preview_gfx.is_some() && self.needs_clear_after_image() {
                self.full_clear = true;
            }
            self.preview = None;
            self.preview_gfx = None;
        }
        self.message = Some(if self.preview_on {
            tr(
                self.lang,
                "preview on. the shell panel follows the cursor (Shift+J for the shell)",
                "プレビューON。シェル枠がカーソルに追従（シェルは Shift+J）",
            )
            .into()
        } else {
            tr(self.lang, "preview off", "プレビューOFF").into()
        });
    }

    /// Make sure the cached preview matches `path`, loading it if not.
    /// Called from the render pass, so it must stay cheap on the cached path.
    pub(crate) fn ensure_preview(&mut self, path: &Path) {
        if self.preview.as_ref().map(|p| p.path == *path).unwrap_or(false) {
            self.preview_wanted = None;
            return;
        }
        // Reading it happens here, in the middle of drawing a frame — so a
        // file that takes a moment to decode takes that moment out of the
        // interface. Holding an arrow key down a folder of photographs meant
        // decoding every one of them on the way past.
        //
        // Only the heavy ones wait: an ordinary file is read and shown at
        // once, as it always was, and something big enough to be felt is read
        // when the cursor has stopped on it.
        if heavy(path) {
            let now = std::time::Instant::now();
            match &self.preview_wanted {
                Some((p, since)) if p == path => {
                    if now.duration_since(*since) < PREVIEW_SETTLE {
                        return;
                    }
                }
                _ => {
                    self.preview_wanted = Some((path.to_path_buf(), now));
                    return;
                }
            }
        }
        self.preview_wanted = None;
        // Moving off an image leaves its pixels stuck on screen unless the
        // terminal is wiped: the graphics layer is not part of the cell buffer
        // ratatui diffs against. This is why the file *after* a picture looked
        // like it had no preview at all.
        //
        // Only when the protocol in use actually leaves something behind.
        // Half-blocks are ordinary cells, and iTerm2's inline images are cell
        // content too — both are cleared by redrawing those cells, which
        // ratatui's diff already does. Kitty's are placed *over* the grid and
        // stay until they are deleted, so those need the wipe.
        //
        // Wiping regardless cost a full repaint of the whole window on every
        // step through a folder of images: the black flash, and most of the
        // wait before the next picture appeared.
        if self.needs_clear_after_image()
            && matches!(self.preview.as_ref().map(|p| &p.body), Some(PreviewBody::Image))
        {
            self.full_clear = true;
        }
        // A different image invalidates the protocol state too.
        if self.preview_gfx.as_ref().map(|(p, _)| p != path).unwrap_or(false) {
            self.preview_gfx = None;
        }
        let body = load_preview(path, self.lang);
        self.preview =
            Some(PreviewState { path: path.to_path_buf(), body, thumb: None, decoded: None });
    }
}

/// Classify and load, with the same dispatch order as F3 (`look_inside`).
fn load_preview(path: &Path, lang: crate::Lang) -> PreviewBody {
    if path.is_dir() {
        return match std::fs::read_dir(path) {
            Ok(rd) => {
                let mut dirs: Vec<String> = Vec::new();
                let mut files: Vec<String> = Vec::new();
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    if is_dir {
                        dirs.push(format!("{}/", name));
                    } else {
                        files.push(name);
                    }
                    if dirs.len() + files.len() > LIST_CAP {
                        break;
                    }
                }
                dirs.sort();
                files.sort();
                let truncated = dirs.len() + files.len() > LIST_CAP;
                dirs.extend(files);
                dirs.truncate(LIST_CAP);
                PreviewBody::List { rows: dirs, truncated }
            }
            Err(e) => PreviewBody::Note(e.to_string()),
        };
    }
    if cian_core::archive::is_archive(path) {
        return match cian_core::archive::list(path) {
            Ok(members) => {
                let truncated = members.len() > LIST_CAP;
                let rows = members
                    .into_iter()
                    .take(LIST_CAP)
                    .map(|m| {
                        if m.is_dir {
                            m.name
                        } else {
                            format!("{}  ({})", m.name, cian_core::human_size(m.size))
                        }
                    })
                    .collect();
                PreviewBody::List { rows, truncated }
            }
            Err(e) => PreviewBody::Note(e.to_string()),
        };
    }
    if cian_core::image::is_image(path) {
        return PreviewBody::Image;
    }
    if cian_core::office::classify(path).is_some() {
        return match cian_core::office::extract(path) {
            Ok((_, lines)) => PreviewBody::Text { lines, hl: Vec::new() },
            Err(e) => PreviewBody::Note(e.to_string()),
        };
    }
    match cian_core::viewer::view_file(path) {
        Ok(mut view) => {
            // Draw-safe first, highlight second, so the per-character colours
            // stay parallel to the characters actually drawn.
            for l in &mut view.lines {
                *l = crate::util::plain(l);
            }
            let hl = if view.kind == cian_core::viewer::ViewKind::Text {
                cian_core::highlight::detect(path)
                    .map(|lang| cian_core::highlight::highlight(&view.lines, lang))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            PreviewBody::Text { lines: view.lines, hl }
        }
        Err(_) => PreviewBody::Note(
            tr(lang, "no preview for this file", "このファイルはプレビューできません").into(),
        ),
    }
}

/// The path the preview should follow: the cursor entry of the focused file
/// pane — unless there is a reason not to look (remote pane: reading a file
/// would download it; nothing selected). `Err` carries the note to show.
/// Kinds the preview never opens by itself, whatever the config says.
///
/// All of these are containers or images that would have to be unpacked or
/// scanned to show anything, and what is inside is not what anyone is looking
/// at the folder for. A `.vsix` is an editor extension, a `.whl` is a Python
/// package, an `.iso` is a disc — the panel stalls, and the listing that comes
/// back answers a question nobody asked. `F3` opens any of them.
///
/// Deliberately *not* here: `.zip`, `.tar`, `.7z` and friends. Those are
/// archives someone is browsing on purpose, and listing one is the point.
pub(crate) const PREVIEW_SKIP_DEFAULT: &[&str] = &[
    // Zip-based packages that are not meant to be browsed as archives.
    "vsix", "jar", "war", "ear", "aar", "whl", "egg", "apk", "aab", "ipa", "nupkg", "crx", "xpi",
    "appx", "msix", "docm", "xlsm", // macro-bearing Office: opened, not listed
    // Disc and disk images.
    "iso", "dmg", "vmdk", "vdi", "vhd", "vhdx", "wim", "img", "qcow2",
    // Installers and packages.
    "msi", "pkg", "deb", "rpm", "cab",
    // A PDF's text is extracted by scanning the whole file, and what comes
    // back is not the page anyone is looking at. `F3` still opens it.
    "pdf",
    // Databases and their logs: large, and meaningless as text.
    "mdf", "ldf", "ndf", "sqlite", "db3", "pdb", "ost", "pst",
];

/// Is this one of the extensions `preview_skip` names?
///
/// Compared lowercased and without the dot, so the config can say `vsix`,
/// `.vsix` or `VSIX` and mean the same thing. The config *adds* to
/// [`PREVIEW_SKIP_DEFAULT`] rather than replacing it.
pub(crate) fn skip_preview(app: &App, path: &std::path::Path) -> bool {
    let Some(ext) = path.extension().map(|e| e.to_string_lossy().to_lowercase()) else {
        return false;
    };
    if PREVIEW_SKIP_DEFAULT.contains(&ext.as_str()) {
        return true;
    }
    // Normalised here rather than only where it was read, so it does not
    // matter how the value arrived: `vsix`, `.vsix` and `VSIX` are the same
    // answer to the same question.
    app.config
        .options
        .preview_skip
        .iter()
        .any(|s| s.trim().trim_start_matches('.').eq_ignore_ascii_case(&ext))
}

pub(crate) fn preview_target(app: &App) -> Result<PathBuf, String> {
    let pane = match app.focused {
        FocusedPane::Left => app.left.active_ref(),
        FocusedPane::Right => app.right.active_ref(),
        FocusedPane::Shell => return Err(String::new()), // caller never asks
    };
    if pane.archive_view().is_some() {
        return Err(tr(
            app.lang,
            "inside an archive. F3 views a member",
            "アーカイブ内。メンバーは F3 で閲覧",
        )
        .into());
    }
    if pane.is_remote() {
        return Err(tr(
            app.lang,
            "remote pane. no preview (it would download every file)",
            "リモートペイン。プレビューなし（毎回ダウンロードになるため）",
        )
        .into());
    }
    match pane.selected() {
        Some(e) if e.is_parent => Ok(e.path.clone()), // `..` previews the parent dir
        // A placeholder would be downloaded just by the cursor resting on it —
        // the one case where following the cursor is actively expensive.
        Some(e) if e.cloud && !cian_core::cloud::include() => Err(tr(
            app.lang,
            "☁ This file has not been downloaded yet.\n\n             Showing it here would fetch it over the network, and holding a\n             cursor down a folder would fetch every file in it.\n\n             F3 opens this one (and downloads it).\n             T → \"Read ☁ cloud-only files\" previews them from now on",
            "☁ このファイルはまだダウンロードされていません。\n\n             ここに表示するとネットワーク越しに取得することになり、\n             カーソルを押しっぱなしにするとディレクトリ内が全部落ちてきます。\n\n             F3 でこの1つを開けます（ダウンロードされます）。\n             T →「☁ クラウド上のファイルも読む」で以後プレビューします",
        )
        .into()),
        // Kinds the config says not to open unasked. A `.vsix` is a zip of an
        // editor extension: listing one means unpacking it, which stalls the
        // panel for a file nobody wanted to look inside.
        Some(e) if skip_preview(app, &e.path) => Err(tr(
            app.lang,
            "preview off for this kind (preview_skip). F3 opens it",
            "この拡張子はプレビュー対象外です（preview_skip）— F3 で開けます",
        )
        .into()),
        Some(e) => Ok(e.path.clone()),
        None => Err(tr(app.lang, "empty folder", "空のディレクトリ").into()),
    }
}
