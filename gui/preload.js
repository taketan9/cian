'use strict';
// The only thing the renderer can reach. One function, and it can only ask the
// engine — no filesystem, no child processes, no `require`.

const { contextBridge, ipcRenderer, webUtils } = require('electron');

contextBridge.exposeInMainWorld('cian', {
    /// Call an engine method. Rejects with the engine's own message, which is
    /// written for a person and goes straight into a dialog.
    call: async (method, params) => {
        const reply = await ipcRenderer.invoke('cian', method, params);
        if (reply.error) throw new Error(reply.error);
        return reply.ok;
    },
    /// Where a dropped file actually is.
    ///
    /// A `File` from a drop no longer carries `.path` — Electron took that
    /// away, and rightly: a page that can read the path of anything dragged
    /// over it is a page that can read anything. `webUtils.getPathForFile` is
    /// the replacement, and it lives here rather than in the page because the
    /// page must not hold `webUtils` itself.
    pathOf: (file) => {
        try {
            return webUtils.getPathForFile(file);
        } catch {
            return null;
        }
    },
    /// Fill the screen, or stop filling it. Returns where it ended up.
    ///
    /// The window is the main process's to change, but the *key* has to be
    /// read where every other key is read. Doing it in the main process with
    /// `before-input-event` looked tidier and could not be tested: injected
    /// input does not go through that path, so the driver pressed F11 and the
    /// window did not move, with no way to tell a broken binding from a
    /// driver that cannot reach it.
    fullscreen: () => ipcRenderer.invoke('cian-fullscreen'),

    /// Say which way round the palette is, so the OS draws the frame to
    /// match. The title bar is not the page's to paint — Windows draws it
    /// from what the app declares its theme to be — so this is the one thing
    /// about the window's own colour the renderer has to hand outwards.
    frame: (light) => ipcRenderer.invoke('cian-frame', !!light),

    /// Hand these files to the desktop's drag. Fire and forget on purpose —
    /// the drag has to start while the gesture is still happening.
    startDrag: (paths) => ipcRenderer.send('cian-drag', paths),

    /// The desktop's own icon for a path, as a data URL (or null).
    fileIcon: (path) => ipcRenderer.invoke('cian-fileicon', path),

    /// Listen for what the engine says unasked. The callback is handed the
    /// message itself and nothing else — no event object, which would carry a
    /// sender the renderer has no business holding.
    onEvent: (fn) => {
        ipcRenderer.on('cian-event', (_e, msg) => fn(msg));
    },
});
