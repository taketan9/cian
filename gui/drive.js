// Press keys at the real window, and read back what happened.
//
// **The keys had never been tried.** Every check up to now went to the engine
// over the pipe, which is the half that was already right; the half that broke
// was the renderer, and it only breaks once a key is pressed. Two faults —
// a call to a function that did not exist, and a second `refresh()` quietly
// replacing the first — got through `node --check`, `cargo test` and the audit
// and landed on Taketan instead.
//
//     node gui/drive.js            # the standard round
//     node gui/drive.js , T ? Esc  # or whatever keys you want to see
//
// Electron is started with a debugging port and driven over CDP. No package:
// Node has had a WebSocket of its own since 22, and adding a dependency to a
// project whose whole point is that it builds offline would be a poor trade.

const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

/// A port of this run's own.
///
/// It was fixed, and a window left over from the run before answered on it —
/// so the keys went to a dead sandbox and the report described someone else's
/// files. The pid is enough to keep two runs apart, and `whose()` below checks
/// that the window it reached is in fact this one's.
const PORT = 9200 + (process.pid % 300);
const ROOT = path.join(__dirname, '..');

/// Keys as you would say them out loud, turned into what CDP wants.
const NAMED = {
    Esc: 'Escape', Enter: 'Enter', Tab: 'Tab', Space: ' ',
    Down: 'ArrowDown', Up: 'ArrowUp', Left: 'ArrowLeft', Right: 'ArrowRight',
    Bksp: 'Backspace', F5: 'F5',
    // 頁送りと行頭・行末。ヘルプが名前を挙げているのに、綴りが無いだけで
    // 押せなかった ── 押していないキーと、押せないキーは別のこと。
    PgUp: 'PageUp', PgDn: 'PageDown', Home: 'Home', End: 'End',
    Del: 'Delete',
};

function parseKey(spec) {
    // `Mod` is the platform's own: Cmd on macOS, Ctrl everywhere else. Monaco
    // binds Ctrl+S that way and is right to — but the driver was sending Ctrl
    // on a Mac, where it reaches nothing, and reporting the save key as dead.
    spec = spec.replace(/^Mod\+/, process.platform === 'darwin' ? 'Meta+' : 'Ctrl+');
    // `+` is both a key and the separator between modifiers, so splitting on
    // it left the spec `"+"` with no key at all and the page was handed an
    // empty string. A step pressing `+` therefore looked like a key the app
    // ignored, and the app had never been given anything to ignore. Stand the
    // final `+` aside before splitting.
    let plus = false;
    if (spec.endsWith('+') && spec.length > 1 ? spec[spec.length - 2] === '+' : spec === '+') {
        plus = true;
        spec = spec.slice(0, -1);
    }
    const parts = spec.split('+');
    if (plus) parts.push('+');
    const base = parts.pop();
    const mods = parts.map((m) => m.toLowerCase());
    let bits = 0;
    if (mods.includes('alt')) bits |= 1;
    if (mods.includes('ctrl')) bits |= 2;
    if (mods.includes('meta') || mods.includes('cmd')) bits |= 4;
    if (mods.includes('shift')) bits |= 8;
    const key = NAMED[base] || base;
    const v = virtual(key);
    if (v.needsShift) bits |= 8;
    delete v.needsShift;
    return {
        key,
        modifiers: bits,
        text: key.length === 1 && (bits & ~8) < 2 ? key : undefined,
        ...v,
    };
}

/// The key's number and its physical code.
///
/// **Without these, CDP sends keyCode 0.** A page reading `e.key` — everything
/// cian's own handlers do — never notices. Monaco does not read `e.key`: it
/// resolves its keybindings from the number, so Ctrl+S arrived as a keystroke
/// with no identity and the save silently never ran. The driver had reported
/// the key as dead, which was true and not the reason.
const VKEY = {
    Escape: 27, Enter: 13, Tab: 9, Backspace: 8, ' ': 32,
    ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40,
    Home: 36, End: 35, PageUp: 33, PageDown: 34, Delete: 46, Insert: 45,
};
const CODE = {
    Escape: 'Escape', Enter: 'Enter', Tab: 'Tab', Backspace: 'Backspace', ' ': 'Space',
    ArrowLeft: 'ArrowLeft', ArrowUp: 'ArrowUp', ArrowRight: 'ArrowRight', ArrowDown: 'ArrowDown',
    Home: 'Home', End: 'End', PageUp: 'PageUp', PageDown: 'PageDown',
    Delete: 'Delete', Insert: 'Insert',
};

function virtual(key) {
    if (VKEY[key]) return { code: CODE[key], windowsVirtualKeyCode: VKEY[key] };
    if (/^F\d{1,2}$/.test(key)) {
        return { code: key, windowsVirtualKeyCode: 111 + Number(key.slice(1)) };
    }
    if (key.length === 1) {
        const up = key.toUpperCase();
        if (up >= 'A' && up <= 'Z') {
            return { code: `Key${up}`, windowsVirtualKeyCode: up.charCodeAt(0) };
        }
        if (up >= '0' && up <= '9') {
            return { code: `Digit${up}`, windowsVirtualKeyCode: up.charCodeAt(0) };
        }
    }
    // Punctuation. cian's own handlers read `e.key` and would not care, but
    // Monaco resolves its bindings from the number — so a chord on `]` or `,`
    // was untestable until these were here.
    const PUNCT = {
        ';': ['Semicolon', 186], '=': ['Equal', 187], ',': ['Comma', 188],
        '-': ['Minus', 189], '.': ['Period', 190], '/': ['Slash', 191],
        '`': ['Backquote', 192], '[': ['BracketLeft', 219], '\\': ['Backslash', 220],
        ']': ['BracketRight', 221], "'": ['Quote', 222],
        // The shifted ones. Without a virtual key code the page is handed an
        // empty `key`, so a step pressing `+` looked like a key the app
        // ignored — and the app was never given anything to ignore.
        '+': ['Equal', 187, true], ':': ['Semicolon', 186, true], '?': ['Slash', 191, true],
        '<': ['Comma', 188, true], '>': ['Period', 190, true], '~': ['Backquote', 192, true],
        '_': ['Minus', 189, true], '"': ['Quote', 222, true], '|': ['Backslash', 220, true],
    };
    const p = PUNCT[key];
    // The third element says "this character *is* the shifted one", and the
    // shift has to be in the modifiers or Chromium works the key back out
    // from the physical position and hands the page `=` where `+` was meant —
    // or, with no code at all, an empty string.
    if (p) return { code: p[0], windowsVirtualKeyCode: p[1], needsShift: !!p[2] };
    return {};
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const argText = (a) => a.value ?? a.description ?? a.unserializableValue ?? JSON.stringify(a.preview ?? '');

/// Shift_JIS bytes. Node cannot encode it, and pulling in iconv for a test
/// fixture would be a dependency for the sake of three lines — but every
/// character used here is in JIS X 0208, so the table is one `Intl`-free map
/// built from what the platform *can* decode.
function sjis(text) {
    const out = [];
    for (const ch of text) {
        const code = ch.codePointAt(0);
        if (code < 0x80) { out.push(code); continue; }
        const found = SJIS_TABLE.get(ch);
        if (found === undefined) throw new Error(`no Shift_JIS byte pair for ${ch}`);
        out.push(found >> 8, found & 0xff);
    }
    return Buffer.from(out);
}

/// Built by asking the platform to decode every two-byte pair once. The
/// decoder is the authority on the mapping, so the table cannot disagree with
/// what the engine will read back.
const SJIS_TABLE = (() => {
    const dec = new TextDecoder('shift_jis', { fatal: false });
    const map = new Map();
    for (let hi = 0x81; hi <= 0xef; hi++) {
        for (let lo = 0x40; lo <= 0xfc; lo++) {
            if (lo === 0x7f) continue;
            const ch = dec.decode(Buffer.from([hi, lo]));
            if (ch.length === 1 && ch !== '\uFFFD' && !map.has(ch)) map.set(ch, (hi << 8) | lo);
        }
    }
    return map;
})();

async function target() {
    // Electron takes a moment to open the port; the page target appears after.
    for (let i = 0; i < 60; i++) {
        try {
            const res = await fetch(`http://127.0.0.1:${PORT}/json`);
            const page = (await res.json()).find((t) => t.type === 'page');
            if (page) return page.webSocketDebuggerUrl;
        } catch { /* not up yet */ }
        await sleep(250);
    }
    throw new Error('the window never appeared on the debugging port');
}

/// Wait until the window is showing this run's sandbox, and say so if it never
/// does. Attaching to the wrong window is silent otherwise: the keys land, the
/// status line answers, and every line of the report is about another
/// directory.
async function settle(cdp, sand) {
    for (let i = 0; i < 40; i++) {
        // Before renderer.js has been evaluated, `state` is not defined and
        // the read throws — which is "not ready yet", the exact thing this
        // loop exists to wait out. It killed the whole run instead, every
        // run, once mermaid's 3.4MB slowed the page past the first poll.
        let cwd = null;
        try {
            cwd = await cdp.read('state?.left?.cwd ?? null');
        } catch { /* the page is still loading */ }
        if (cwd && cwd.endsWith(path.join(path.basename(sand), 'from'))) return;
        await sleep(200);
    }
    throw new Error('the window never opened on this run\'s sandbox');
}

class Cdp {
    constructor(ws) { this.ws = ws; this.id = 0; this.waiting = new Map(); this.said = []; }

    static async open(url) {
        const ws = new WebSocket(url);
        await new Promise((ok, no) => { ws.onopen = ok; ws.onerror = no; });
        const cdp = new Cdp(ws);
        ws.onmessage = (e) => {
            const msg = JSON.parse(e.data);
            // Everything the page says, not only what it prints to stderr.
            // The first Monaco run opened nothing and reported nothing, and
            // the reason — a plain exception — was sitting in a console this
            // was not reading.
            if (msg.method === 'Runtime.consoleAPICalled' && msg.params.type !== 'log') {
                cdp.said.push(`${msg.params.type}: ${msg.params.args.map(argText).join(' ')}`);
            }
            if (msg.method === 'Runtime.exceptionThrown') {
                cdp.said.push(`例外: ${msg.params.exceptionDetails.exception?.description
                    || msg.params.exceptionDetails.text}`);
            }
            if (msg.method === 'Log.entryAdded' && msg.params.entry.level === 'error') {
                // The URL too: "failed to load resource" without saying which
                // resource is the least useful true sentence a browser says.
                const e = msg.params.entry;
                cdp.said.push(`${e.source}: ${e.text}${e.url ? '  ← ' + e.url : ''}`);
            }
            const w = cdp.waiting.get(msg.id);
            if (!w) return;
            cdp.waiting.delete(msg.id);
            msg.error ? w.no(new Error(msg.error.message)) : w.ok(msg.result);
        };
        return cdp;
    }

    send(method, params = {}) {
        const id = ++this.id;
        this.ws.send(JSON.stringify({ id, method, params }));
        return new Promise((ok, no) => this.waiting.set(id, { ok, no }));
    }

    async press(spec) {
        // `type:…` puts a string in, one character at a time, so a prompt can
        // be answered. Everything else is one key.
        // `wait:1200` gives something slow a moment. The editor's runtime
        // takes about a second to load the first time, and reading the window
        // 120 ms after F3 said the key had done nothing.
        if (spec.startsWith('wait:')) {
            await sleep(Number(spec.slice(5)));
            return;
        }
        if (spec.startsWith('type:')) {
            for (const ch of spec.slice(5)) await this.press(ch);
            return;
        }
        const k = parseKey(spec);
        for (const type of ['keyDown', 'keyUp']) {
            await this.send('Input.dispatchKeyEvent', {
                type: type === 'keyDown' && k.text ? 'keyDown' : type,
                key: k.key,
                code: k.code,
                windowsVirtualKeyCode: k.windowsVirtualKeyCode,
                nativeVirtualKeyCode: k.windowsVirtualKeyCode,
                text: type === 'keyDown' ? k.text : undefined,
                modifiers: k.modifiers,
            });
        }
        await sleep(120);
    }

    async read(expr) {
        const r = await this.send('Runtime.evaluate', {
            expression: expr, returnByValue: true, awaitPromise: true,
        });
        if (r.exceptionDetails) {
            // `.text` on its own is the word "Uncaught" and nothing else,
            // which is worse than no message at all — it names the category
            // and hides the fault.
            const d = r.exceptionDetails;
            throw new Error(d.exception?.description || d.exception?.value || d.text);
        }
        return r.result.value;
    }
}

/// What the window says about itself: the status line, and whichever sheet is
/// up. Enough to tell a key that worked from a key that did nothing.
const LOOK = `({
    status: document.querySelector('#status')?.textContent?.trim() ?? '',
    sheet: document.querySelector('#find:not([hidden])') ? 'sheet' : null,
    // Not just present — actually on top. The confirmation opened behind the
    // editor for a while, and merely being un-hidden said it was up all along.
    asking: (() => {
        const head = document.querySelector('#ask:not([hidden]) .head');
        if (!head) return null;
        const b = head.getBoundingClientRect();
        const at = document.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2);
        return head.contains(at) || at === head ? head.textContent : '(裏に隠れている)';
    })(),
    rows: document.querySelectorAll('#find:not([hidden]) .hit').length,
    at: [...document.querySelectorAll('#find:not([hidden]) .hit')].findIndex((e) => e.classList.contains('on')),
    focused: document.activeElement?.dataset?.answer ?? null,
    frame: (() => {
        const v = document.querySelector('#view:not([hidden])');
        return v ? getComputedStyle(v).boxShadow.replace(/px/g, '') : null;
    })(),
    prompt: (() => {
        const i = document.querySelector('.vfoot input');
        if (!i) return null;
        const b = i.getBoundingClientRect();
        const f = document.querySelector('.vfoot').getBoundingClientRect();
        // No template literal here: LOOK is one, and a backtick inside it
        // ends it. Twice now.
        return '左端から ' + Math.round(b.left - f.left) + 'px  幅 ' + Math.round(b.width) + 'px';
    })(),
    cursor: state?.[state.focus]?.cursor,
    cwd: state?.[state.focus]?.cwd,
    typed: document.querySelector('input:not([hidden])')?.value ?? null,
    shell: document.querySelector('#shell:not([hidden])')
        ? { about: document.getElementById('s-about').textContent,
            panes: [...document.querySelectorAll('#s-panes .sgrid')]
                .map((n) => n.style.left + '+' + n.style.width
                    + (n.classList.contains('on') ? '◀' : '')).join(' '),
            text: [...document.querySelectorAll('#s-panes .sgrid.on > div')]
                // Doubled on purpose: LOOK is a template literal, and inside
                // one a lone \\s is just an s. The regex reaching the page was
                // /s+$/ — it trimmed trailing letters and left the spaces.
                .map((d) => d.textContent.replace(/[ ]+$/, ''))
                .filter(Boolean).slice(-3).join(' ⏎ ') }
        : null,
    report: document.querySelector('#report:not([hidden])')
        ? { name: document.getElementById('r-name').textContent,
            about: document.getElementById('r-about').textContent,
            rows: document.querySelectorAll('#report .hit').length,
            first: document.querySelector('#report .hit')?.textContent }
        : null,
    view: document.querySelector('#view:not([hidden])')
        ? { pic: document.querySelector('#v-pic:not([hidden]) img, #v-pic:not([hidden]) embed')
                ? document.querySelector('#v-pic img, #v-pic embed').src.slice(0, 24) + '…' : null,
            about: document.getElementById('v-about').textContent,
            foot: document.getElementById('v-foot').textContent,
            first: document.querySelector('.view-line')?.textContent,
            lines: document.querySelectorAll('.view-line').length }
        : null,
    // The viewer's own idea of itself, not just its DOM. "F3 did nothing" has
    // two very different causes — never asked, or asked and stuck half-open —
    // and the sheet's hidden attribute cannot tell them apart.
    // No backtick in here: LOOK is one template literal, and one inside ends it.
    vstate: (() => { try { return (viewer.on ? 'on' : 'off') + (viewer.opening ? '+opening' : ''); } catch { return null; } })(),
    marks: state?.[state.focus]?.entries?.filter((x) => x.marked).map((x) => x.name) ?? [],
    scroll: document.querySelector('#find:not([hidden]) .hits')?.scrollTop ?? 0,
    focus: state?.focus,
    left: state?.left ? state.left.entries.length : null,
    marked: state?.[state.focus]?.marked,
})`;

async function main() {
    // Its own sandbox, always. The round presses keys that copy, move and
    // delete; pointed at a home directory it would do all three there. The
    // left pane opens on it, and `z` reaches `to`.
    const sand = fs.mkdtempSync(path.join(os.tmpdir(), 'cian-drive-'));
    // `from` holds files only, so `Space` always marks a file. It marked the
    // `to` directory once, the engine correctly refused to put it inside
    // itself, and the round read as a paste that had quietly done nothing.
    fs.mkdirSync(path.join(sand, 'from'));
    fs.mkdirSync(path.join(sand, 'to'));
    // A diagram, for the mermaid preview.
    fs.writeFileSync(path.join(sand, 'from', 'zu.md'),
        '# 図\n\n```mermaid\ngraph LR\n  A[開始] --> B{判定}\n  B -->|yes| C[実行]\n  B -->|no| D[終了]\n```\n\nおわり\n');
    // Brackets, for `%`.
    fs.writeFileSync(path.join(sand, 'from', 'k.rs'),
        'fn main() {\n    let x = (1 + 2);\n    println!("hi");\n}\n');
    // A binary, for the hex editor.
    fs.writeFileSync(path.join(sand, 'from', 'z.bin'), Buffer.from('HELLO WORLD\u0000\u0001', 'latin1'));
    // 差分送り（F7）のための一対。**離れた場所に三つだけ違う。**
    // まるごと違う二つを比べると差分は一塊になり、「次へ」が動いたのか
    // 動いていないのか区別がつかない。
    {
        const base = Array.from({ length: 12 }, (_, i) => `line ${i + 1}`);
        fs.writeFileSync(path.join(sand, 'from', 'd1.txt'), `${base.join('\n')}\n`);
        const other = [...base];
        other[1] = 'line 2 CHANGED';
        other[5] = 'line 6 CHANGED';
        other[9] = 'line 10 CHANGED';
        // `to/` 側は **`d2.txt`** ── 一周の前半が `from/` を丸ごと `to/` へ
        // 写すので、同じ名前だと上書きされて「2つは同一です」になる。
        fs.writeFileSync(path.join(sand, 'to', 'd2.txt'), `${other.join('\n')}\n`);
    }
    // A picture, for F3 on something the window draws rather than reads.
    fs.writeFileSync(path.join(sand, 'from', 'p.png'), Buffer.from(
        'iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAYAAADED76LAAAAHElEQVQoz2NgGAWjYBSMglEwCkbB'
        + 'KBgFo2AUjAIAB9wAAeEjBaEAAAAASUVORK5CYII=', 'base64'));
    for (const name of ['あ.txt', 'b.md', 'c.rs']) {
        // Long enough that G and gg have somewhere to go, and one of them in
        // Shift_JIS — the encoding the viewer exists to get right, and the one
        // a machine in Tokyo meets in every log it did not write.
        // The markdown one gets headings, so :outline has something to find.
        const body = name === 'b.md'
            ? ['# 見出し一', '本文', '## 小見出し A', '本文', '## 小見出し B', '本文',
               '# 見出し二', ...Array.from({ length: 33 }, (_, i) => `${i + 1} 行目`)].join('\n')
            : Array.from({ length: 40 }, (_, i) => `${i + 1} 行目 ${name} テスト`).join('\n');
        if (name === 'あ.txt') {
            fs.writeFileSync(path.join(sand, 'from', name), sjis(body + '\n'));
        } else {
            fs.writeFileSync(path.join(sand, 'from', name), body + '\n');
        }
    }

    const keys = process.argv.slice(2);
    const round = keys.length ? keys.map((k) => [k, '']) : [
        [',', 'ソート'], [',', 'ソートもう一度'],
        // The first row is 隠しファイル in both builds now (cian-tui's
        // `toggle_rows` order). It said 配色を送る and pressed whatever was
        // second, which turned input-sync on and left it on for the rest of
        // the round — a label that had drifted from what the key does.
        ['T', 'トグルを開く'], ['Enter', '隠しファイルを切り替える'], ['Esc', '閉じる'],
        // 配色を暗くしたら、**窓の縁も**暗くなること。タイトルバーは OS が
        // 描くので CSS では届かない ── 暗い配色に白い枠が乗っていた。
        ['read:\'frame の口: \' + (window.cian && typeof window.cian.frame === \'function\')', ''],
        // init.lua が届いているか ── 「設定を直したのに効かない」はこの
        // プロジェクトで一番よく出るバグなので、毎回1つは読み返す。
        ['read:\'init.lua の notify_min_secs: \' + (notifyAfterMs / 1000)', ''],
        ['read:(()=>{setLook(1, false);return \'陰翳に\';})()', ''], ['wait:500', ''],
        ['read:\'陰翳の ground=\' + getComputedStyle(document.documentElement).getPropertyValue(\'--bg\').trim()', ''],
        ['read:(async()=>\'OS に伝えた: \' + await tellFrame())()', ''],
        ['read:(()=>{setPalette(\'dracula\', false);return \'dracula に\';})()', ''], ['wait:500', ''],
        ['read:\'dracula の ground=\' + getComputedStyle(document.documentElement).getPropertyValue(\'--bg\').trim()', ''],
        ['read:(()=>{setLook(0, false);return \'白磁に戻す\';})()', ''], ['wait:400', ''],
        ['read:\'戻した ground=\' + getComputedStyle(document.documentElement).getPropertyValue(\'--bg\').trim()', ''],
        ['read:(async()=>\'OS に伝えた: \' + await tellFrame())()', ''],
        // With an input method holding the character. A terminal never sees
        // this event, which is why both builds shipped a helper to switch the
        // IME off; the window reads the physical key instead and leaves the
        // person's input method alone.
        ['ime:j', 'IME 中でも下へ'], ['ime:k', 'IME 中でも上へ'],
        // 一覧のキー表 ── 行が読めているかは絵でしか分からない。
        // インデントが崩れているのを一度そのまま出している（`?` の表）。
        ['?', 'ヘルプを開く'], ['wait:500', ''], ['shot:help-list@#find', '一覧のキー表'],
        ['Esc', '閉じる'],
        ['Space', 'マーク'], ['V', '反転'], ['Ctrl+a', '全マーク'],
        ['Tab', 'ペイン切替'], ['Ctrl+l', '右へ'], ['Ctrl+h', '左へ'],
        ['F5', '読み直し'], ['p', 'パスをコピー'],
        ['o', 'ペインを揃える'],
        ['Space', 'ひとつ持つ'], ['Ctrl+c', 'クリップボードへ'],
        ['Tab', '反対ペインへ'],
        ['z', 'パスで移動'], [`type:${sand}/to`, ''], ['Enter', 'to へ'],
        ['Ctrl+v', '貼り付け'],
        // Back to the left, marks cleared by the copy that just finished, and
        // onto `c.rs` by name rather than by counting rows.
        //
        // It used to be `Down` and hope. That worked only because the pane
        // was silently rebuilt on every refresh — which cleared the marks as
        // a side effect, so `Ctrl+c` had carried one file and the cursor
        // landed where the round assumed. With the rebuild gone (it was
        // taking the history and the sort with it) the marks survive, six
        // files are copied, and `Down` lands on a `.png` — so the whole
        // second half of the round, the editor and the save, stopped being
        // tested and said nothing about it.
        // **Enter を挟まずに F3 ── カーソルの同期が生きているか。**
        //
        // 窓版はカーソルを `ask()` の `cursors` に毎回載せている（renderer.js の
        // その行のコメントに、`j` を3回押したあとの `r` が3行上を改名した、と
        // 書いてある）。それが外れると、この一周のほとんどは Enter で入るので
        // 気づかない ── だから Enter を挟まない道を1本だけ通す。
        //
        // 測り方でここは2回間違えた。`viewer.name` は前に開いたファイルの
        // ものが残るし、`openFiles.list` はマーク中なら「マークした全部」に
        // なる（F3 の仕様）。**どちらも「違うものが開いた」と読める。**
        // だからマークを外してから、`viewer.on` と一緒に読む。
        ['Tab', '左へ'],
        // マークを外してから ── F3 はマーク中なら「マークした全部」を開くので、
        // 残ったマークを見ていると「カーソルと違うものが開いた」と読める。
        // 一度そう読んで、無いバグを追いかけた。
        ['read:(async()=>{const p=await ask(\'unmarkall\',{pane:state.focus});if(p){state[state.focus]=p;draw(state.focus);}return \'マークを外した\';})()', ''],
        ['read:\'マーク: \' + (state[state.focus].marked||0)', ''],
        ['land:d1.txt', 'd1.txt の行へ'],
        ['F3', 'Enter を挟まず開く'], ['wait:2500', ''],
        ['read:\'F3: on=\' + viewer.on + \' name=\' + (viewer.name||\'?\')', ''],
        ['Esc', ''], ['Esc', ''], ['Esc', ''], ['wait:600', ''],
        ['land:c.rs', 'c.rs の行へ'], ['Enter', 'ファイルを読む'],
        ['F3', 'エディタで開く'], ['wait:3000', ''],
        ['top:#view', 'エディタが最前面'],
        // `?` のキー一覧は、エディタの**上**に出なければ出ていないのと同じ。
        ['?', 'キー一覧'], ['wait:600', ''], ['top:#report', '一覧が最前面'],
        ['shot:help-editor@#report', 'エディタのキー表'],
        ['Esc', 'ファイルへ戻る'], ['wait:400', ''],
        // vim style is the default in both builds now, so the round asks for
        // insert mode before typing. It typed `XX` into normal mode instead —
        // two motions — and then "saved" a file it had not changed, which is a
        // write test that cannot fail.
        ['i', '挿入モードへ'], ['type:XX', '打つ'], ['Esc', 'ノーマルへ'],
        ['Mod+s', '保存'], ['wait:900', ''],
        ['Esc', ''], ['Esc', ''], ['Esc', '3回で閉じる'],

        // ── ここから下は「押したことがある」を増やすための一周 ───────────
        //
        // keycover が 70 種中 13 種と言っていた。押していないキーは
        // **動く証拠が無いキー**で、実際 Ctrl+E も vim 流の Ctrl+C/X/V も
        // この集合にいて実機で壊れているのが見つかった。1周が長くなるのは
        // 承知の上で ── 「プッシュ前の最後に実施するから構わない」。
        //
        // 順は面ごと。壊すものは押さない（削除は既存の一周が扱う）。
        // `Ctrl+Enter` はファイルの上では**外部アプリが起動する**ので、
        // ディレクトリの上でだけ押す。

        // 移動
        ['Shift+D', '10行下'], ['Shift+U', '10行上'],
        ['PgDn', '1頁下'], ['PgUp', '1頁上'], ['Shift+PgUp', 'シェルの巻き戻し'],
        ['y', 'やり直し（別名）'], ['wait:400', ''],
        ['Shift+S', 'SSH ピッカー'], ['wait:900', ''], ['Esc', ''], ['wait:400', ''],
        ['Ctrl+n', '次の行'], ['Ctrl+p', '前の行'],
        // **この塊は自分の出発点を自分で決める。**
        //
        // 最初はここが「砂場のどこか」から始まる前提で書いていて、上の半分が
        // ペインをどこへ置いたかで結果が変わった ── 単独で試すと通り、周回の
        // 中では通らない、といういちばん質の悪い形になる。砂場の根に降りて
        // から歩けば、履歴の中身まで決まる。
        // **前半が残した画面を閉じてから始める。**
        //
        // ここに足した塊は、上の一周が何も開いていない前提で書いてあった ──
        // 実際にはフィルタのプロンプトが開いたままで、`Backspace` は一覧では
        // なく入力欄へ行っていた。単独では通り、周回では通らない。周回に足す
        // ものは、自分の出発点を自分で作る。
        ['Esc', '前半の残りを閉じる'], ['wait:500', ''],
        ['Bksp', '親へ'], ['wait:900', ''],
        ['read:\'親 → \' + state[state.focus].cwd.split(/[\\\\/]/).pop()', ''],
        ['Alt+Left', '履歴を戻る'], ['wait:900', ''],
        ['read:\'Alt+← → \' + state[state.focus].cwd.split(/[\\\\/]/).pop()', ''],
        ['Alt+Right', '進む'], ['wait:900', ''],
        ['read:\'Alt+→ → \' + state[state.focus].cwd.split(/[\\\\/]/).pop()', ''],
                ['Shift+L', '右ペインへ'], ['Shift+H', '左ペインへ'],
        ['b', 'ブランチ表示'], ['wait:900', ''], ['b', '戻る'], ['wait:600', ''],

        // マークとクリップボード（貼り付けはしない ── 既存の一周が扱う）
        ['Ctrl+a', '全マーク'], ['Shift+Space', 'マークして上へ'],
        ['Shift+P', 'ファイルをクリップボードへ'], ['wait:400', ''],
        ['Ctrl+x', '切り取り'], ['wait:400', ''],
        ['Ctrl+a', '全マーク'], ['Ctrl+a', '解除'],

        // 窓の見た目
        ['Ctrl+=', '文字を大きく'], ['Ctrl+-', '小さく'], ['Ctrl+0', '戻す'],
        ['F12', 'ズーム'], ['wait:400', ''], ['F12', '戻す'], ['wait:400', ''],
        ['F11', '全画面'], ['wait:800', ''], ['F11', '戻す'], ['wait:800', ''],

        // 探す
        // `.rs` は砂場に必ずある。0件だとシートが開かず、その `top:` は
        // 「隠れている」ではなく「ない」と鳴って、意味が変わる。
        ['Shift+F', '名前で探す'], ['wait:600', ''],
        ['type:.rs'], ['Enter', ''], ['wait:4000', ''],
        ['read:\'名前で探す → \' + (report.rows||[]).length + \' 件\'', ''],
        ['top:#report', '結果が最前面'], ['Esc', '閉じる'], ['wait:400', ''],
        ['Ctrl+g', 'grep（別名）'], ['wait:500', ''], ['Esc', ''], ['wait:300', ''],
        // 表題は `:grep` ではなく「何をするか」。横幅も見る ── 入力欄が
        // 見切れていたのはここ。
        ['Ctrl+f', 'grep'], ['wait:500', ''],
        ['read:\'表題: \' + document.querySelector(\'#ask .head\').textContent', ''],
        ['read:\'欄の幅: \' + Math.round(document.querySelector(\'#ask .field\').getBoundingClientRect().width) + \'px\'', ''],
        ['shot:ask-grep@#ask .sheet', '入力シート'],
        ['type:行目'], ['Enter', ''], ['wait:1800', ''],
        ['top:#report', '結果が最前面'], ['Esc', '閉じる'], ['wait:400', ''],
        ['C', 'コマンドパレット'], ['wait:600', ''], ['Esc', ''], ['wait:300', ''],
        ['Ctrl+Shift+P', '同じものの別名'], ['wait:600', ''], ['Esc', ''], ['wait:300', ''],
        ['Ctrl+,', '同じものの三つ目'], ['wait:600', ''], ['Esc', ''], ['wait:300', ''],

        // タブ
        ['F9', '新規タブ'], ['wait:400', ''], ['Enter', '確認'], ['wait:700', ''],
        ['F2', '次のタブ'], ['wait:400', ''], ['F1', '前のタブ'], ['wait:400', ''],
        ['w', 'タブを閉じる'], ['wait:600', ''],

        // ディレクトリの上でだけ ── ファイルの上では外部アプリが起動する
        ['land:from', 'ディレクトリへ'], ['Ctrl+Enter', '反対ペインで開く'], ['wait:800', ''],

        // シェル
        ['Shift+J', 'シェルへ'], ['wait:1200', ''],
        ['Shift+F8', '左右に分割'], ['wait:800', ''],
        ['Shift+F9', '上下に分割'], ['wait:800', ''],
        ['Shift+F1', '前のペイン'], ['wait:400', ''],
        ['Shift+F2', '次のペイン'], ['wait:400', ''],
        ['Shift+F12', 'このペインだけ'], ['wait:500', ''],
        ['Shift+F12', '分割に戻す'], ['wait:500', ''],
        ['Shift+F10', '分割を閉じる'], ['wait:500', ''], ['Enter', '確認'], ['wait:700', ''],
        ['Ctrl+Shift+Enter', 'スニペット'], ['wait:700', ''], ['Esc', ''], ['wait:400', ''],
        ['Shift+Enter', 'シェルのメニュー'], ['wait:600', ''], ['Esc', ''], ['wait:400', ''],
        ['F9', 'シェルの新規タブ'], ['wait:800', ''],
        ['F1', 'タブ1へ'], ['wait:400', ''],
        ['F10', 'シェルタブを閉じる'], ['wait:500', ''], ['Enter', '確認'], ['wait:800', ''],
        ['Esc', 'ファイルへ'], ['Esc', ''], ['wait:500', ''],

        // エディタ
        // 中へ戻る ── 上で親へ上がったままなので、`b.md` はここには無い。
        // 「その行が無い」と「そのキーが効かない」は別のことで、`land:` は
        // 前者を「なし」と言って落ちるが、区別は書いた側の仕事。
        // 素の `l` は「入る」ではない ── ヘルプが `Enter/l` と書くのは
        // アーカイブの中の行だけで、普通の一覧では割り当てが無い。
        // 押して何も起きないのを「効かない」と読むところだった。
        ['land:from', 'from へ'], ['Enter', '入る'], ['wait:900', ''],
        // 見出しの多いほうを開く ── `zu.md` は見出しが一つで、`Mod+]` が
        // 「動かない」のか「行き先が無い」のか区別がつかない。
        ['land:b.md', 'md へ'], ['Enter', '開く'], ['wait:3000', ''],
        ['top:#view', 'エディタが最前面'],
        ['Ctrl+e', 'プレビュー'], ['wait:1200', ''],
        ['Ctrl+e', 'ソースへ'], ['wait:800', ''],
        // **横から書き換えられたファイルに保存しない。**
        //
        // 共有ディレクトリで2人が同じノートを開いて両方保存すると、後の人が
        // 前の人を黙って消していた。保存は断り、選ばせる。
        // **cian 自身の保存では再現しない** ── エンジンから見れば「自分が
        // 書いた」ので、当然ぶつからない。一度それで「書けてしまった」を
        // 出して、再現の作り方を間違えていた。外から書く必要がある。
        //
        // `date` にしてあるのは引用符を避けるため。中身は何でもよく、
        // 「長さと時刻が変わる」ことだけが要る。
        ['read:(async()=>{await window.cian.call("shellinput",{text:"date > "+viewer.path+"\\n"});return "シェルから書いた";})()', ''],
        ['wait:1800', ''],
        ['read:(async()=>{const r=await window.cian.call(\'save\',{lines:[\'こちらの編集\']});return \'保存 → \'+(r.conflict?\'断られた: \'+r.conflict:\'書けてしまった\');})()', ''],
        // 対ディスク差分ガター ── 1行足して、脇に印が出るか。
        // **「dirty」はファイル1つに1ビットしかなく、どこを直したかは
        // 覚えているしかなかった。**
        ['read:(()=>{const p=viewer.ed.getPosition();viewer.ed.executeEdits(\'t\',[{range:new monaco.Range(1,1,1,1),text:\'GUTTER TEST\\n\'}]);return \'1行入れた\';})()', ''],
        ['wait:1200', ''],
        ['read:\'ガターの印: \' + document.querySelectorAll(\'.gut-new, .gut-changed\').length + \' 本\'', ''],
        ['shot:gutter@#view', '差分ガター'],
        ['read:(()=>{viewer.ed.trigger(\'t\',\'undo\');return \'戻した\';})()', ''], ['wait:900', ''],
        // **`Mod+`、`Ctrl+` ではない。** これらは Monaco の `KeyMod.CtrlCmd` で
        // 束ねてあり、それは Windows では Ctrl、Mac では ⌘。`Ctrl+]` で
        // 押していたときは三つとも「何も起きない」と出て、危うく壊れて
        // いると報告するところだった ── 押し方が違うだけだった。
        ['Mod+]', '次の見出し'], ['wait:400', ''],
        ['read:\'見出し送り → 行 \' + viewer.ed.getPosition().lineNumber', ''],
        ['Mod+[', '前の見出し'], ['wait:400', ''],
        ['Mod+Shift+o', 'アウトライン'], ['wait:1200', ''],
        ['read:\'アウトライン \' + report.rows.length + \' 行\'', ''],
        ['top:#report', '最前面'], ['Esc', ''], ['wait:400', ''],
        ['Mod+Shift+b', 'blame'], ['wait:1200', ''],
        ['Shift+Enter', 'エディタのメニュー'], ['wait:600', ''], ['Esc', ''], ['wait:400', ''],
        ['u', 'エディタの外へ出す前に'], ['wait:200', ''],
        ['Esc', ''], ['Esc', ''], ['Esc', '閉じる'], ['wait:600', ''],

        // ── 残りの四つ。**どれも「状態を作らないと届かない」キー** ────────
        //
        // 押していないキーが4つ残っていた理由は同じで、grep の結果／差分／
        // アーカイブという、開いていないと存在しない場所のキーだから。
        // 押すためには先に開く。ここまでの塊と同じで、出発点は自分で作る。

        // grep の結果を渡り歩く（Ctrl+N / Ctrl+Shift+N）
        ['Esc', '画面を閉じる'], ['wait:500', ''],
        ['Ctrl+f', 'grep'], ['wait:600', ''], ['type:行目'], ['Enter', ''], ['wait:3000', ''],
        ['read:\'grep → \' + (report.rows||[]).length + \' 件\'', ''],
        ['Enter', '最初の一致へ'], ['wait:3000', ''],
        ['top:#view', 'ファイルが開いた'],
        ['read:\'一致 \' + (hits.at + 1) + \' / \' + hits.list.length', ''],
        ['Ctrl+n', '次の一致'], ['wait:1600', ''],
        ['read:\'Ctrl+N → \' + (hits.at + 1) + \' / \' + hits.list.length', ''],
        ['Ctrl+Shift+n', '前の一致'], ['wait:1600', ''],
        ['read:\'Ctrl+Shift+N → \' + (hits.at + 1) + \' / \' + hits.list.length', ''],
        ['Esc', ''], ['Esc', ''], ['Esc', '閉じる'], ['wait:700', ''],

        // 差分の中を歩く（F7 / Shift+F7）
        // 三箇所だけ違う一対を比べる ── まるごと違う二つでは差分が一塊に
        // なり、「次の差分」が動いたかどうかを言えない。
        ['land:d1.txt', '左'], ['Tab', '右へ'], ['wait:400', ''],
        ['land:d2.txt', '右'], ['Tab', '左へ'], ['wait:400', ''],
        ['=', '比較'], ['wait:3500', ''],
        ['read:\'比較: pair=\' + pair.on', ''],
        // 差分送りはカーソルを動かすだけで、下端の一行は変わらない ──
        // 「動かなかったキー」に数えられても、それは動かなかった証拠では
        // ない。動いたかどうかは差分エディタに訊く。
        ['read:\'差分 \' + pair.ed.getLineChanges().length + \' 箇所\'', ''],
        ['F7', '次の差分'], ['wait:700', ''],
        ['read:\'F7 → \' + (pair.ed.getModifiedEditor().getPosition()||{}).lineNumber + \' 行目\'', ''],
        ['F7', 'さらに次'], ['wait:700', ''],
        ['read:\'F7 → \' + (pair.ed.getModifiedEditor().getPosition()||{}).lineNumber + \' 行目\'', ''],
        ['Shift+F7', '前の差分'], ['wait:700', ''],
        ['read:\'Shift+F7 → \' + (pair.ed.getModifiedEditor().getPosition()||{}).lineNumber + \' 行目\'', ''],
        ['Esc', '比較を閉じる'], ['wait:800', ''],

        // アーカイブの中で `l`（素の `l` が「入る」になる唯一の場所）
        ['Bksp', '砂場の根へ'], ['wait:800', ''],
        ['land:from', 'ディレクトリを選ぶ'],
        ['read:(async()=>{const r=await ask(\'compress\',{pane:state.focus,kind:\'zip\',name:\'bag\'});return \'zip: \' + (r ? \'できた\' : \'できず\');})()', ''],
        ['wait:1200', ''], ['F5', '読み直し'], ['wait:800', ''],
        ['land:bag.zip', 'zip へ'], ['Enter', '中へ'], ['wait:1500', ''],
        ['read:\'アーカイブ: \' + (state[state.focus].archive || \'入れていない\').split(/[\\\\/]/).pop()', ''],
        // 中のディレクトリの行に乗ってから ── アーカイブに入った直後の
        // カーソルは `..` の上で、そこで `l` を押すと「入る」ではなく
        // 「出る」になる。行を選ばずに押して、結果を数えても意味がない。
        ['read:\'中身: \' + (state[state.focus].entries||[]).map(e=>e.name).join(\' \')', ''],
        ['land:from', '中のディレクトリへ'],
        ['l', '中へ'], ['wait:900', ''],
        ['read:\'l のあと: \' + (state[state.focus].entries||[]).length + \' 行\'', ''],
        // zip の中へコピー ── **横に落ちていないこと**を確かめる。
        //
        // アーカイブの表示は作り物で、ペインは入る前のディレクトリを `cwd` に
        // 覚えたままなので、素のコピーは zip の *隣* にファイルを落として
        // 「コピーしました」と言っていた。合っているのは言葉だけだった。
        ['Bksp', 'zip の根へ'], ['wait:800', ''],
        ['Tab', '反対ペインへ'],
        ['land:k.rs', 'k.rs の行へ'],
        ['c', 'zip へ追加'], ['wait:700', ''], ['Enter', 'はい'], ['wait:2000', ''],
        ['read:\'zip の中: \' + (state[state.focus === \'left\' ? \'right\' : \'left\'].entries||[]).map(e=>e.name).join(\' \')', ''],
        ['read:\'コピー元は変わらず: \' + (state[state.focus].entries||[]).length + \' 行\'', ''],
        ['Tab', 'zip 側へ戻る'],
        ['Bksp', 'ひとつ戻る'], ['wait:700', ''],
        ['Bksp', 'アーカイブを出る'], ['wait:800', ''],

        // コピーの取り消し ── 押せた証拠ではなく、**消えた証拠**を取る。
        //
        // **行き先は新しく作る。** 一周の前半が `to/` に何本か置いている
        // ので、同じ名前がすでにあると `copy_creates` は「このコピーが
        // 作ったものは無い」と正しく答え、取り消しは何もしない ── 通った
        // のに何も確かめていない一周になる。空の `to/undo` を作って、
        // そこへ一本だけ写す。
        ['Tab', '右へ'],
        ['z', 'パスで移動'], [`type:${sand}/to`, ''], ['Enter', 'to へ'], ['wait:800', ''],
        ['A', 'ディレクトリを作る'], ['type:undo'], ['Enter', ''], ['wait:900', ''],
        ['land:undo', 'そこへ'], ['Enter', '入る'], ['wait:900', ''],
        ['Tab', '左へ'], ['land:from', 'from へ'], ['Enter', '入る'], ['wait:900', ''],
        ['land:k.rs', 'k.rs の行へ'], ['Space', 'マーク'],
        ['c', '反対ペインへコピー'], ['wait:700', ''], ['Enter', 'はい'], ['wait:2000', ''],
        ['read:\'コピー後の行き先 → \' + (state.right.entries||[]).filter(e=>!e.parent).length + \' 行\'', ''],
        ['Mod+z', 'コピーを取り消す'], ['wait:2000', ''],
        ['read:\'Ctrl+z のあと → \' + (state.right.entries||[]).filter(e=>!e.parent).length + \' 行\'', ''],
        // コピーはやり直せない（元の場所を覚えていない）。断られるのが
        // 正しい ── ここで確かめているのはキーが届くことの方。
        ['Mod+Shift+z', 'やり直し'], ['wait:900', ''],

        // 取り消し・やり直し（一周の最後に、砂場を元へ）
        ['u', '取り消し'], ['wait:600', ''],
        ['Ctrl+r', 'やり直し'], ['wait:600', ''],
    ];

    // Its own config directory, inside the sandbox.
    //
    // Without this the driver ran against the real ~/.config/cian, and every
    // `:editstyle vim` or `:view icons` it typed while testing was written
    // into somebody's actual settings — quietly, and the next run then
    // started from whatever the last test had left. A test that changes what
    // it is testing is not a test. An explicit CIAN_CONFIG_DIR still wins, so
    // a keymap.lua can be handed in on purpose.
    const conf = process.env.CIAN_CONFIG_DIR || path.join(sand, 'config');
    fs.mkdirSync(conf, { recursive: true });

    // A real init.lua, because "the setting did not take" is this project's
    // most repeated bug and the only way to catch it is to write the file and
    // read it back out of the running window. Seeding the value from the
    // console would prove the reader and nothing about the config.
    //
    // **Something inert.** Whatever is written here is in force for the whole
    // round, so it has to be a setting the rest of the script does not care
    // about: `notify_min_secs` is a number the window keeps and never acts on
    // during a run of this length.
    if (!process.env.CIAN_CONFIG_DIR) {
        fs.writeFileSync(path.join(conf, 'init.lua'),
            'cian.set_option("notify_min_secs", 7)\n', 'utf8');
    }

    const el = spawn(process.env.CIAN_ELECTRON
        || path.join(__dirname, 'node_modules/electron/dist/Electron.app/Contents/MacOS/Electron'),
        [__dirname, path.join(sand, 'from'), `--remote-debugging-port=${PORT}`],
        { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'],
          env: { ...process.env, CIAN_CONFIG_DIR: conf } });

    const crashes = [];
    for (const s of [el.stdout, el.stderr]) {
        s.on('data', (b) => {
            const t = String(b);
            if (/Uncaught|ReferenceError|TypeError|is not a function/.test(t)) crashes.push(t.trim());
        });
    }

    let bad = 0;
    let said = [];
    try {
        const cdp = await Cdp.open(await target());
        said = cdp.said;
        await cdp.send('Runtime.enable');
        await cdp.send('Log.enable');
        await settle(cdp, sand);

        // What a state looks like in one line. Written once: the `click:` and
        // `ime:` steps printed only the status, so a menu that opened was
        // invisible in the report — which is the fault that once made a
        // working F3 read as dead.
        const marks = (after) => {
            const asking = after.prompt ? `  ｜: ${after.prompt}  枠 ${after.frame}`
                : (after.asking ? `  ⟨${after.asking}⟩ 焦点=${after.focused}` : null);
            const menu = after.sheet && after.rows
                ? `  ▣ ${after.rows}項目（${after.at + 1}番目）`
                : null;
            const sh = after.shell
                ? `  ▸ ${after.shell.about}  [${after.shell.panes}]  «${after.shell.text}»`
                : null;
            const rep = after.report
                ? `  ▤ ${after.report.name} ｜${after.report.about}｜ ${after.report.rows}行  «${after.report.first}»`
                : null;
            const vst = after.vstate && after.vstate !== 'off' ? `  ◈${after.vstate}` : '';
            const view = after.view
                ? (after.view.pic
                    ? `  ▦ ${after.view.about}  ${after.view.pic}`
                    : `  ｜${after.view.foot}  ${after.view.about}  «${after.view.first}»`)
                : null;
            return vst + (asking ?? rep ?? menu ?? view ?? sh
                ?? (after.marks.length ? `  [${after.marks.join(' ')}]` : ''));
        };

        for (const [key, what] of round) {
            // `list` reads instead of pressing: every row of whatever sheet is
            // open, with its right-hand column. The menus were compared with
            // cian-tui's by reading source on both sides, which is how six
            // labels drifted without anything noticing — a menu nobody ever
            // reads back is a menu that says whatever it last said.
            // `shot:name` writes a PNG next to the sandbox. Reading rows back
            // says what a menu *says*; only a picture says whether it fits.
            if (key.startsWith('shot:')) {
                // `shot:name@<css>` — その要素だけを3倍で撮る。
                //
                // **CSS の後勝ちは全景では見えない。** 2pxの枠が消えている、
                // 線が二本ある、列がずれている ── どれも1200px幅に縮めた絵の
                // 中では1画素の話になる。毎回 sips でオフセットを当てて
                // いたが、当て損なうと真っ黒な絵を見て「問題ない」と言う
                // ことになるので、位置は要素に訊く。
                const [name, sel] = key.slice(5).split('@');
                let clip = null;
                if (sel) {
                    const box = await cdp.read(`(() => {
                        const n = document.querySelector(${JSON.stringify(sel)});
                        if (!n || n.hidden) return null;
                        const b = n.getBoundingClientRect();
                        if (!b.width || !b.height) return null;
                        return { x: b.left, y: b.top, width: b.width, height: b.height };
                    })()`);
                    if (!box) {
                        console.log(`  shot    ${sel} が見つかりません`);
                        bad++;
                        continue;
                    }
                    clip = { ...box, scale: 3 };
                }
                const png = await cdp.send('Page.captureScreenshot',
                    clip ? { format: 'png', clip, captureBeyondViewport: true } : { format: 'png' });
                // Not in the sandbox: that is deleted when the run ends, and
                // a picture you cannot open afterwards is not evidence.
                const dir = path.join(os.tmpdir(), 'cian-shots');
                fs.mkdirSync(dir, { recursive: true });
                const at = path.join(dir, `${name}.png`);
                fs.writeFileSync(at, Buffer.from(png.data, 'base64'));
                console.log(`  shot    ${what || name}${sel ? ` (${sel} ×3)` : ''}  → ${at}`);
                continue;
            }
            // `click:<css>` — press the mouse on the first match. The whole
            // mouse surface was untestable: every check up to now sent keys,
            // and the differences that kept surviving (the ◀ ▶ arrows, the
            // breadcrumb segments, the dividers) are all things you can only
            // reach with a pointer.
            if (key.startsWith('click:')) {
                const sel = key.slice(6);
                const box = await cdp.read(`(() => {
                    const n = document.querySelector(${JSON.stringify(sel)});
                    if (!n) return null;
                    const b = n.getBoundingClientRect();
                    return { x: Math.round(b.left + b.width / 2), y: Math.round(b.top + b.height / 2) };
                })()`);
                if (!box) {
                    console.log(`× ${key.padEnd(8)}${(what || '').padEnd(16)} 見つかりません`);
                    bad++;
                    continue;
                }
                const before = await cdp.read(LOOK);
                for (const type of ['mousePressed', 'mouseReleased']) {
                    await cdp.send('Input.dispatchMouseEvent', {
                        type, x: box.x, y: box.y, button: 'left', clickCount: 1,
                    });
                }
                await sleep(250);
                const after = await cdp.read(LOOK);
                const moved = JSON.stringify(before) !== JSON.stringify(after);
                console.log(`${moved ? '  ' : '× '}${key.padEnd(8)}${(what || '').padEnd(16)} ${after.status}${marks(after)}`);
                if (!moved) bad++;
                continue;
            }
            // `ime:j` — the keydown a browser reports while an input method
            // holds the character: `Process`, virtual key 229, and the
            // physical key still named. It is the only way to test the IME
            // road without a Japanese IME on this machine, and the road exists
            // precisely because a terminal never sees this event at all.
            if (key.startsWith('ime:')) {
                const ch = key.slice(4);
                const before = await cdp.read(LOOK);
                for (const type of ['rawKeyDown', 'keyUp']) {
                    await cdp.send('Input.dispatchKeyEvent', {
                        type,
                        key: 'Process',
                        code: `Key${ch.toUpperCase()}`,
                        windowsVirtualKeyCode: 229,
                        nativeVirtualKeyCode: 229,
                        modifiers: ch === ch.toUpperCase() && /[A-Z]/.test(ch) ? 8 : 0,
                    });
                }
                await sleep(200);
                const after = await cdp.read(LOOK);
                const moved = JSON.stringify(before) !== JSON.stringify(after);
                console.log(`${moved ? '  ' : '× '}${key.padEnd(8)}${(what || '').padEnd(16)} ${after.status}${marks(after)}`);
                if (!moved) bad++;
                continue;
            }
            // `drag:<css>:dx,dy` — press on the first match, move by that
            // many pixels, release. The dividers and the file rows are the
            // two things in this window that only a held pointer can work,
            // and neither could be reached from here.
            if (key.startsWith('drag:')) {
                const cut = key.lastIndexOf(':');
                const sel = key.slice(5, cut);
                const [dx, dy] = key.slice(cut + 1).split(',').map(Number);
                const box = await cdp.read(`(() => {
                    const n = document.querySelector(${JSON.stringify(sel)});
                    if (!n) return null;
                    const b = n.getBoundingClientRect();
                    return { x: Math.round(b.left + b.width / 2), y: Math.round(b.top + b.height / 2) };
                })()`);
                if (!box) {
                    console.log(`× ${key.padEnd(8)}${(what || '').padEnd(16)} 見つかりません`);
                    bad++;
                    continue;
                }
                const before = await cdp.read(LOOK);
                const at = { x: box.x, y: box.y };
                await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...at, button: 'left', clickCount: 1 });
                // In steps: a drag handler that follows the pointer needs
                // moves to follow, and one jump is not a drag.
                for (let i = 1; i <= 5; i++) {
                    await cdp.send('Input.dispatchMouseEvent', {
                        type: 'mouseMoved',
                        x: box.x + Math.round((dx * i) / 5),
                        y: box.y + Math.round((dy * i) / 5),
                        button: 'left', buttons: 1,
                    });
                    await sleep(40);
                }
                await cdp.send('Input.dispatchMouseEvent', {
                    type: 'mouseReleased', x: box.x + dx, y: box.y + dy, button: 'left', clickCount: 1,
                });
                await sleep(300);
                const after = await cdp.read(LOOK);
                const moved = JSON.stringify(before) !== JSON.stringify(after);
                console.log(`${moved ? '  ' : '× '}${key.padEnd(8)}${(what || '').padEnd(16)} ${after.status}${marks(after)}`);
                if (!moved) bad++;
                continue;
            }
            // `read:<expr>` — evaluate something in the page and print it.
            // Added the third time a state was diagnosed by reasoning about
            // which listener ran first, which is a way of being wrong slowly.
            if (key.startsWith('read:')) {
                let out;
                try {
                    out = await cdp.read(key.slice(5));
                } catch (err) {
                    out = `例外: ${err.message}`;
                }
                console.log(`  read    ${what || key.slice(5)} = ${JSON.stringify(out)}`);
                continue;
            }
            // `top:<css>` — その面が**本当にいちばん上に見えているか**。
            //
            // 二度やった: 状態（`report.on === true`、題も正しい）だけ見て
            // 「出た」と判断し、実際には `#view` の**裏**に開いていた。
            // 同じ z-index の兄弟は文書順で重なるので、後に書いたほうが勝つ
            // ── index.html のコメントがその事故を書いている場所で、また
            // 起きた。**状態ではなく画素を見る**。
            if (key.startsWith('top:')) {
                const sel = key.slice(4);
                const out = await cdp.read(
                    `(() => { const n = document.querySelector(${JSON.stringify(sel)});
                       if (!n || n.hidden) return 'ない';
                       const r = n.getBoundingClientRect();
                       const at = document.elementFromPoint(r.left + r.width / 2, r.top + 40);
                       if (!at) return '取れない';
                       return at.closest(${JSON.stringify(sel)}) ? 'ok'
                           : ('隠れている ← ' + (at.closest('[id]') || at).id); })()`,
                );
                console.log(`  top     ${sel} → ${out}`);
                if (out !== 'ok') bad++;
                continue;
            }
            // `land:<name>` — put the cursor on a named row. A step that
            // depends on *which* file is under the cursor should say which
            // file, not a number of Downs.
            if (key.startsWith('land:')) {
                const want = key.slice(5);
                const at = await cdp.read(
                    `(() => { const p = state[state.focus];
                       const i = p.entries.findIndex((e) => e.name === ${JSON.stringify(want)});
                       if (i < 0) return 'なし';
                       p.cursor = i; draw(state.focus); return String(i); })()`,
                );
                console.log(`  land    ${want} → 行 ${at}`);
                if (at === 'なし') bad++;
                continue;
            }
            if (key === 'list') {
                const rows = await cdp.read(`[...document.querySelectorAll('#find:not([hidden]) .hit, #report:not([hidden]) .hit')]
                    .map((e) => e.textContent.replace(/\\s+/g, ' ').trim())`);
                console.log(`  list    ${what}`);
                for (const r of rows) console.log(`            ${r}`);
                continue;
            }
            const before = await cdp.read(LOOK);
            await cdp.press(key);
            const after = await cdp.read(LOOK);
            // **A wait is not a key.** It was counted with them, so the
            // number at the bottom grew by one for every `wait:` in the round
            // — and when the round doubled, so did the noise. The count is
            // meant to say "this many keys did nothing"; a pause doing
            // nothing is what a pause is for.
            const isWait = key.startsWith('wait:');
            const moved = isWait || JSON.stringify(before) !== JSON.stringify(after);
            const note = what ? `  ${what}` : '';
            // The viewer before the shell. The shell panel is open from
            // startup now, so a report that prefers it can never show the
            // viewer — which read as "F3 did nothing" for a whole afternoon
            // while F3 was working fine.
            console.log(`${moved ? '  ' : '× '}${key.padEnd(8)}${note.padEnd(16)} ${after.status}${marks(after)}`);
            if (!moved) bad++;
        }
        // Let the last job finish before looking. A copy started by the
        // final key is still running when the loop ends.
        await sleep(600);
        console.log(`\n最後の状態: ${(await cdp.read(LOOK)).status}`);
        console.log('砂場:');
        for (const extra of ['from/展開先']) {
            const at = path.join(sand, ...extra.split('/'));
            if (fs.existsSync(at)) {
                console.log(`  ${extra}/  ${fs.readdirSync(at).sort().join('  ') || '(空)'}`);
            }
        }
        // Bytes, not just names: the editor's whole promise is that a file
        // goes back the way it came, and a name tells you nothing about that.
        const edited = path.join(sand, 'from', 'あ.txt');
        const raw = fs.readFileSync(edited);
        const head = [...raw.subarray(0, 12)].map((b) => b.toString(16).padStart(2, '0')).join(' ');
        console.log(`  あ.txt  ${raw.length} バイト  先頭: ${head}`);
        for (const dir of ['from', 'to']) {
            const at = path.join(sand, dir);
            const names = fs.readdirSync(at).sort().join('  ');
            console.log(`  ${dir}/  ${names || '(空)'}`);
        }
    } finally {
        el.kill();
        fs.rmSync(sand, { recursive: true, force: true });
    }

    if (crashes.length) {
        console.log('\n落ちた:');
        crashes.forEach((c) => console.log('  ' + c));
    }
    if (said.length) {
        console.log('\nページが言ったこと:');
        said.forEach((c) => console.log('  ' + c));
    }
    // A key that changed nothing is not always wrong — pressing `,` twice only
    // reverses — so this reports rather than fails. The crashes are the failure.
    console.log(`\n動かなかったキー ${bad} 件、例外 ${crashes.length} 件`);
    process.exit(crashes.length ? 1 : 0);
}

main().catch((e) => { console.error(e.message); process.exit(1); });
