'use strict';
// The Electron side: one window, and the bridge from it to the engine.
//
// The renderer gets no Node at all — `contextIsolation` on, `nodeIntegration`
// off, and one narrow channel through the preload. A file manager runs whatever
// the disk hands it; the window that draws the listing has no business being
// able to read the disk itself.

const { app, BrowserWindow, Menu, ipcMain, nativeImage, nativeTheme, dialog } = require('electron');

/// The picture under the pointer when the file type has none of its own. A
/// 16×16 page, drawn here rather than shipped as a file: `startDrag` refuses
/// an empty icon and this is the whole of the fallback.
const DRAG_ICON = nativeImage.createFromDataURL(
    'data:image/svg+xml;base64,' + Buffer.from(
        '<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32">'
        + '<rect x="6" y="3" width="20" height="26" rx="3" fill="#f2f2f4" stroke="#9aa0a6"/>'
        + '<path d="M10 10h12M10 15h12M10 20h8" stroke="#9aa0a6" stroke-width="2"/></svg>',
    ).toString('base64'));
const path = require('node:path');
const fs = require('node:fs');
const os = require('node:os');
const { Engine } = require('./engine');

let engine = null;

/// Draw on the CPU.
///
/// A managed Windows desktop — a VM, an RDP session, a locked-down driver —
/// hands Chromium a GPU it cannot actually get a command buffer from:
///
///     ContextResult::kTransientFailure: Failed to send
///     GpuControl.CreateCommandBuffer
///
/// and the window arrives and then will not go away. That is what the first
/// Windows machine to run this did, 2026-08-31.
///
/// Turning acceleration off costs this program nothing worth measuring. It is
/// a listing, a status line and a text editor: DOM and text, no canvas, no
/// WebGL, nothing that a GPU was going to make faster. Weighed against a dead
/// window on the exact machines this is built for, it is not a close call.
///
/// CIAN_GPU=1 puts it back, for a machine where it demonstrably helps.
/// It has to be decided here — after `ready` Electron will not hear it.
if (process.env.CIAN_GPU !== '1') app.disableHardwareAcceleration();

/// The frame's colour before the page paints, matched to the saved look so
/// the window does not flash the wrong ground on the way up. It was a fixed
/// dark — right for a dark theme and exactly wrong for 白磁, the default.
/// The ground to open on before the page has painted, when the engine cannot
/// be asked in time.
///
/// **It used to be the whole answer, and it holds three of eighteen palettes.**
/// The other fifteen opened on hakuji's near-white and repainted a beat later
/// — a flash of the wrong colour on every start. The engine answers with the
/// real one now (`settings.ground`); this is only the fallback for a start
/// where it did not reply inside a second and a half.
const GROUNDS = { hakuji: '#f7f8f8', inei: '#14110f', terminal: '#0c0c0c' };

/// cian's own icon, wherever this is running from.
///
/// **The window had no icon at all.** Nothing was passed to `BrowserWindow`,
/// so on Windows the taskbar drew Electron's default atom — which is why
/// "the icon is not cian-ish" was true in a way nobody had said out loud:
/// the *window* was not wearing cian's icon, whatever `cian.ico` looked like.
///
/// Packaged it sits beside `main.js`; from a checkout it is at the root of
/// the repository. Same two-place search as `enginePath`, for the same
/// reason. An object rather than a value, so a missing file passes nothing
/// at all — Electron refuses an icon it cannot read.
function iconArg() {
    for (const at of besideOrAbove('cian.ico')) {
        if (fs.existsSync(at)) return { icon: at };
    }
    return {};
}

/// Packaged, beside `main.js`; from a checkout, at the root of the repository.
function besideOrAbove(name) {
    return [path.join(__dirname, name), path.join(__dirname, '..', name)];
}

/// The Dock, on a Mac.
///
/// **`BrowserWindow`'s `icon` does nothing on macOS** — the Dock takes its
/// picture from the `.app` bundle, and running from a checkout there is no
/// bundle, so the Dock showed Electron's own icon. `app.dock.setIcon` is the
/// way to say it anyway, and it wants a PNG: `nativeImage` reads `.ico` on
/// Windows only. `packaging/icon.py` writes `cian.png` from the same drawing
/// as the `.ico`, in the same run — two files, one picture, because two
/// drawings would drift the first time one of them was touched.
function nameTheDock() {
    if (process.platform !== 'darwin' || !app.dock) return;
    for (const at of besideOrAbove('cian.png')) {
        if (!fs.existsSync(at)) continue;
        const img = nativeImage.createFromPath(at);
        if (!img.isEmpty()) app.dock.setIcon(img);
        return;
    }
}

function createWindow(ground) {
    const win = new BrowserWindow({
        width: 1200,
        height: 800,
        backgroundColor: ground,
        // The name says what it stands for, which is where the name came
        // from: the README has opened with **C**omfortable **I**nterface for
        // **A**gile File e**X**plorer **N**avigation since the first commit,
        // and it had never been anywhere a person actually looks. Seven
        // invented lines were offered and all seven were wrong, for the good
        // reason that cian already had one.
        //
        // Lower-case `x` and `file`, on his instruction, and an em dash
        // rather than a hyphen — he asked for the wider one. The README
        // capitalises the acronym's letters to mark which they are; a title
        // bar is not an acronym key, and `Agile File eXplorer` in running
        // text reads as a brand doing a trick. The README keeps its bold —
        // that is where the marking belongs.
        //
        // `index.html`'s <title> is what Windows draws; this is the frame's
        // title before the page loads, so the two say the same thing rather
        // than the bar changing under you a beat after the window opens.
        title: 'cian — Comfortable Interface for Agile file explorer Navigation',
        ...iconArg(),
        webPreferences: {
            preload: path.join(__dirname, 'preload.js'),
            contextIsolation: true,
            nodeIntegration: false,
            sandbox: false,
        },
    });
    win.loadFile(path.join(__dirname, 'index.html'));

    // The window's own console, in the terminal that started it.
    //
    // A renderer throws into a devtools nobody has open, and the process that
    // launched it prints nothing at all — so a key that quietly does nothing
    // looks the same as a key that is not bound. In-house there may be no
    // devtools habit at all, and this is the only trail.
    win.webContents.on('console-message', (_e, level, message, line, source) => {
        const where = source ? `${source.split('/').pop()}:${line}` : 'renderer';
        const how = level >= 2 ? console.error : console.log;
        how(`[${where}] ${message}`);
    });
    return win;
}

/// Tell the OS which way round this window is.
///
/// `nativeTheme.themeSource` is what Windows reads to decide the caption bar,
/// and what macOS reads for the traffic-light background. It is a global for
/// the app rather than a window property, which is why this is a function and
/// not an option in `createWindow` — the palette can change while the window
/// is open, and the frame has to follow it.
function setFrameTheme(light) {
    try {
        nativeTheme.themeSource = light ? 'light' : 'dark';
    } catch {
        // An Electron old enough not to have it still draws a window; it just
        // draws the frame the OS would have anyway.
    }
}

/// The menu bar, decided rather than inherited.
///
/// **Electron installs a default menu when none is set, and on Windows that
/// menu owns Ctrl+A, Ctrl+C, Ctrl+X, Ctrl+V, Ctrl+Z, Ctrl+Y and Ctrl+R.**
/// Those are seven of cian's keys, and a menu accelerator takes the keystroke
/// before the page ever sees it — so mark-all, the file clipboard and redo
/// would have been dead on the only platform this build is for. It cannot be
/// seen from a Mac, where that same default menu is on Cmd instead.
///
/// So: no menu at all off macOS. cian is a full-screen keyboard program and
/// the bar is a row of pixels it has no use for.
///
/// macOS keeps one, and keeps Edit in it. Not for the look — without an Edit
/// menu, Cmd+C and Cmd+V stop working inside text fields on macOS, which is a
/// platform behaviour rather than a choice. Cmd there means "the text in this
/// field", which is what a Mac user means by it; cian's own bindings take
/// Ctrl as well, and Ctrl is what the hands this is built for use.
function installMenu() {
    if (process.platform !== 'darwin') {
        Menu.setApplicationMenu(null);
        return;
    }
    Menu.setApplicationMenu(Menu.buildFromTemplate([
        { role: 'appMenu' },
        { role: 'editMenu' },
        { role: 'windowMenu' },
    ]));
}

/// Who this is, as far as the Windows taskbar is concerned.
///
/// **Two buttons appear on the taskbar, and only one of them is the window.**
/// Reported as "cian-gui というアイコンの窓と、cian という Electron アイコンの
/// 窓が立ち上がる" — and true since the first build.
///
/// Two things were wrong. Electron takes its name from `package.json`, which
/// says `cian-gui-electron`, so that is what the app called itself. And with
/// no explicit AppUserModelID, Windows has no way to tie the window to the
/// app that opened it: it files the window under the *launcher's* identity
/// and the app under Electron's, which is two buttons for one program. The
/// ID has to be set before any window exists, so it is the first thing here.
///
/// `main.js` creates exactly one `BrowserWindow` (the `activate` handler only
/// fires when there are none), so a second Electron *window* was never
/// possible — whatever the taskbar was showing, it was showing it twice.
function nameSelf() {
    app.setName('cian');
    if (process.platform === 'win32') app.setAppUserModelId('jp.cian.cian');
}

app.whenReady().then(async () => {
    nameSelf();
    nameTheDock();
    installMenu();
    // The first plain argument is where to start; anything beginning with a
    // dash belongs to Chromium and may turn up anywhere in the line. Taking
    // argv[2] whatever it was meant that adding `--remote-debugging-port` gave
    // the engine a switch as its starting directory, and the window came up
    // empty with the reason only in a stream nobody was reading.
    const where = process.argv.slice(2).find((a) => !a.startsWith('-'));
    engine = new Engine(where || os.homedir());
    // Every call from the renderer, forwarded whole. The engine names its own
    // methods; this does not want a case per method that would need editing
    // each time one is added.
    ipcMain.handle('cian', async (_event, method, params) => {
        try {
            return { ok: await engine.call(method, params) };
        } catch (e) {
            return { error: String(e.message || e) };
        }
    });
    // F11 fills the screen, which is what F11 does on Windows. The window is
    // this process's to change; the key that asks for it is read in the page,
    // with every other key.
    // The icon the desktop itself uses for a file.
    //
    // A terminal can only draw a glyph from a font, so cian-tui picks from a
    // Nerd Font table and the window inherited it. This is a window: the OS
    // already has a picture for every registered type — the actual Excel icon
    // for an .xlsx, the app that claims a .psd — and it is one call away.
    //
    // The renderer asks per *extension*, not per file, so a folder of two
    // thousand files is a handful of calls.
    ipcMain.handle('cian-fileicon', async (_event, path) => {
        try {
            const img = await app.getFileIcon(path, { size: 'normal' });
            return img.isEmpty() ? null : img.toDataURL();
        } catch {
            return null;
        }
    });

    // Drag a file out of cian and into anything else.
    //
    // **A terminal program cannot be a drag source at all** — cian-tui says so
    // where it offers the clipboard instead, and the clipboard is the whole of
    // its answer. A window can hand the desktop the real file, so dropping
    // onto Finder, Explorer, a mail draft or another application works the way
    // it does from any other window.
    //
    // `startDrag` must be called while the drag is starting, which is why this
    // is `on` (fire and forget) rather than `handle` (await a reply): the
    // round trip of a reply is long enough for the gesture to be over.
    ipcMain.on('cian-drag', async (event, paths) => {
        if (!Array.isArray(paths) || !paths.length) return;
        let icon;
        try {
            icon = await app.getFileIcon(paths[0], { size: 'normal' });
        } catch { /* fall through to the drawn one */ }
        // Electron refuses an empty icon, and a file type with no registered
        // picture gives one — so there is always something to hold.
        if (!icon || icon.isEmpty()) icon = DRAG_ICON;
        try {
            event.sender.startDrag({ files: paths, file: paths[0], icon });
        } catch { /* the gesture ended before we got here */ }
    });

    // The palette changed while the window was open. `T` and `:theme` both end
    // here, because both end at a different `--bg` and the frame follows the
    // ground rather than the name of the thing that set it.
    ipcMain.handle('cian-frame', (_event, light) => {
        setFrameTheme(!!light);
        // What it *is*, not that it was asked for. The visible half of this is
        // a Windows caption bar, which cannot be photographed from here — so
        // the value read back is the only thing a check on this machine can
        // stand on, and it is worth returning for that alone.
        try {
            return nativeTheme.themeSource;
        } catch {
            return 'unknown';
        }
    });

    ipcMain.handle('cian-fullscreen', (event) => {
        const win = BrowserWindow.fromWebContents(event.sender);
        if (!win) return false;
        win.setFullScreen(!win.isFullScreen());
        return win.isFullScreen();
    });
    // One quick question before the frame exists: which ground was saved.
    // Bounded, because a window that waits on a wedged engine is worse than a
    // window that flashes.
    let ground = GROUNDS.hakuji;
    let light = true;
    try {
        const s = await Promise.race([
            engine.call('settings', {}),
            new Promise((_, no) => setTimeout(() => no(new Error('slow')), 1500)),
        ]);
        if (s && s.ground && s.ground.bg) {
            ground = s.ground.bg;
            light = !!s.ground.light;
        } else if (s && GROUNDS[s.look]) {
            ground = GROUNDS[s.look];
            light = s.look === 'hakuji';
        }
    } catch { /* the default ground */ }
    // **The title bar is the OS's, and it was always the light one.** Windows
    // draws the caption from what the app says its theme is, so a dark palette
    // sat under a white bar — the window disagreeing with itself along its own
    // top edge. Set before the frame exists, so it opens right rather than
    // correcting itself.
    setFrameTheme(light);
    const win = createWindow(ground);
    // The engine's unasked lines go straight to the window. Nothing here
    // interprets them; a progress count is the renderer's business.
    engine.onEvent = (msg) => {
        if (!win.isDestroyed()) win.webContents.send('cian-event', msg);
    };

    app.on('activate', () => {
        if (BrowserWindow.getAllWindows().length === 0) createWindow(ground);
    });
});

app.on('window-all-closed', () => {
    if (engine) engine.stop();
    // macOS keeps an app alive with no windows; everywhere else this is the end.
    if (process.platform !== 'darwin') app.quit();
});

app.on('before-quit', () => {
    if (engine) engine.stop();
});
