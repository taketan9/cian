'use strict';
// The `cian-server` child, and the promise-per-call wrapper over its pipe.
//
// One JSON object per line each way. Replies carry the `id` of the call they
// answer, so several may be in flight and the order they come back in does not
// matter — which it will not, once a directory read is slower than a keypress.

const { spawn } = require('node:child_process');
const readline = require('node:readline');
const path = require('node:path');
const fs = require('node:fs');

/// Where the engine binary is, in each of the two ways this runs.
///
/// Packaged, it sits beside the app. From a checkout it is under `target/`,
/// and there may be two — **the newer one wins, not the release one.**
///
/// Preferring release looked like the careful choice and was the opposite: a
/// morning's release build sat there while an afternoon of `cargo build` went
/// into debug, and the front end kept talking to the old engine. It answered
/// "no such method" to something written an hour earlier, which is a confusing
/// way to learn this.
function enginePath() {
    const exe = process.platform === 'win32' ? 'cian-server.exe' : 'cian-server';
    const beside = path.join(__dirname, exe);
    if (fs.existsSync(beside)) return beside;
    const built = ['release', 'debug']
        .map((profile) => path.join(__dirname, '..', 'target', profile, exe))
        .filter((p) => fs.existsSync(p))
        .sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
    if (built.length) return built[0];
    throw new Error(`cian-server not found — cargo build -p cian-server`);
}

class Engine {
    constructor(cwd) {
        this.next = 1;
        this.pending = new Map();
        /// Set by whoever wants the unasked lines.
        this.onEvent = null;
        // `cwd` が null なら引数を渡さない ── エンジンが init.lua の `home`
        // を見て決める。空文字を渡すと「空のパス」として扱われる。
        this.child = spawn(enginePath(), cwd ? [cwd] : [], {
            stdio: ['pipe', 'pipe', 'pipe'],
            windowsHide: true,
        });
        readline.createInterface({ input: this.child.stdout }).on('line', (line) => {
            let msg;
            try {
                msg = JSON.parse(line);
            } catch {
                // Not our protocol. Anything the engine prints that is not a
                // reply belongs in the log, not thrown away.
                console.error('engine said:', line);
                return;
            }
            // A line with no id is the engine speaking unasked — progress,
            // or an operation finishing. It belongs to nobody's promise.
            if (msg.event) {
                if (this.onEvent) this.onEvent(msg);
                return;
            }
            const waiting = this.pending.get(msg.id);
            if (!waiting) return;
            this.pending.delete(msg.id);
            if (msg.error) waiting.reject(new Error(msg.error));
            else waiting.resolve(msg.ok);
        });
        // The engine's stderr is the engine's own trouble; keep it whole.
        this.child.stderr.on('data', (b) => console.error('engine:', String(b).trimEnd()));
        // A dead engine must not leave callers waiting for ever.
        this.child.on('exit', (code) => {
            this.gone = true;
            const dead = new Error(`the engine stopped (exit ${code})`);
            for (const { reject } of this.pending.values()) reject(dead);
            this.pending.clear();
        });
        // **Closing the window used to end with a dialog.** The last thing a
        // window does on its way out is remember things — the look, the font,
        // the split — and each of those is a `call`. `stop()` has already
        // killed the child by then, so the write lands on a pipe with nobody
        // at the other end: `write EOF`, raised as an `error` event on a
        // stream nobody was listening to, which Node turns into an uncaught
        // exception and Electron into `Uncaught Exception: Error: write EOF`.
        //
        // Two halves, because either alone still leaves the other case: the
        // handler here catches a pipe that dies mid-write, and `gone` below
        // stops us starting a write we already know has nowhere to go. A
        // shutting-down engine is not an error to show anybody — the answers
        // were only ever going to be discarded.
        const quiet = () => { this.gone = true; };
        this.child.stdin.on('error', quiet);
        this.child.on('error', quiet);
    }

    call(method, params = {}) {
        const id = this.next++;
        const line = JSON.stringify({ id, method, params });
        return new Promise((resolve, reject) => {
            if (this.gone || !this.child.stdin.writable) {
                reject(new Error('the engine is not running'));
                return;
            }
            this.pending.set(id, { resolve, reject });
            // The callback form, so a pipe that closes between the check above
            // and the write itself is answered here rather than thrown.
            this.child.stdin.write(line + '\n', (e) => {
                if (!e) return;
                this.pending.delete(id);
                reject(e);
            });
        });
    }

    stop() {
        this.gone = true;
        this.child.kill();
    }
}

module.exports = { Engine };
