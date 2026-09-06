'use strict';
// The listing, drawn. No engine logic here — this asks and paints, and every
// answer it paints came from cian-core.

/// Which language the interface speaks.
///
/// Japanese unless asked otherwise, which is the opposite way round from
/// cian-tui — that build is written in English and translates *to* Japanese.
/// The signature is deliberately the same, `tr(en, ja)`, so a line can be
/// moved between the two builds without being turned inside out, and so the
/// Japanese stays in the source where `scripts/parity.py` can see it.
///
/// From `cian.set_option("lang", "en")` in init.lua, and switchable at
/// runtime from the menu and the switches — as it is there, and like there it
/// is not remembered: the language belongs to init.lua, and a window that
/// remembered a different one would quietly disagree with the terminal build
/// on the same machine.
let lang = 'ja';

function tr(en, ja) {
    return lang === 'en' ? en : ja;
}

/// Switch, and repaint everything made of words.
///
/// Nothing is remembered. cian-tui's `ToggleId::Lang` is runtime-only too, and
/// for a reason worth keeping: the language belongs to `init.lua`, which both
/// builds read, and a window that quietly remembered a different one would
/// disagree with the terminal build on the same machine.
function setLang(next) {
    if (next === lang) return;
    lang = next;
    paintMarkup();
    // Everything a person is looking at right now. The listing carries the
    // column headings, the status row carries the chips, the hint bar is
    // nothing but words — and a menu that is open is redrawn where it stands.
    draw('left');
    draw('right');
    drawStatus();
    drawHints();
    if (menu.spec) drawMenu();
    say(tr('Language: English', '言語: 日本語'));
}

/// The handful of words written into index.html rather than drawn from here.
///
/// The markup keeps its Japanese — it is what `scripts/requests.py` checks for
/// the confirmation buttons (Taketan asked for「キャンセル」by name), and it is
/// what the window shows before this file has run. This puts them into
/// whichever language is on, and is called again when it changes.
function paintMarkup() {
    const set = (sel, en, ja) => {
        const n = document.querySelector(sel);
        if (n) n.textContent = tr(en, ja);
    };
    set('#p-foot', 'Esc = stop   b = to the background', tr('Esc = stop   b = to the background', 'Esc = 中止   b = バックグラウンドへ'));
    set('#ask [data-answer="no"]', 'Cancel  (Esc)', tr('Cancel  (Esc)', 'キャンセル  (Esc)'));
    const q = document.getElementById('find-q');
    if (q) q.placeholder = tr('/ to narrow', '/ で絞り込み');
}

/// The two panes as the engine last described them.
const state = { left: null, right: null, focus: 'left' };

const el = {
    hints: document.getElementById('hints'),
    work: document.getElementById('work'),
    gripPanes: document.getElementById('grip-panes'),
    gripMain: document.getElementById('grip-main'),
    panes: document.getElementById('panes'),
    left: document.querySelector('[data-pane="left"]'),
    right: document.querySelector('[data-pane="right"]'),
    status: document.getElementById('status'),
    stBadge: document.getElementById('st-badge'),
    stChips: document.getElementById('st-chips'),
    stMsg: document.getElementById('st-msg'),
    stShell: document.getElementById('st-shell'),
    ask: document.getElementById('ask'),
    find: document.getElementById('find'),
    findHits: document.getElementById('find-hits'),
    findFoot: document.getElementById('find-foot'),
    fbar: document.getElementById('fbar'),
    fSign: document.getElementById('f-sign'),
    fInput: document.getElementById('f-input'),
    fCount: document.getElementById('f-count'),
    prog: document.getElementById('prog'),
    pHead: document.getElementById('p-head'),
    pNow: document.getElementById('p-now'),
    pFill: document.getElementById('p-fill'),
    pNum: document.getElementById('p-num'),
    shell: document.getElementById('shell'),
    sTabs: document.getElementById('s-tabs'),
    sTitle: document.getElementById('s-title'),
    sAbout: document.getElementById('s-about'),
    sPanes: document.getElementById('s-panes'),
    sPreview: document.getElementById('s-preview'),
    report: document.getElementById('report'),
    rName: document.getElementById('r-name'),
    rAbout: document.getElementById('r-about'),
    rQ: document.getElementById('r-q'),
    chat: document.getElementById('chat'),
    cName: document.getElementById('c-name'),
    cAbout: document.getElementById('c-about'),
    cBody: document.getElementById('c-body'),
    cIn: document.getElementById('c-in'),
    cHint: document.getElementById('c-hint'),
    rRows: document.getElementById('r-rows'),
    rFoot: document.getElementById('r-foot'),
    view: document.getElementById('view'),
    vName: document.getElementById('v-name'),
    vAbout: document.getElementById('v-about'),
    vBody: document.getElementById('v-body'),
    vPic: document.getElementById('v-pic'),
    vRead: document.getElementById('v-read'),
    vFoot: document.getElementById('v-foot'),
};

/// The operation currently running, if any, so its progress has somewhere to
/// land and Esc has something to call off.
let running = null;

/// Ask before doing. Resolves true only on a deliberate yes.
///
/// Nothing in cian reaches the disk without passing through here: the terminal
/// build's whole promise is that a slip costs nothing, and a front end that
/// quietly skipped the asking would not be the same program.
function confirm(head, body, choices = {}) {
    el.ask.querySelector('.head').textContent = head;
    el.ask.querySelector('.body').textContent = body;
    const yesBtn = el.ask.querySelector('[data-answer="yes"]');
    // The plain answer can be renamed per question — a transfer's Enter means
    // "skip what already exists", a delete's means "to the trash" — and the
    // stronger variants ride on their own letter, as the terminal build has
    // them: `a` is the "I really mean it" key, `r` renames on the way.
    yesBtn.textContent = tr(`${choices.yes ?? 'Do it'}  (Enter)`, `${choices.yes ?? '実行'}  (Enter)`);
    const extras = choices.extras ?? [];
    for (const x of extras) {
        const b = document.createElement('button');
        b.dataset.answer = x.key;
        b.textContent = `${x.label}  (${x.key})`;
        yesBtn.before(b);
    }
    el.ask.hidden = false;
    // The focus goes where Enter goes.
    //
    // It used to sit on the safe button, meaning to make leaning on the
    // keyboard harmless — but Enter answers yes here whatever has the focus,
    // so it was not protecting anything. All it did was put a ring around
    // やめる while the key labelled (Enter) did 実行, which reads as the
    // opposite of what happens. Being asked at all is the protection.
    yesBtn.focus();
    return new Promise((resolve) => {
        const done = (answer) => {
            el.ask.hidden = true;
            el.ask.removeEventListener('click', onClick);
            document.removeEventListener('keydown', onKey, true);
            for (const b of el.ask.querySelectorAll('.buttons button')) {
                if (b.dataset.answer !== 'yes' && b.dataset.answer !== 'no') b.remove();
            }
            yesBtn.textContent = tr('Do it  (Enter)', '実行  (Enter)');
            resolve(answer);
        };
        const onClick = (e) => {
            const a = e.target.dataset && e.target.dataset.answer;
            if (a === 'yes') done(true);
            else if (a === 'no') done(false);
            else if (a) done(a);
        };
        const onKey = (e) => {
            if (e.key === 'Escape' || e.key === 'n') { e.stopPropagation(); done(false); }
            else if (e.key === 'Enter' || e.key === 'y') { e.stopPropagation(); done(true); }
            else if (extras.some((x) => x.key === e.key)) { e.stopPropagation(); done(e.key); }
            else if (e.key !== 'Tab') { e.stopPropagation(); }
        };
        el.ask.addEventListener('click', onClick);
        // Captured, so the listing's own keys never see these.
        document.addEventListener('keydown', onKey, true);
    });
}

/// What kind of news a message carries — cian-tui's `message_color`
/// (render.rs:3223), classified from the text for the same reason: messages
/// come from a hundred call sites, and most already begin with a glyph or
/// contain an unambiguous word. The window had two states, "bad" and "not
/// bad", so a cancellation and a completion looked alike.
function messageKind(msg, bad) {
    if (bad) return 'bad';
    if (!msg) return '';
    if (/^✔|^保存|しました$|できました/.test(msg)) return 'good';
    if (/^⚠|中止|未保存|やめました/.test(msg)) return 'warn';
    if (/できません|失敗|ありません|知りません|エラー/.test(msg)) return 'bad';
    return '';
}

/// The transient half of the status line. The chips beside it are rebuilt by
/// drawStatus(); this is the one thing that changes because something was
/// *said* rather than because something *is*.
const status = { msg: '', bad: false };

function say(text, bad = false) {
    status.msg = text;
    status.bad = bad;
    drawStatus();
    // Every state change in this program passes through here on its way to
    // saying what happened, which makes it the one place the hint bar can be
    // kept honest without threading a call through eighty functions.
    drawHints();
}

/// Free space per pane, fetched when the pane lands somewhere new. Cached by
/// path: the status line redraws on every keystroke and a statvfs per `j`
/// would be a disk question asked two hundred times for one answer.
const disk = { left: { at: null, v: null }, right: { at: null, v: null } };
/// And the branch bar, cached the same way: it costs a `git status`, which is
/// a per-directory question and not a per-keystroke one.
const repo = { left: { at: null, v: null, drawn: null }, right: { at: null, v: null, drawn: null } };

async function freshenDisk(which) {
    const pane = state[which];
    const d = disk[which];
    if (!pane || pane.remote || pane.cwd === d.at) return;
    d.at = pane.cwd;
    try {
        // Straight through the bridge, not ask(): a pane that cannot answer
        // (an archive, a listing mid-change) is a chip that stays blank, not
        // a dialog.
        d.v = await window.cian.call('df', { pane: which });
    } catch { d.v = null; }
    try {
        const r = await window.cian.call('vcs', { pane: which });
        // On `kind`, not on `branch`: only git reports a branch, so keying on
        // it threw away every Subversion answer — and with it the menu's way
        // of telling which of the two groups belongs here.
        repo[which].v = r && r.kind ? r : null;
    } catch { repo[which].v = null; }
    repo[which].at = pane.cwd;
    drawStatus();
    // And the listing once, because the answer carries a badge per row now.
    // The status chip was the only thing this fed, so the first version of
    // the git column drew nothing: the marks arrive a moment after the rows
    // they belong to, and nothing asked for the rows again.
    //
    // Once per directory, tracked separately from `at`. Calling `draw` from
    // here re-enters this function, and the guard at the top is the *disk*
    // cache's — in a directory where something else reassigns `state` between
    // the two, that guard opens again and the pair spin. The window hung on
    // the first repository it was pointed at.
    if (repo[which].v && repo[which].v.marks && repo[which].drawn !== pane.cwd) {
        repo[which].drawn = pane.cwd;
        draw(which);
    }
}

/// The terminal build's status row (render.rs draw_status), chip for chip:
/// badge → counts → marks → the file under the cursor → the filter → the
/// disk → the running operation → the message. The badge and the message are
/// never dropped; the chips clip from the left (CSS does the dropping).
function drawStatus() {
    const which = state.focus;
    const pane = state[which];
    // The badge: which surface has the keys, and in what mode.
    const mode = term.on && term.focused ? ['S', '']
        : visual.on ? [which === 'left' ? 'L' : 'R', ' VISUAL']
        : filter.on ? [which === 'left' ? 'L' : 'R', ' FILTER']
        : [which === 'left' ? 'L' : 'R', ''];
    el.stBadge.textContent = mode[0] + mode[1];
    el.stBadge.className = mode[1] === ' VISUAL' ? 'visual' : mode[1] === ' FILTER' ? 'filter' : '';
    // …and the same colour on the frame of the surface that has the keys.
    //
    // cian-tui paints the *focused pane's border* with the mode's colour
    // (`focus_badge_color`, render.rs:2134) — so `/` turns the pane you are
    // narrowing green, and `:` turns it purple. The window painted only the
    // prompt row at the bottom of the screen, which is the one place the eye
    // is not: you look at the listing while you narrow it. Reported as the
    // green being in the wrong place, and it was.
    el.work.dataset.mode = term.on && term.focused ? 'shell'
        : visual.on ? 'visual'
        : filter.on ? (filter.mode === 'cmd' ? 'cmd' : 'search')
        : '';
    const chips = [];
    const chip = (cls, text) => {
        const s = document.createElement('span');
        s.className = cls;
        s.textContent = text;
        chips.push(s);
    };
    if (pane) {
        chip('n', tr(`${pane.entries.length} items`, `${pane.entries.length} 件`));
        if (pane.marked > 0) chip('mk', tr(`${pane.marked} marked`, `マーク ${pane.marked}`));
        const row = pane.entries[pane.cursor];
        if (row && !row.parent) chip('cur', row.name);
        if (pane.filter) chip('flt', tr(`filter /${pane.filter} (${pane.entries.length})`, `フィルタ /${pane.filter} (${pane.entries.length} 件)`));
    }
    // The branch, with ahead/behind and how many files are changed — green
    // when the tree is clean, amber when it is not. cian-tui's own chip
    // (render.rs), and the one line a developer glances at most.
    const g = repo[which]?.v;
    if (g && g.branch) {
        const bits = [`\u{e0a0} ${g.branch}`];
        if (g.ahead > 0) bits.push(`↑${g.ahead}`);
        if (g.behind > 0) bits.push(`↓${g.behind}`);
        if (g.changed > 0) bits.push(` ✚${g.changed}`);
        chip(g.changed > 0 ? 'git dirty' : 'git', bits.join(' '));
    }
    const d = disk[which]?.v;
    if (d && d.total > 0) {
        const usedPct = (d.total - d.available) / d.total;
        chip(`disk${usedPct >= 0.95 ? ' crit' : usedPct >= 0.8 ? ' warn' : ''}`,
            tr(`${human(d.available)} free / ${human(d.total)}`, `空き ${human(d.available)} / ${human(d.total)}`));
    }
    if (running) {
        // Per cent where the bytes are known — the same number the bar shows,
        // so the chip left behind by `b` is not a different measurement.
        const pct = running.bytesTotal > 0
            ? `${Math.round((running.bytes / running.bytesTotal) * 100)}%`
            : `${running.done ?? 0} / ${running.total}`;
        chip('op', `↻ ${running.verb} ${pct}`);
    }
    // Something is being asked of the engine and is taking a moment. It has
    // no percentage to report — `:du` cannot know how deep the tree is until
    // it has walked it — so this says only that the key landed.
    if (busy.n > 0) chip('op', tr('⋯ running', '⋯ 実行中'));
    el.stChips.replaceChildren(...chips);
    el.stMsg.textContent = status.msg ? `◂ ${status.msg}` : '';
    const kind = messageKind(status.msg, status.bad);
    el.stMsg.className = kind;
    // The active shell's own title on the right, the terminal build's rule:
    // suppressed while a message is showing — the message wins the space.
    el.stShell.textContent = !status.msg && term.on ? el.sTitle.textContent : '';
}

/// Bytes, in the width a listing can spare. A directory shows `—` as the
/// terminal build has it — a dash is "not a number here", where a blank reads
/// as a cell that failed to load. `..` alone shows nothing.
function size(row) {
    if (row.parent) return '';
    if (row.is_dir) return '—';
    const u = ['B', 'K', 'M', 'G', 'T', 'P', 'E'];
    let n = row.len, i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return (i === 0 ? n : n.toFixed(n < 10 ? 1 : 0)) + u[i];
}

/// Which surface the keys are actually in, said on `#work` so the stylesheet
/// can tell "this is the current pane" from "this is where you are typing".
/// The file pane wore the accent frame while the shell had the keyboard.
function markFocus() {
    el.work.dataset.focus = term.on && term.focused ? 'shell' : 'files';
}

/// The one owner of "the keys are in the shell now".
///
/// Ten places set `term.focused` and nine of them also had to remember the
/// class beside it — so the frame and the flag drifted apart, and the tenth
/// site somebody adds next month would have drifted too. Same shape as the
/// undo stacks: if two things must move together, one function moves them.
function setShellFocus(on) {
    term.focused = on;
    el.shell.classList.toggle('on', on);
    // The preview borrows this panel's pixels; giving the shell the keys
    // gives them straight back, and taking them away brings the preview
    // again. cian-tui's bargain exactly (preview.rs): the PTY never stopped.
    if (preview.on) {
        if (on) paintPreview(null);
        else { preview.at = null; showPreview(); }
    }
    markFocus();
}

/// Above this many entries a listing draws only what is on screen. Below it,
/// everything is built exactly as before — see the note in `draw`.
const VIRTUAL_FROM = 300;
/// Rows built above and below the visible window, so a small scroll does not
/// have to wait for a repaint to have something to show.
const VIRTUAL_PAD = 12;
/// One pending repaint per pane, cancelled by the next scroll event.
const scrollSoon = { left: 0, right: 0 };

/// A blank block standing in for rows that were not built. Keeps the
/// scrollbar the length it would have been.
function spacer(px) {
    const d = document.createElement('div');
    d.style.height = `${px}px`;
    d.style.flex = '0 0 auto';
    return d;
}

/// How tall one row is, measured rather than assumed: `--cell-h` follows the
/// font size, which Ctrl+= changes.
function rowHeight(rows) {
    const probe = rows.querySelector('.row');
    const h = probe ? probe.getBoundingClientRect().height : 0;
    if (h > 0) return h;
    const css = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--cell-h'));
    return css > 0 ? css : 26;
}

function draw(which) {
    const pane = state[which];
    const root = el[which];
    root.classList.toggle('active', state.focus === which);
    markFocus();
    if (!pane) return;
    root.classList.toggle('remote', !!pane.remote);
    // What this pane is showing, said in the one line people actually read.
    // A server's rows look exactly like a local directory's, and mistaking
    // somebody's server for your own disk is worth a word and a frame.
    // The tab strip, drawn only when there is a second tab: a row of chrome
    // showing one tab is a row of chrome saying nothing.
    const strip = root.querySelector('.tabs');
    if (!pane.tabs || pane.tabs.length < 2) {
        strip.replaceChildren();
    } else {
        strip.replaceChildren(...pane.tabs.map((name, i) => {
            const t = document.createElement('span');
            t.textContent = name;
            if (i === pane.tab) t.className = 'on';
            t.addEventListener('mousedown', () => goTab(which, { at: i }));
            return t;
        }));
    }
    const where = pane.remote
        ? `${pane.remote}:${pane.cwd}`
        : pane.archive
            ? tr(`inside ${pane.archive.split(/[\\/]/).pop()}`, `${pane.archive.split(/[\\/]/).pop()} の中`)
            : (pane.flat ? `${pane.flat} — ${pane.cwd}` : pane.cwd);
    // Which side you are on, but only when the other side is not on screen.
    // With both panes visible the highlight says it; with one, Tab would
    // otherwise change everything and announce nothing.
    // **どちら側かは、下の帯が言っている。** 状態行の先頭に `L` / `R` が
    // 出ているのに、パンくずの頭にも `[左]` と書いていた ── 同じことを2回
    // 言っていて、しかも読む場所として悪い方（一番読まれる行の頭）を
    // 使っていた。2026-09-05:「めっちゃダサい」。
    const lead = '';
    // A breadcrumb rather than a grey string: the parents dim, the folder you
    // are *in* in the text colour. It is the most-read line in the window and
    // it was set smaller than the date column and in the same ink as the
    // things nobody reads.
    const crumb = root.querySelector('.crumb');
    const cut = Math.max(where.lastIndexOf('/'), where.lastIndexOf('\\'));
    const here = cut >= 0 && cut < where.length - 1 ? where.slice(cut + 1) : '';
    const parents = here ? where.slice(0, cut + 1) : where;
    // One isolated run holding both parts, so the right-to-left box clips the
    // head without reordering anything inside it.
    const path = document.createElement('span');
    path.className = 'path';
    // Each ancestor is its own target. cian-tui registers a click rect per
    // segment (mouse.rs:803) — a path you can read and not click is a path
    // you retype into `z`.
    const clickable = !pane.remote && !pane.archive && !pane.flat;
    if (clickable) {
        let sofar = '';
        for (const seg of (lead + parents).split(/(?<=[\\/])/)) {
            sofar += seg;
            const at = sofar.slice(lead.length);
            const s2 = document.createElement('span');
            s2.className = 'seg';
            s2.textContent = seg;
            if (at.length > 1) {
                s2.addEventListener('mousedown', (e) => {
                    e.stopPropagation();
                    setShellFocus(false);
                    state.focus = which;
                    landOn(at.replace(/[\\/]$/, '') || '/', true).then(() => say(state[which].cwd));
                });
            }
            path.append(s2);
        }
    } else {
        path.append(document.createTextNode(lead + parents));
    }
    const tail = document.createElement('span');
    tail.className = 'here';
    tail.textContent = here;
    path.append(tail);
    // ◀ ▶ — this pane's history, one click each. cian-tui puts them in the
    // pane's title bar (`nav_rects`, mouse.rs:791) and the window had the
    // journey on Alt+← / Alt+→ and nowhere a hand holding a mouse could find
    // it. Drawn always, dimmed when there is nowhere to go: a control that
    // appears and disappears is a control you cannot aim at.
    const nav = document.createElement('span');
    nav.className = 'nav';
    for (const [glyph, fwd, what] of [['◀', false, tr("back", '戻る')], ['▶', true, tr("forward", '進む')]]) {
        const b = document.createElement('span');
        b.className = 'navb';
        b.textContent = glyph;
        b.title = what;
        b.addEventListener('mousedown', (e) => {
            e.stopPropagation();
            setShellFocus(false);
            state.focus = which;
            step(fwd ? 'forward' : 'back');
        });
        nav.append(b);
    }
    crumb.replaceChildren(nav, path);

    const rows = root.querySelector('.rows');
    // Rebuilt whole. A listing is a few hundred rows and Chromium does not
    // notice; the moment it does, this is where a windowed list goes.
    const frag = document.createDocumentFragment();
    rows.classList.toggle('details', viewMode === 'details');
    rows.classList.toggle('classic', viewMode === 'classic');
    // The columns that fit, decided by the pane's real width — the terminal
    // build's progressive drop (render.rs: the date needs ~52 columns, the
    // size ~34), translated through the half-width cell of the current size.
    const ch = FONT.at / 2;
    root.classList.toggle('no-when', viewMode !== 'details' && root.clientWidth < 52 * ch);
    root.classList.toggle('no-size', viewMode !== 'details' && root.clientWidth < 34 * ch);
    // The ☁ column only where something is actually in the cloud, as the
    // terminal build allocates it — two blank cells on every row otherwise.
    root.classList.toggle('no-cloud', !pane.entries.some((e) => e.cloud));
    // The column header, in classic and details both — the terminal build
    // draws one in every list view, and a table without its sort marker is a
    // table you have to remember about. Clicking a heading sorts by it.
    const head = root.querySelector('.dhead');
    if (head.dataset.built !== `${viewMode}/${lang}`) {
        // Keyed by the language too: the headings are words, and a cache on
        // the view alone kept 名前 / サイズ / 日時 after the switch.
        head.dataset.built = `${viewMode}/${lang}`;
        head.replaceChildren();
        const cols = viewMode === 'details'
            ? [['glyph', '', null], ['name', tr("Name", '名前'), 'name'], ['cloud', '', null],
               ['size', tr("Size", 'サイズ'), 'size'], ['kind', tr("Kind", '種類'), 'ext'], ['when', tr("Date", '日時'), 'date']]
            : [['cloud', '', null], ['mark', '', null], ['glyph', '', null],
               ['name', tr("Name", '名前'), 'name'], ['size', tr("Size", 'サイズ'), 'size'], ['when', tr("Date", '日時'), 'date']];
        for (const [cls, label, key] of cols) {
            const c = document.createElement('span');
            c.className = cls;
            c.textContent = label;
            c.dataset.key = key || '';
            if (key) {
                c.classList.add('sortable');
                c.addEventListener('mousedown', () => applySort(key));
            }
            head.append(c);
        }
    }
    if (!head.hidden) {
        // Which column the listing is actually sorted by. Explorer marks it,
        // and a table that does not is a table you have to remember about.
        // The pane's own order, not a global — sorting is per pane, and a
        // remembered "current sort" described the wrong pane after a Tab.
        for (const c of head.children) {
            c.textContent = c.textContent.replace(/ [↑↓]$/, '');
            if (c.dataset.key && c.dataset.key === pane.sort_key) {
                c.textContent += pane.sort_reverse ? ' ↓' : ' ↑';
            }
        }
    }
    // **Only the rows you can see.**
    //
    // Every entry became a DOM node, and `draw()` runs on every cursor move —
    // so one `j` in `C:\Windows\System32` built five thousand nodes and threw
    // five thousand away. Measured here: 645ms a keystroke at 5000 rows,
    // against 2ms at eight. That is the whole of "もっさりする"; the engine
    // reads the directory once and is not the slow part.
    //
    // Rows are a fixed height (`--cell-h`), so the window is arithmetic: two
    // spacers stand in for what is above and below, and the scrollbar keeps
    // the size and position it would have had. Below `VIRTUAL_FROM` nothing
    // changes at all — a directory of forty files was never the problem, and
    // a mechanism that only runs where it is needed cannot break the case
    // where it is not.
    //
    const virtual = pane.entries.length >= VIRTUAL_FROM;
    const cellH = virtual ? rowHeight(rows) : 0;
    let from = 0;
    let to = pane.entries.length;
    if (virtual) {
        const seen = Math.max(1, Math.ceil(rows.clientHeight / cellH));
        // Centred on whichever of the cursor and the scroll position is
        // driving: `j` moves the cursor, the wheel moves the scroll, and both
        // have to end up drawn.
        const top = Math.floor(rows.scrollTop / cellH);
        const lo = Math.min(top, pane.cursor);
        const hi = Math.max(top + seen, pane.cursor);
        from = Math.max(0, lo - VIRTUAL_PAD);
        to = Math.min(pane.entries.length, hi + VIRTUAL_PAD);
        if (from > 0) frag.append(spacer(from * cellH));
    }
    pane.entries.slice(from, to).forEach((row, n) => {
        const i = from + n;
        const div = document.createElement('div');
        div.className = 'row'
            + (row.is_dir ? ' dir' : '')
            + kindClassOf(row)
            + (row.marked ? ' marked' : '')
            + (i === pane.cursor ? ' cursor' : '');
        // Draggable out of the window — to the desktop, to Finder or
        // Explorer, to a mail draft, to another application. **cian-tui
        // cannot do this at any price**: a terminal program is not a drag
        // source, which is why its answer is `Shift+P` and the clipboard.
        // The marked files if there are any, otherwise this one.
        if (!row.parent && !pane.remote && !pane.archive && window.cian.startDrag) {
            div.draggable = true;
            div.addEventListener('dragstart', (e) => {
                const marked = pane.entries.filter((x) => x.marked && !x.parent);
                const paths = (marked.length ? marked : [row]).map((x) => x.path);
                // Electron takes the drag over from here; the browser's own
                // would carry text, not files.
                e.preventDefault();
                window.cian.startDrag(paths);
            });
        }
        const name = document.createElement('span');
        name.className = 'name';
        name.textContent = row.parent ? '..' : row.name;
        if (viewMode === 'details') {
            // Explorer's details, in Explorer's order: icon, name, size,
            // kind, date. The icon belongs here too — a details list with no
            // picture in it reads as a table of strings, and the kind of a
            // file is the first thing the eye wants.
            const g = document.createElement('span');
            g.className = 'glyph';
            g.textContent = glyphFor(row);
            const cl = document.createElement('span');
            cl.className = 'cloud';
            cl.textContent = row.cloud ? '☁' : '';
            const len = document.createElement('span');
            len.className = 'size';
            len.textContent = size(row);
            const kind = document.createElement('span');
            kind.className = 'kind';
            kind.textContent = kindOf(row);
            const gb = gitBadge(which, row);
            const w = document.createElement('span');
            w.className = 'when';
            w.textContent = when(row);
            div.append(gb, g, name, cl, len, kind, w);
        } else {
            // Classic, in the terminal build's column order: mark, icon,
            // name, then the numbers. The ● column is what makes ten marked
            // rows readable at a glance — a colour on the name alone
            // vanishes into whichever row is the cursor.
            const mk = document.createElement('span');
            mk.className = 'mark';
            mk.textContent = row.marked ? '●' : '';
            const g = document.createElement('span');
            g.className = 'glyph';
            g.textContent = iconFor(row);
            const cl = document.createElement('span');
            cl.className = 'cloud';
            cl.textContent = row.cloud ? '☁' : '';
            const len = document.createElement('span');
            len.className = 'size';
            len.textContent = size(row);
            const w = document.createElement('span');
            w.className = 'when';
            w.textContent = when(row);
            // The terminal build's column order: git, ☁, mark, icon, name,
            // then the numbers.
            div.append(gitBadge(which, row), cl, mk, g, name, len, w);
        }
        div.addEventListener('mousedown', async (e) => {
            state.focus = which;
            // Add to the marks rather than moving the cursor — cian-tui's
            // grid does this (grid.rs:352), and it is what every list in
            // every OS does with that modifier.
            //
            // Which modifier, though, is the platform's: on macOS Ctrl+click
            // *is* the secondary click, so binding this to Ctrl there marks a
            // row and opens the context menu on the same press. Cmd is the
            // Mac's add-to-selection and Ctrl is everywhere else's.
            if (ADD_TO_MARKS(e)) {
                e.preventDefault();
                const next = await ask('mark', { pane: which, at: i });
                if (next) { state[which] = next; draw(which); }
                return;
            }
            pane.cursor = i;
            draw('left'); draw('right');
        });
        div.addEventListener('dblclick', () => { state.focus = which; enter(); });
        // Right-click opens the same menu Shift+Enter does, on the row you
        // pointed at. A file manager where the right button does nothing is a
        // file manager that feels broken before you have tried anything.
        div.addEventListener('contextmenu', (e) => {
            e.preventDefault();
            state.focus = which;
            pane.cursor = i;
            draw('left'); draw('right');
            openMenu(CONTEXT);
        });
        frag.append(div);
    });
    if (virtual && to < pane.entries.length) frag.append(spacer((pane.entries.length - to) * cellH));
    rows.replaceChildren(frag);

    // Keep the cursor on screen without yanking the view about.
    const at = virtual
        ? rows.querySelector('.row.cursor')
        : rows.children[pane.cursor];
    if (at) at.scrollIntoView({ block: 'nearest' });
    // Scrolling reveals rows that were never built. Attached once per pane —
    // the listener outlives the rows it draws.
    if (virtual && !rows.dataset.watching) {
        rows.dataset.watching = '1';
        rows.addEventListener('scroll', () => {
            if (scrollSoon[which]) return;
            // One repaint per frame at most: a wheel spin fires scroll far
            // faster than anything can usefully be drawn.
            scrollSoon[which] = requestAnimationFrame(() => {
                scrollSoon[which] = 0;
                if (state[which]) draw(which);
            });
        }, { passive: true });
    }

    // The chip row follows every repaint — the counts, the marks and the
    // name under the cursor are all things a repaint may have changed — and
    // the disk chip refreshes itself only when the pane landed somewhere new.
    drawStatus();
    freshenDisk(which);
    placeGrips();
}

/// How many engine calls are outstanding and have been slow enough to be
/// worth admitting to.
///
/// `:du` over a big tree takes seconds, and the window said nothing at all
/// while it did — no way to tell "working" from "the key did not land".
/// Anything under a quarter of a second stays silent, because a chip that
/// flashes on every keystroke is worse than no chip.
const busy = { n: 0 };
const BUSY_AFTER_MS = 250;

async function ask(method, params) {
    let slow = null;
    let counted = false;
    slow = setTimeout(() => {
        counted = true;
        busy.n += 1;
        drawStatus();
    }, BUSY_AFTER_MS);
    try {
        // Every request states where both cursors are. The cursor moves here,
        // on every `j`, without asking the engine — so the engine's own copy
        // went stale, and `r` after three presses of `j` renamed a file three
        // rows up. Both, not just the focused one, because `=` compares what
        // the two of them are pointing at. Stated once, here, rather than
        // remembered at each of the dozen call sites.
        if (state.left && state.right) {
            params = {
                cursors: { left: state.left.cursor, right: state.right.cursor },
                ...params,
            };
        }
        return await window.cian.call(method, params);
    } catch (e) {
        say(String(e.message || e), true);
        return null;
    } finally {
        clearTimeout(slow);
        if (counted) { busy.n -= 1; drawStatus(); }
    }
}

/// `Shift+P` — the files themselves, for Finder or Explorer to paste.
///
/// `p` puts the path text on the clipboard. These are two different things and
/// the terminal build keeps them on two keys for a reason: pasting a path into
/// a folder is not pasting a file into it.
async function clipFiles() {
    const r = await ask('clipfiles', { pane: state.focus });
    if (!r) return;
    say(tr(`${r.count} on the clipboard (Finder can paste them)`, `${r.count} 件をクリップボードへ（Finder で貼り付けられます）`));
}

async function refresh() {
    const s = await ask('state', {});
    if (!s) return;
    state.left = s.left;
    state.right = s.right;
    draw('left'); draw('right');
    say(tr(`${state.left.entries.length} / ${state.right.entries.length} items`, `${state.left.entries.length} 件 / ${state.right.entries.length} 件`));
}

/// Mark the row under the cursor, or every row.
/// Mark, and step. `Space` steps down and `Shift+Space` up — the terminal
/// build has both, because marking a run upwards is as common as downwards
/// and doing it with Space and k is two hands' worth of keys.
async function mark(all, step = 1) {
    const which = state.focus;
    const next = await ask(all ? 'markall' : 'mark', { pane: which });
    if (!next) return;
    if (!all && step < 0) {
        // The engine always steps down after marking; going up is two steps
        // back from where it left the cursor.
        next.cursor = Math.max(0, next.cursor - 2);
    }
    state[which] = next;
    draw(which);
    say(next.marked ? tr(`${next.marked} marked`, `${next.marked} 件マーク`) : tr('nothing marked', 'マークなし'));
}

/// The bar, while an operation is running.
///
/// Shown for as long as the work takes, as the terminal build shows it, and
/// dismissed with `b` *without cancelling* — the status chip carries the rest.
/// `op_bar_hidden` there, `prog.hidden` here.
const prog = { hidden: false, stalledAt: 0 };

/// A path shortened from the middle, keeping both ends — the terminal
/// build's `truncate_middle`. Both ends carry: the head says which volume or
/// project, the tail says which file. Cutting either off answers half the
/// question the line exists to answer.
///
/// **桁で測る、UTF-16 の単位ではなく。** 端末版の `truncate_middle` は
/// 「全角は2つぶん」で予算を切っていて、こちらは `text.length` で切っていた
/// ── **同じパスを二つの前端が違う長さに詰めていた**（日本語のフォルダ名が
/// あると窓版は倍近く残し、箱からはみ出す）。2026-09-06、`scripts/widths.py`。
function truncateMiddle(text, max = 68) {
    if (cellWidth(text) <= max) return text;
    const keep = max - 1;
    const headBudget = Math.ceil(keep / 2);
    const tailBudget = keep - headBudget;
    const take = (chars, budget) => {
        let out = '';
        let used = 0;
        for (const ch of chars) {
            const w = cellWidth(ch);
            if (used + w > budget) break;
            out += ch;
            used += w;
        }
        return out;
    };
    const head = take([...text], headBudget);
    const tail = take([...text].reverse(), tailBudget);
    return `${head}…${[...tail].reverse().join('')}`;
}

function drawProg() {
    if (!running || prog.hidden) { el.prog.hidden = true; return; }
    el.prog.hidden = false;
    el.pHead.textContent = tr(`${running.verb}`, `${running.verb}中`);
    el.pNow.textContent = truncateMiddle(running.path || '');
    // By bytes where they are known, by files otherwise — cian-core's own
    // rule (Progress::fraction), so the bar and the numbers cannot disagree.
    const frac = running.bytesTotal > 0
        ? running.bytes / running.bytesTotal
        : (running.total > 0 ? running.done / running.total : 0);
    el.pFill.style.width = `${Math.round(Math.min(1, frac) * 100)}%`;
    const secs = Math.round((running.ms ?? 0) / 1000);
    const elapsed = secs >= 60 ? tr(`${Math.floor(secs / 60)}m${secs % 60}s`, `${Math.floor(secs / 60)}分${secs % 60}秒`) : tr(`${secs}s`, `${secs}秒`);
    const bytes = running.bytesTotal > 0
        ? `${human(running.bytes)} / ${human(running.bytesTotal)}   `
        : '';
    // Stalled: the work has not reported for a while. Said plainly, because a
    // bar that has not moved in twenty seconds is otherwise indistinguishable
    // from a program that has died.
    const still = prog.stalledAt && performance.now() - prog.stalledAt > 8000
        ? tr(`   ⚠ nothing for ${Math.round((performance.now() - prog.stalledAt) / 1000)}s`, `   ⚠ ${Math.round((performance.now() - prog.stalledAt) / 1000)} 秒動いていません`)
        : '';
    el.pNum.textContent =
        tr(`${Math.round(Math.min(1, frac) * 100)}%   ${bytes}(${running.done} / ${running.total})   ·   ${elapsed}${still}`, `${Math.round(Math.min(1, frac) * 100)}%   ${bytes}(${running.done} / ${running.total} 件)   ·   ${elapsed}${still}`);
}

/// Copy, move or delete whatever is marked — or the row under the cursor when
/// nothing is. The destination is the other pane, which is the whole idea of
/// two panes side by side.
async function operate(kind) {
    const which = state.focus;
    const pane = state[which];
    if (!pane) return;
    // And `d` deletes it, for the same reason.
    if (pane.archive && kind === 'delete') {
        const row = pane.entries[pane.cursor];
        if (row && !row.parent) { await archiveEdit(row, 'delete'); return; }
    }

    // What is about to happen, named, before anything happens. The count comes
    // from the same rule the engine will use, so the sheet cannot promise one
    // thing and the engine do another.
    const chosen = pane.entries.filter((r) => !r.parent && r.marked);
    const here = pane.entries[pane.cursor];
    const rows = chosen.length ? chosen : (here && !here.parent ? [here] : []);
    if (!rows.length) {
        say(tr('nothing to work on', '対象がありません'));
        return;
    }
    const dest = state[which === 'left' ? 'right' : 'left'];
    const verb = { copy: tr('copy', 'コピー'), move: tr('move', '移動'), delete: tr('delete', '削除') }[kind];
    // **The other pane may be standing inside a zip.** An archive view is
    // synthetic and the pane still remembers the directory it walked in from,
    // so an ordinary copy landed the files *beside* the archive and said
    // "copied" — right message, wrong place, nothing on screen to tell you.
    // cian-tui has asked "add to the zip?" since it could read one
    // (actions.rs:92). The engine refuses the plain copy as well; this is the
    // half that can name the archive in the question.
    if (kind !== 'delete' && dest.archive) {
        const zip = dest.archive.split(/[\\/]/).pop();
        if (kind === 'move') {
            say(tr('zip takes copies only — move is not supported', 'zip へはコピー（追加）のみ — 移動は未対応'), true);
            return;
        }
        if (!await confirm(tr(`Add ${rows.length} to ${zip}`, `${rows.length} 件を ${zip} に追加`),
            rows.map((r) => r.name).join('\n')
            + tr('\n\nthe archive is rebuilt with them in it', '\n\nアーカイブを作り直します'))) {
            say(tr('stopped', 'やめました'));
            return;
        }
        say(tr('rebuilding the archive…', 'アーカイブを作り直しています…'));
        const done = await ask('zipadd', { pane: which, paths: rows.map((r) => r.path) });
        if (!done) return;
        state.left = done.left;
        state.right = done.right;
        draw('left');
        draw('right');
        say(tr(`added ${done.added} to ${zip}`, `${done.added} 件を ${zip} に追加しました`));
        return;
    }
    // **A remote pane's `cwd` is where it was before it connected.** Naming
    // it here told you the copy was going to a local folder while the engine
    // sent it to a server — the sheet and the machine describing two different
    // operations, and the sheet is the one you read.
    const where = dest.remote
        ? `${dest.remote}:${dest.remote_path ?? ''}`
        : dest.archive ? dest.archive : dest.cwd;
    const head = kind === 'delete'
        ? tr(`${rows.length} to the trash`, `${rows.length} 件をゴミ箱へ`)
        : tr(`${verb} ${rows.length}: → ${where}`, `${rows.length} 件を${verb}: → ${where}`);
    // Every name, not a summary. "12 件" tells you nothing about whether the
    // twelve are the ones you meant.
    //
    // **And for a folder going to a server, what is inside it.** `proj/` is
    // one row and can be four thousand files over somebody else's network;
    // the sheet said "1 件" and the question could not be answered from what
    // it showed. The count comes from the planner the transfer itself runs
    // on, so the sheet cannot promise a number the job then disagrees with.
    // Local sources only — walking a remote tree to answer a dialog costs a
    // round trip per directory, and the answer would arrive after you had
    // decided.
    let body = rows.map((r) => r.name).join('\n');
    if (dest.remote && !pane.remote && rows.some((r) => r.is_dir)) {
        const plan = await ask('transferplan', { paths: rows.map((r) => r.path) });
        if (plan) {
            body = plan.rows.map((r) => (r.is_dir
                ? tr(`${r.name}/   (${r.files} files, ${human(r.bytes)})`,
                     `${r.name}/   （${r.files} ファイル・${human(r.bytes)}）`)
                : r.name)).join('\n')
                + tr(`\n\n${plan.files} files in all, ${human(plan.bytes)}`,
                     `\n\n合計 ${plan.files} ファイル・${human(plan.bytes)}`);
        }
    }
    // The terminal build's three answers. The plain yes *skips* what already
    // exists — it used to overwrite, silently, which is the one outcome a
    // confirmation exists to prevent. `a` overwrites on purpose; `r` renames
    // a single item on the way over.
    const answer = kind === 'delete'
        ? await confirm(head, body, { yes: tr('to the trash', 'ゴミ箱へ'), extras: [{ key: 'a', label: tr('delete for good', '完全削除') }] })
        : await confirm(head, body, {
            yes: tr(`${verb} (skipping same names)`, `${verb}（同名はスキップ）`),
            extras: [
                { key: 'a', label: tr('overwrite', '上書き') },
                ...(rows.length === 1 ? [{ key: 'r', label: tr('rename', '名前を変えて') }] : []),
            ],
        });
    if (!answer) {
        say(tr('stopped', 'やめました'));
        return;
    }
    if (answer === 'r') {
        // A single-item move/copy can be renamed on the way, seeded with the
        // name it arrived with.
        const name = await askFor(tr(`the name to ${verb} to`, `${verb}先の名前`), rows[0].name);
        if (!name) { say(tr('stopped', 'やめました')); return; }
        const r = await ask('transferas', {
            src: rows[0].path, dest: dest.cwd, name, move: kind === 'move',
        });
        if (r) { say(`${verb} → ${name}`); await reread(); }
        return;
    }
    const started = await ask(kind, {
        pane: which,
        conflict: answer === 'a' ? 'overwrite' : 'skip',
        mode: answer === 'a' ? 'permanent' : 'trash',
    });
    if (!started) return;
    // A move between two directories on the *same* server is a rename: the
    // engine does it there and back in one call, so there is no job to watch.
    if (started.renamed !== undefined) {
        await reread();
        say(tr(`moved ${started.renamed} on the server`, `サーバ上で ${started.renamed} 件を移しました`));
        return;
    }
    // A transfer reports through the same events as a local copy, so the bar
    // works unchanged — but the word on it should say which way it went.
    if (started.remote) {
        beginOp(started, kind, dest.remote
            ? tr('upload', 'アップロード')
            : tr('download', 'ダウンロード'));
        return;
    }
    beginOp(started, kind, verb);
}

/// Offer to redo a refused transfer with administrator rights.
///
/// The elevated process copies on its own, so there is no bar to show — cian
/// waits on it and says what happened. A declined UAC prompt comes back as an
/// error, which is the right outcome and is reported as one.
async function offerElevate(msg, verb) {
    const what = msg.elevate;
    const names = (what.paths || []).map((p) => p.split(/[\\/]/).pop());
    if (!await confirm(
        tr('Windows refused it — try again as administrator?', 'Windows に拒否されました — 管理者としてやり直しますか'),
        names.join('\n') + tr('\n\nWindows will ask for permission. The copy runs outside cian, so there is no progress bar.',
            '\n\nWindows が確認を出します。コピーは cian の外で走るので進捗バーは出ません。'))) {
        say(msg.errors.join('  /  '), true);
        return;
    }
    say(tr('waiting for the elevated copy…', '管理者権限のコピーを待っています…'));
    const done = await ask('elevate', what);
    if (!done) return;
    state.left = done.left;
    state.right = done.right;
    draw('left');
    draw('right');
    say(tr(`${verb} ${done.done} as administrator`, `管理者として ${done.done} 件を${verb}しました`));
}

/// Take up an operation the engine has just accepted — running now, or in
/// line behind one that is. A queued job gets no bar: there is nothing to
/// show yet, and the one on screen belongs to the job actually working.
function beginOp(started, kind, verb) {
    if (started.queued) {
        say(tr(`queued — ${started.queued} waiting (:queue lists them)`, `キューに追加 — ${started.queued} 件待ち（:queue で一覧）`));
        return;
    }
    running = {
        op: started.op, kind, verb,
        total: started.count, done: 0, bytes: 0, bytesTotal: 0, ms: 0, path: '',
    };
    prog.hidden = false;
    prog.stalledAt = performance.now();
    drawProg();
    say(tr(`${verb}… 0 / ${started.count}`, `${verb}中… 0 / ${started.count}`));
}

/// Land the cursor on a row. Every jump goes through here so that a jump
/// made mid-visual extends the selection — `G` in visual means "to the end",
/// and a `G` that moved the cursor without re-painting silently didn't.
function jumpTo(at) {
    const pane = state[state.focus];
    if (!pane || !pane.entries.length) return;
    pane.cursor = Math.max(0, Math.min(pane.entries.length - 1, at));
    draw(state.focus);
    if (visual.on) paintVisual();
    if (preview.on) showPreview();
}

async function clearMarksAndFilter() {
    const which = state.focus;
    if (state[which].filter) {
        const p = await ask('filter', { pane: which, text: '' });
        if (p) state[which] = p.pane ?? p;
    }
    if (state[which].marked > 0) {
        const p = await ask('unmarkall', { pane: which });
        if (p) state[which] = p;
    }
    draw(which);
    say(tr('marks and filter cleared', 'マークとフィルタを解除しました'));
}

function move(delta) {
    const pane = state[state.focus];
    if (!pane || !pane.entries.length) return;
    const last = pane.entries.length - 1;
    pane.cursor = Math.min(last, Math.max(0, pane.cursor + delta));
    draw(state.focus);
    if (visual.on) paintVisual();
    if (preview.on) showPreview();
}

async function enter() {
    const which = state.focus;
    const pane = state[which];
    if (!pane) return;
    const row = pane.entries[pane.cursor];
    if (!row) return;
    // Over the network the rows' paths are the server's, not this disk's —
    // opening one locally would look for a directory that is not here.
    if (pane.remote) {
        if (row.parent) { await remoteStep({ up: true }); return; }
        // A file opens — downloaded and read, Ctrl+S uploads it back. Enter
        // means "read it" on a server the same as it does on this disk.
        if (!row.is_dir) { await lookInside(); return; }
        await remoteStep({});
        return;
    }
    // Inside an archive, Enter on a file reads it — the same thing Enter
    // means on a file everywhere else. It used to fall through to the engine,
    // which refused with "まだ開けません" long after F3 could open it: the
    // message had outlived the limitation it described.
    if (pane.archive && !row.is_dir && !row.parent) {
        await lookInside();
        return;
    }
    // An archive is a directory you can walk into, which is what the terminal
    // build does with Enter — reading a zip as a list of names is `:lsar`, and
    // it is a different question.
    if (!row.is_dir && !row.parent && /\.(zip|tar|gz|tgz|7z|rar|jar)$/i.test(row.name)) {
        const r = await ask('enterarchive', { pane: which });
        if (!r) return;
        state[which] = r.pane;
        draw(which);
        say(tr(`inside ${r.archive.split(/[\\/]/).pop()}`, `${r.archive.split(/[\\/]/).pop()} の中`));
        return;
    }
    // A file is read here rather than handed to the desktop — the same
    // division the terminal build makes. Ctrl+Enter is the other one.
    if (!row.is_dir && !row.parent) {
        await lookInside();
        return;
    }
    const next = await ask('enter', { pane: which, cursor: pane.cursor });
    if (!next) return;
    state[which] = next;
    draw(which);
    say(next.cwd);
}

async function parent() {
    const which = state.focus;
    const next = await ask('parent', { pane: which });
    if (!next) return;
    state[which] = next;
    draw(which);
    say(next.cwd);
}

/// Ask for a line of text. Resolves to null when the answer is no answer.
///
/// The same sheet as the confirm, with a field in it: one dialog to know
/// rather than two, and the keys mean the same thing in both.
function askFor(head, initial = '', opts = {}) {
    const sheet = el.ask.querySelector('.sheet');
    el.ask.querySelector('.head').textContent = head;
    // **Wide when the answer is long.** The sheet was 380px whatever it was
    // asking for, and a path does not fit in 380px — you typed into a box that
    // showed you the last thirty characters of what you had written. A new
    // name fits either way, so the cost of the wider box is nothing.
    sheet.classList.toggle('wide', !!opts.wide);
    const body = el.ask.querySelector('.body');
    body.textContent = '';
    const input = document.createElement('input');
    // A password is never shown, and never pre-filled. cian has nowhere to
    // keep one that would be better than not keeping one, so it is asked for
    // each time and held only until the connection is made.
    input.type = opts.secret ? 'password' : 'text';
    input.value = opts.secret ? '' : initial;
    input.className = 'field';
    // A prompt inside the field, when the command it takes has one — `:` is
    // information about what you type, not about what the box is.
    if (opts.hint) input.placeholder = opts.hint;
    body.append(input);
    // The command this came from, small and under the field. It used to *be*
    // the title — a box headed `:grep` says which command you are in and not
    // one word about what it will do. The sentence goes on top now and the
    // name stays here, because the name is still worth learning.
    if (opts.note) {
        const note = document.createElement('div');
        note.className = 'note';
        note.textContent = opts.note;
        body.append(note);
    }
    el.ask.hidden = false;
    input.focus();
    // The stem, not the suffix: renaming is nearly always about the name and
    // almost never about the `.txt`.
    const dot = opts.secret ? -1 : initial.lastIndexOf('.');
    if (dot > 0) input.setSelectionRange(0, dot);
    else input.select();

    return new Promise((resolve) => {
        const done = (value) => {
            el.ask.hidden = true;
            body.textContent = '';
            el.ask.removeEventListener('click', onClick);
            document.removeEventListener('keydown', onKey, true);
            resolve(value);
        };
        const onClick = (e) => {
            const a = e.target.dataset && e.target.dataset.answer;
            if (a) done(a === 'yes' ? input.value : null);
        };
        const onKey = (e) => {
            if (e.key === 'Escape') { e.stopPropagation(); done(null); }
            else if (e.key === 'Enter') { e.stopPropagation(); done(input.value); }
            // ↑↓ **without closing the box**, when the caller has somewhere to
            // step. cian-tui's search popup does this (keys.rs:713): several
            // files share a substring, and walking them should not mean
            // closing the search and typing it again.
            else if (opts.onStep && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
                e.stopPropagation();
                e.preventDefault();
                opts.onStep(input.value, e.key === 'ArrowDown' ? 1 : -1);
            }
            else e.stopPropagation();
        };
        el.ask.addEventListener('click', onClick);
        document.addEventListener('keydown', onKey, true);
    });
}

/// Rename or delete one member of the open zip.
///
/// Rebuilt rather than patched, on the engine's side: a zip is a container
/// with an index, and rewriting one entry in place is how archives get
/// corrupted. Only zip — a tar would be rebuilt as a zip wearing a tar's
/// name, which is worse than refusing.
async function archiveEdit(row, kind) {
    const which = state.focus;
    // A row inside an archive carries `<archive>/<member>` as its path, so the
    // member is what follows the archive itself — the name alone would be
    // wrong for anything in a folder inside the zip.
    const pane = state[which];
    const member = row.path.startsWith(`${pane.archive}/`)
        ? row.path.slice(pane.archive.length + 1)
        : row.name;
    if (kind === 'delete') {
        if (!await confirm(tr(`Delete ${row.name} from the archive`, `${row.name} をアーカイブから削除`),
            tr('the archive is rebuilt without it', 'アーカイブを作り直します'))) {
            say(tr('stopped', 'やめました'));
            return;
        }
    }
    let to = '';
    if (kind === 'rename') {
        const next = await askFor(tr(`a new name for ${row.name}`, `${row.name} の新しい名前`), row.name);
        if (next === null || next === row.name) { say(tr('stopped', 'やめました')); return; }
        // The member keeps the folder it is in; only the last part is asked for.
        const head = member.includes('/') ? member.slice(0, member.lastIndexOf('/') + 1) : '';
        to = head + next;
    }
    say(tr('rebuilding the archive…', 'アーカイブを作り直しています…'));
    const r = await ask('archiveedit', { pane: which, member, to });
    if (!r) return;
    state[which] = r;
    draw(which);
    say(kind === 'delete'
        ? tr(`removed ${row.name} from the archive`, `${row.name} をアーカイブから削除しました`)
        : tr(`renamed to ${to.split('/').pop()}`, `${to.split('/').pop()} に変えました`));
}

/// Rename what the cursor is on.
async function rename() {
    const which = state.focus;
    const pane = state[which];
    const row = pane && pane.entries[pane.cursor];
    // Inside an archive, `r` renames the *member* — cian-tui's own key there
    // (arcview.rs). Browsing a zip should feel like browsing a folder, not
    // like looking at one through glass.
    if (pane && pane.archive && row && !row.parent) { await archiveEdit(row, 'rename'); return; }
    if (!row || row.parent) {
        say(tr('nothing to work on', '対象がありません'));
        return;
    }
    const name = await askFor(tr(`a new name for ${row.name}`, `${row.name} の新しい名前`), row.name);
    if (name === null || name === row.name) {
        say(tr('stopped', 'やめました'));
        return;
    }
    const next = await ask('rename', { pane: which, name });
    if (!next) return;
    state[which] = next;
    draw(which);
    say(`${row.name} → ${name}`);
}

/// A new file, or a new directory.
async function create(dir) {
    const which = state.focus;
    const name = await askFor(dir ? tr('name for the new folder', '新しいディレクトリの名前') : tr('name for the new file', '新しいファイルの名前'));
    if (name === null || !name.trim()) {
        say(tr('stopped', 'やめました'));
        return;
    }
    const next = await ask('create', { pane: which, name, dir });
    if (!next) return;
    state[which] = next;
    draw(which);
    say(tr(`created ${name}`, `${name} を作りました`));
}

/// One step back, whatever it was.
async function undo() {
    const r = await ask('undo', {});
    if (!r) return;
    state.left = r.left;
    state.right = r.right;
    draw('left'); draw('right');
    say(r.said);
}

/// Show or hide the dotfiles.
async function toggleHidden() {
    const which = state.focus;
    const r = await ask('hidden', { pane: which });
    if (!r) return;
    state[which] = r.pane;
    draw(which);
    if (menu.spec === TOGGLES) drawMenu();
    say(r.showing ? tr('dotfiles shown', '隠しファイルを表示') : tr('dotfiles hidden', '隠しファイルを非表示'));
}

/// `,` shows the four keys and lets you choose — it does not walk them.
///
/// It had walked them, which is a different thing: the terminal build opens a
/// picker on whichever key is in force, with n/s/d/e as direct picks, and
/// choosing the key already in force flips the direction. Walking meant `,`
/// took two presses to leave `name`, because the first one only reversed it.
// The words are cian-tui's `sort_label()` — the same four the column headings
// use, in both builds. This list said 日付 while the heading above it said 日時.
/// The four sort keys, their names, and the letter each answers to.
///
/// **A function, not a constant.** `tr()` returns a string, so a `const`
/// holding one holds whichever language was on when the file loaded — and
/// nothing short of a reload ever changes it again. Switching to English left
/// 名前 / サイズ / 日時 in the sort menu, メモ帳 in the toggles and
/// クラシック / アイコン in the view row: six frozen words in a window that
/// was otherwise entirely translated. Everything made of words has to be
/// *asked* for at the moment it is drawn.
function sorts() {
    return [['name', tr("Name", '名前'), 'n'], ['size', tr("Size", 'サイズ'), 's'],
            ['date', tr("Date", '日時'), 'd'], ['ext', tr('Extension', '拡張子'), 'e']];
}
async function applySort(key, reverse) {
    const which = state.focus;
    const r = await ask('sort', { pane: which, key, ...(reverse === undefined ? {} : { reverse }) });
    if (!r) return;
    state[which] = r.pane;
    draw(which);
    // `r.by` is the engine's wire name (`date`), not a label: cian-tui says
    // 並び: 日時 ▲ and so does this.
    const word = (sorts().find(([k]) => k === key) || [, r.by])[1];
    say(tr(`sort: ${word} ${r.reverse ? '▼' : '▲'}`, `並び: ${word} ${r.reverse ? '▼' : '▲'}`));
}

/// `/` narrows what is here. A second `/`, with nothing typed yet, looks
/// underneath instead — one slash for this listing, two for the tree. The
/// terminal build settled on that and it reads itself.
/// The prompt row at the foot, and which of the three things is typing into it.
///
/// `/` narrows the listing, `//` searches the tree below it, `:` takes a
/// command — three different questions, one place to type them, which is
/// where cian-tui puts all three (its prompt line, above the hints). The
/// command line used to raise a modal sheet in the middle of the window and
/// the finder a full-screen scrim over the very listing it was searching.
///
/// The colour says which: green for the two that search, purple for the one
/// that runs — cian-tui's own, because they take the same letters and the
/// only thing telling them apart is the frame.
const filter = { on: false, mode: null, resolve: null };

const PROMPT_SIGN = { filter: '/', find: '//', cmd: ':' };

function openPrompt(mode, seed = '', note = '') {
    filter.on = true;
    filter.mode = mode;
    el.fbar.dataset.mode = mode === 'cmd' ? 'cmd' : 'search';
    el.fSign.textContent = PROMPT_SIGN[mode];
    el.fbar.hidden = false;
    el.fInput.value = seed;
    el.fCount.textContent = note;
    el.fInput.focus();
    el.fInput.select();
    // The badge and the frame both come from `drawStatus`, and neither was
    // called when a prompt opened — so the mode's colour only arrived on the
    // next thing that happened to `say()` something, and often never.
    drawStatus();
    drawHints();
}

function closePrompt() {
    filter.on = false;
    filter.mode = null;
    el.fbar.hidden = true;
    el.fInput.blur();
    drawStatus();
    drawHints();
}

function startFilter() {
    // Seeded with what is already narrowing this pane, as the terminal build
    // seeds its box — reopening the filter to adjust it should not clear it.
    openPrompt('filter', state[state.focus]?.filter ?? '');
}

function endFilter(keep) {
    closePrompt();
    if (!keep) applyFilter('');
}

async function applyFilter(text) {
    const which = state.focus;
    const next = await ask('filter', { pane: which, text });
    if (!next) return;
    state[which] = next;
    draw(which);
    const n = next.entries.length;
    el.fCount.textContent = tr(`${n} items`, `${n} 件`);
    say(text ? tr(`narrowed: ${text} — ${n}`, `絞り込み: ${text} — ${n} 件`) : tr(`${n} items`, `${n} 件`));
}

/// The file finder: `//` opens it, typing narrows it, Enter goes there.
///
/// Ranking is the engine's — one fuzzy matcher rather than two that would
/// drift — and the round trip costs less than the ranking, because the engine
/// is a pipe away and not a network away.
const finder = { open: false, rows: [], at: 0, walking: false };

async function openFinder() {
    const which = state.focus;
    finder.open = true;
    finder.rows = [];
    finder.at = 0;
    finder.walking = true;
    el.find.hidden = false;
    el.findFoot.textContent = tr('looking…', '探しています…');
    el.findHits.replaceChildren();
    // Typed at the foot like everything else; the sheet above holds the hits.
    openPrompt('find');
    // Asked for before the walk has found anything, on purpose: the picker is
    // usable from the first keystroke and the tree arrives underneath it.
    await ask('find', { pane: which });
    rankNow();
}

function closeFinder() {
    finder.open = false;
    el.find.hidden = true;
    if (filter.mode === 'find') closePrompt();
}

async function rankNow() {
    if (!finder.open) return;
    const r = await ask('rank', { query: el.fInput.value, limit: 200 });
    if (!r || !finder.open) return;
    finder.rows = r.rows;
    finder.at = Math.min(finder.at, Math.max(0, r.rows.length - 1));
    drawHits(r.of);
}

function drawHits(of) {
    const frag = document.createDocumentFragment();
    finder.rows.forEach((row, i) => {
        const div = document.createElement('div');
        div.className = 'hit' + (row.is_dir ? ' d' : '') + (i === finder.at ? ' on' : '');
        const p = document.createElement('span');
        p.className = 'p';
        p.textContent = row.rel;
        div.append(p);
        div.addEventListener('mousedown', () => { finder.at = i; goToHit(); });
        frag.append(div);
    });
    el.findHits.replaceChildren(frag);
    const on = el.findHits.children[finder.at];
    if (on) on.scrollIntoView({ block: 'nearest' });
    el.findFoot.textContent = finder.walking
        ? tr(`${finder.rows.length} / ${of} (still looking)`, `${finder.rows.length} / ${of} 件（まだ探しています）`)
        : tr(`${finder.rows.length} / ${of}`, `${finder.rows.length} / ${of} 件`);
}

async function goToHit() {
    const row = finder.rows[finder.at];
    if (!row) return;
    const which = state.focus;
    closeFinder();
    const next = await ask('reveal', { pane: which, path: row.path });
    if (!next) return;
    state[which] = next;
    draw(which);
    say(row.rel);
}

/// The looks, in the order `T` walks them.
///
/// 白磁 leads because it is the default, and the default is chosen for the
/// person opening this for the first time rather than for the person who
/// built it — the same reasoning that made notepad the default grammar.
/// Taketan's own is solarized-light, one press away.
/// The window's own three, hand-made for this window: 白磁 is the default,
/// 陰翳 its dark counterpart, 端末譲り the one that looks like the terminal
/// build. The eighteen named palettes cian-tui ships arrive beside them at
/// startup, from cian-core's table — one list, one key, and a theme chosen
/// here is the theme the terminal opens with.
const LOOKS = [
    ['', '白磁'],
    ['inei', '陰翳'],
    ['terminal', '端末譲り'],
];

/// The palettes from cian-core, once they have arrived.
const palettes = new Map();

/// A spec becomes CSS custom properties.
///
/// The eleven the window uses are derived from the seventeen a palette
/// publishes — and the arithmetic that had to match the terminal build's
/// (which ink reads on the accent, how far to pull a colour toward the page)
/// is done in the engine, so there is one answer rather than two.
/// `a` blended toward `b` by `t` (0 = all `a`, 1 = all `b`). The window's own
/// small piece of colour arithmetic — everything else comes from the engine,
/// which is where cian-tui's `toward` lives.
function mix(a, b, t) {
    const rgb = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
    const [ar, ag, ab] = rgb(a);
    const [br, bg, bb] = rgb(b);
    const one = (x, y) => Math.round(x + (y - x) * t).toString(16).padStart(2, '0');
    return `#${one(ar, br)}${one(ag, bg)}${one(ab, bb)}`;
}

/// A palette spec becomes the window's custom properties.
function paletteVars(t) {
    return {
        '--bg': t.bg,
        '--pane': t.bg,
        '--pane-off': t.popup,
        '--line': t.border,
        '--text': t.fg,
        '--dim': t.dim,
        '--dir': t.blue,
        '--accent': t.accent,
        '--accent-dim': t.accent_dim,
        '--on-accent': t.on_accent,
        '--mark': t.mark,
        // The file-kind colours, mapped as the terminal build maps its
        // FilePalette from the same Spec (theme.rs from_spec): code=yellow,
        // config=cyan, document=doc, image=magenta, media=cyan, archive=red,
        // executable=green. Without these the 白磁 quiet tones stayed put
        // under every one of the eighteen palettes.
        '--k-code': t.yellow,
        '--k-config': t.cyan,
        '--k-doc': t.doc,
        '--k-image': t.magenta,
        '--k-media': t.cyan,
        '--k-archive': t.red,
        '--k-exec': t.green,
        // Derived rather than sent: `sel` is the terminal build's selection
        // colour and these are two steps of it toward the page, which is the
        // same arithmetic `accent_dim` gets on the engine side.
        '--sel-strong': mix(t.sel, t.bg, 0.35),
        '--row-hover': mix(t.fg, t.bg, 0.94),
    };
}

function paintPalette(t) {
    const r = document.documentElement.style;
    for (const [k, v] of Object.entries(paletteVars(t))) r.setProperty(k, v);
    document.documentElement.dataset.dark = t.light ? '' : '1';
}

/// The variables a palette sets. One list, used to paint the whole window, to
/// paint a single pane, and to wipe either — three copies of it would drift
/// the first time a colour was added.
const PALETTE_VARS = ['--bg', '--pane', '--pane-off', '--line', '--text', '--dim',
    '--dir', '--accent', '--accent-dim', '--on-accent', '--mark',
    '--k-code', '--k-config', '--k-doc', '--k-image', '--k-media',
    '--k-archive', '--k-exec', '--sel-strong', '--row-hover'];

/// What one pane is wearing, over whatever the window is wearing.
///
/// cian-tui has both of these and keeps both for the session only
/// (`App.pane_bg`, `App.pane_theme`): a ground you give the left listing is
/// for the work in front of you, not a preference to be remembered. The point
/// is telling two panes apart when both are showing a directory called `src`.
const paneSkin = { left: { ground: null, theme: null }, right: { ground: null, theme: null } };

/// The fourteen pane grounds, from the engine (cian-core's own table).
let grounds = [];

function paneEl(which) {
    return el.panes.querySelector(`.pane[data-pane="${which}"]`);
}

/// Put a pane's own ground and palette on the pane element itself.
///
/// A custom property set on the element wins for that subtree, so this is the
/// whole mechanism — no second stylesheet, and the rest of the window keeps
/// the palette it had. `--panes-pct` and `--main-pct` live on `:root`, so
/// clearing the element's inline properties cannot disturb the layout.
function paintPane(which) {
    const node = paneEl(which);
    if (!node) return;
    for (const k of PALETTE_VARS) node.style.removeProperty(k);
    const skin = paneSkin[which];
    if (skin.theme) {
        const t = palettes.get(skin.theme);
        if (t) {
            const vars = paletteVars(t);
            for (const [k, v] of Object.entries(vars)) node.style.setProperty(k, v);
        }
    }
    if (skin.ground) {
        // The active pane shows `--pane` and the other `--pane-off`; a chosen
        // ground is the pane's ground either way, a shade quieter when the
        // keys are elsewhere.
        node.style.setProperty('--pane', skin.ground);
        node.style.setProperty('--pane-off', mix(skin.ground, '#000000', 0.3));
    }
}

function clearPalette() {
    const r = document.documentElement.style;
    for (const k of PALETTE_VARS) r.removeProperty(k);
    delete document.documentElement.dataset.dark;
}

/// How the listing is laid out. **窓版だけの話** ── 端末版は 2026-09-06 に
/// `:view` の引数ごと落とした（旗を立てるだけで誰も読まなかった）。
/// アイコンモードは 2026-09-02 に入口を閉じ、描く側も 2026-09-06 に消した。
// モードの順番はここ一つ。`モード ▸` も T トグルの巡回も `表示 ▸` も
// この順に従う（クラシック → 詳細一覧）。**amber モードは 2026-09-06 に
// 出た** ── ノートのアプリは `~/workspace/amber` で、cian は2画面ファイラ。
const VIEWS = ['classic', 'details'];
function viewName(mode) {
    // 「モード」と呼ぶと決めたので、名前にも付ける ── メニューが
    // 「モード ▸ クラシック」なら、それは「クラシックモード」のこと。
    return {
        classic: tr("classic mode", 'クラシックモード'),
        details: tr("details mode", '詳細一覧モード'),
    }[mode];
}
/// The one that takes the whole window: the Explorer arrangement, where the
/// listing is the thing you are looking at. Classic keeps the two panes,
/// which is what cian is for.
const ONE_PANE = ['details'];
let viewMode = 'classic';

function setView(mode, remember = true) {
    if (!VIEWS.includes(mode)) { say(`${mode}? — :view classic | details`, true); return; }
    viewMode = mode;
    el.panes.classList.toggle('one', ONE_PANE.includes(mode));
    // `draw` already paints `active` on the focused pane, which is what
    // decides who is at the front here — so the two views need no second
    // notion of focus.
    draw('left');
    draw('right');
    if (remember) ask('remember', { key: 'gui_view', value: mode });
}

/// 「その面はいま無い」を言う3つ ── 同じ行を6回・4回・4回書いていた。
///
/// **括ったのは文言のためではなく、答えが1つであるため。** 同じ問いに同じ
/// 言葉で答える場所が10か所あると、いつか1か所だけ別の言い方になる ──
/// `audit.py` の②が数え続けていたのはそれ。
///
/// 呼び方は `if (!needViewer()) return;`。真偽を返すのは、`return null` の
/// 側と `return` の側の両方があるから（中で return するとその違いが消える）。
function needViewer() {
    if (viewer.on && viewer.ed) return true;
    say(tr('open a file first', '先にファイルを開いてください'), true);
    return false;
}

function needShell() {
    if (term.on) return true;
    say(tr('no shell is open', 'シェルが開いていません'), true);
    return false;
}

function needTargets(what) {
    if (what.length) return true;
    say(tr('nothing to work on', '対象がありません'), true);
    return false;
}

/// いまどのシェルタブに居るか。タブを動かす4つが同じ行を書いていた。
///
/// **この関数自身が5つ目になっていた。** 括るときに同じ文字列を一括置換して、
/// 中身まで `sayShellTab()` に置き換わり、呼ぶと自分を呼び続けた ──
/// `node --check` は通り、audit は「きれいです」になり、台帳も通った。
/// `node gui/drive.js` の例外欄だけが「Maximum call stack size exceeded」と
/// 言った。**括ったあとは動かす。**
function sayShellTab() {
    say(tr(`shell ${term.tab + 1} / ${term.tabs}`, `シェル ${term.tab + 1} / ${term.tabs}`));
}

/// The lower-cased extension, or ''. Four functions asked this question with
/// the same regex on four lines — the audit's "same line four times" — and
/// four copies of one rule is how one of them starts answering differently.
function extOf(row) {
    return (row.name.match(/\.([a-z0-9]+)$/i) || [, ''])[1].toLowerCase();
}

/// What kind of thing this is, said in a word. Explorer's "種類" column —
/// which is more useful than the extension it is derived from, because the
/// extension is already right there in the name.
function kindOf(row) {
    if (row.parent) return '';
    if (row.is_dir) return tr('Folder', 'ディレクトリ');
    const ext = extOf(row);
    if (!ext) return tr('File', 'ファイル');
    const known = {
        md: 'Markdown', txt: tr('Text', 'テキスト'), log: tr('Log', 'ログ'), json: 'JSON', toml: 'TOML',
        yml: 'YAML', yaml: 'YAML', csv: 'CSV', tsv: 'TSV', xml: 'XML', html: 'HTML',
        css: 'CSS', js: 'JavaScript', ts: 'TypeScript', rs: 'Rust', py: 'Python',
        go: 'Go', c: 'C', h: tr('C header', 'C ヘッダ'), cpp: 'C++', java: 'Java', lua: 'Lua',
        sh: tr('shell', 'シェル'), bat: tr('Batch', 'バッチ'), ps1: 'PowerShell', sql: 'SQL',
        pdf: 'PDF', zip: 'ZIP', tar: 'TAR', gz: 'GZIP', '7z': '7-Zip', rar: 'RAR',
        png: tr('PNG image', 'PNG 画像'), jpg: tr('JPEG image', 'JPEG 画像'), jpeg: tr('JPEG image', 'JPEG 画像'), gif: tr('GIF image', 'GIF 画像'),
        webp: tr('WebP image', 'WebP 画像'), svg: tr('SVG image', 'SVG 画像'), bmp: tr('BMP image', 'BMP 画像'), ico: tr("icons", 'アイコン'),
        mp3: tr('Audio', '音声'), wav: tr('Audio', '音声'), flac: tr('Audio', '音声'), mp4: tr('Video', '動画'), mov: tr('Video', '動画'), mkv: tr('Video', '動画'),
        xlsx: 'Excel', xls: 'Excel', docx: 'Word', doc: 'Word', pptx: 'PowerPoint',
        ttf: tr('Font', 'フォント'), otf: tr('Font', 'フォント'), woff2: tr('Font', 'フォント'),
        exe: tr('Application', 'アプリケーション'), dll: tr('Library', 'ライブラリ'), so: tr('Library', 'ライブラリ'), dylib: tr('Library', 'ライブラリ'),
    };
    return known[ext] || tr(`${ext.toUpperCase()} file`, `${ext.toUpperCase()} ファイル`);
}

/// What kind of file this is, as a row class — the terminal build's
/// `kind_for` (render.rs), extension for extension. The class picks the
/// name's colour from the palette; a dotfile recedes to muted.
function kindClassOf(row) {
    if (row.parent || row.is_dir) return '';
    if (row.name.startsWith('.')) return ' k-muted';
    const ext = extOf(row);
    if (/^(rs|py|js|mjs|cjs|ts|tsx|jsx|go|c|h|cpp|cc|cxx|hpp|java|rb|php|lua|swift|kt|kts|vue|svelte|html|htm|css|scss|sass|less)$/.test(ext)) return ' k-code';
    if (/^(toml|ini|conf|cfg|yaml|yml|json|jsonc|xml|env)$/.test(ext)) return ' k-config';
    if (/^(md|markdown|txt|log|pdf|docx?|xlsx?|pptx?|rtf|csv|tsv)$/.test(ext)) return ' k-doc';
    if (/^(png|jpe?g|gif|bmp|svg|webp|ico|tiff?)$/.test(ext)) return ' k-image';
    if (/^(mp3|wav|flac|ogg|m4a|aac|mp4|mov|mkv|avi|webm|wmv)$/.test(ext)) return ' k-media';
    if (/^(zip|tar|gz|7z|rar|bz2|xz|zst|tgz)$/.test(ext)) return ' k-archive';
    if (/^(exe|msi|bat|cmd|ps1|sh|bash|zsh|fish|app|dll|so|dylib)$/.test(ext)) return ' k-exec';
    return '';
}

/// The row's small leading icon — the terminal build's `icon_for`
/// (render.rs), codepoint for codepoint, drawn from the bundled Nerd font.
/// Written as escapes and copied from that table, not from memory: a glyph
/// remembered wrong renders as some other picture, silently. The emoji set
/// below stays for the icon tiles, where a big picture is the point.
/// The one-character git state of a row, from the engine.
///
/// cian-tui draws this column in every listing inside a repository (`●` staged,
/// `✚` changed, `?` untracked, `‼` conflict, `~` a directory with changes under
/// it) and the window drew only the branch chip — so "which of these did I
/// touch" meant running `git status` in the shell beside a file manager that
/// already knew.
///
/// The `~` on a directory is the half that earns the column: it says where to
/// go next without walking in.
function gitBadge(which, row) {
    const b = document.createElement('span');
    b.className = 'git';
    const m = repo[which]?.v?.marks?.[row.path];
    if (m && !row.parent) {
        b.textContent = m.badge;
        b.dataset.git = m.kind;
        b.title = {
            staged: tr('staged', 'ステージ済み'), modified: tr('changed', '変更あり'), untracked: tr('untracked', '未追跡'),
            conflict: tr('conflict', '衝突'), dirdirty: tr('something below here has changed', 'この下に変更があります'),
        }[m.kind] || m.kind;
    }
    return b;
}

/// The desktop's own icon for a row, when it has one.
///
/// A terminal can only draw a glyph from a font, which is why cian-tui picks
/// from a Nerd Font table and why the window inherited that table. The OS
/// already has a picture for every registered type — the real Excel icon for
/// an .xlsx, whatever app claims a .psd — and it is one call away here.
///
/// Cached by **extension**, not by path: a folder of two thousand files is a
/// handful of calls, and the icon of a `.rs` is the icon of every `.rs`. The
/// glyph is drawn first and replaced when the picture arrives, so a slow
/// answer costs nothing and a missing one costs nothing either.
const nativeIcons = new Map();

function nativeIconFor(row, into) {
    if (!window.cian.fileIcon || row.parent) return;
    // Folders too. They were excluded here, so the listing drew the desktop's
    // real icon for every file and a Nerd Font glyph for every directory —
    // which is what "these are not Windows' icons" was about: the one row
    // shape a person looks at first was the one row shape still coming from
    // the font. Every plain folder shares Explorer's folder icon, so they
    // share one cache key and cost one call for the whole tree.
    const key = row.is_dir ? '\u0000<dir>' : (extOf(row) || `\u0000${row.name.toLowerCase()}`);
    const known = nativeIcons.get(key);
    if (known === null) return;
    if (typeof known === 'string') { paintIcon(into, known); return; }
    if (known instanceof Promise) { known.then((url) => url && paintIcon(into, url)); return; }
    const p = window.cian.fileIcon(row.path).then((url) => {
        nativeIcons.set(key, url || null);
        return url;
    }).catch(() => { nativeIcons.set(key, null); return null; });
    nativeIcons.set(key, p);
    p.then((url) => url && paintIcon(into, url));
}

function paintIcon(into, url) {
    if (!into.isConnected) return;
    const img = document.createElement('img');
    img.className = 'nativeicon';
    img.src = url;
    into.replaceChildren(img);
}

function iconFor(row) {
    if (row.parent) return '\u{f062}';
    if (row.is_dir) {
        return {
            '.git': '\u{e702}', '.github': '\u{f408}', node_modules: '\u{e5fa}',
            src: '\u{f121}', tests: '\u{f0c3}', test: '\u{f0c3}',
            docs: '\u{f02d}', doc: '\u{f02d}',
            target: '\u{f1c6}', build: '\u{f1c6}', dist: '\u{f1c6}', out: '\u{f1c6}',
            '.vscode': '\u{e7c5}', '.idea': '\u{e7c5}',
        }[row.name] ?? '\u{f07b}';
    }
    const whole = {
        'cargo.toml': '\u{e7a8}', 'cargo.lock': '\u{e7a8}',
        dockerfile: '\u{f308}', '.dockerignore': '\u{f308}',
        makefile: '\u{e779}', 'readme.md': '\u{f48a}', readme: '\u{f48a}',
        license: '\u{f02d}', 'license.md': '\u{f02d}',
        '.gitignore': '\u{f1d3}', '.gitattributes': '\u{f1d3}', '.gitmodules': '\u{f1d3}',
        '.env': '\u{f462}', '.env.local': '\u{f462}',
        'package.json': '\u{e60b}', 'package-lock.json': '\u{e60b}', 'yarn.lock': '\u{e60b}',
    }[row.name.toLowerCase()];
    if (whole) return whole;
    const ext = extOf(row);
    const map = {
        rs: '\u{e7a8}', py: '\u{e73c}',
        js: '\u{f2ee}', mjs: '\u{f2ee}', cjs: '\u{f2ee}',
        ts: '\u{e628}', tsx: '\u{e628}', jsx: '\u{e628}', go: '\u{e627}',
        c: '\u{e61e}', h: '\u{e61e}',
        cpp: '\u{e61d}', cc: '\u{e61d}', cxx: '\u{e61d}', hpp: '\u{e61d}',
        java: '\u{e738}', rb: '\u{e21e}', php: '\u{e608}', lua: '\u{e620}',
        swift: '\u{e755}', kt: '\u{e634}', kts: '\u{e634}',
        md: '\u{f48a}', markdown: '\u{f48a}',
        json: '\u{e60b}', jsonc: '\u{e60b}', yaml: '\u{f481}', yml: '\u{f481}',
        toml: '\u{f013}', ini: '\u{f013}', conf: '\u{f013}', cfg: '\u{f013}',
        xml: '\u{f72d}', html: '\u{f13b}', htm: '\u{f13b}',
        css: '\u{f13c}', scss: '\u{f13c}', sass: '\u{f13c}', less: '\u{f13c}',
        vue: '\u{fd42}', svelte: '\u{e697}',
        sh: '\u{f489}', bash: '\u{f489}', zsh: '\u{f489}', fish: '\u{f489}',
        png: '\u{f1c5}', jpg: '\u{f1c5}', jpeg: '\u{f1c5}', gif: '\u{f1c5}',
        bmp: '\u{f1c5}', svg: '\u{f1c5}', webp: '\u{f1c5}', ico: '\u{f1c5}',
        tif: '\u{f1c5}', tiff: '\u{f1c5}',
        mp3: '\u{f001}', wav: '\u{f001}', flac: '\u{f001}', ogg: '\u{f001}',
        m4a: '\u{f001}', aac: '\u{f001}',
        mp4: '\u{f03d}', mov: '\u{f03d}', mkv: '\u{f03d}', avi: '\u{f03d}',
        webm: '\u{f03d}', wmv: '\u{f03d}',
        pdf: '\u{f1c1}',
        zip: '\u{f1c6}', tar: '\u{f1c6}', gz: '\u{f1c6}', '7z': '\u{f1c6}',
        rar: '\u{f1c6}', bz2: '\u{f1c6}', xz: '\u{f1c6}',
        txt: '\u{f0f6}', log: '\u{f0f6}',
        exe: '\u{f013}', dll: '\u{f013}', so: '\u{f013}', dylib: '\u{f013}',
    };
    return map[ext] ?? '\u{f15c}';
}

/// What an icon tile shows for a file. Deliberately coarse: a dozen kinds a
/// glance can tell apart, not a catalogue. Anything unknown is a plain page,
/// which is honest — the name below it is the real information.
function glyphFor(row) {
    if (row.parent) return '↩';
    if (row.is_dir) return '📁';
    const ext = extOf(row);
    if (/^(png|jpe?g|gif|webp|bmp|svg|avif|ico)$/.test(ext)) return '🖼️';
    if (/^(zip|tar|gz|tgz|7z|rar|jar)$/.test(ext)) return '📦';
    if (/^(pdf)$/.test(ext)) return '📕';
    if (/^(md|txt|log)$/.test(ext)) return '📝';
    if (/^(rs|js|ts|py|lua|c|h|cpp|go|java|sh|bat|ps1|toml|ya?ml|json|html|css)$/.test(ext)) return '📜';
    if (/^(xlsx?|csv)$/.test(ext)) return '📊';
    if (/^(docx?|pptx?)$/.test(ext)) return '📄';
    if (/^(mp[34]|wav|mov|mkv|flac|m4a)$/.test(ext)) return '🎞️';
    return '📄';
}

/// A modified time, the way a listing shows one.
///
/// This year gets `MM-DD HH:MM`; anything older gets the year instead of the
/// clock — `ls -l` and Finder both do this, and for good reason. The year is
/// the same four digits on almost every row, so printing it everywhere spends
/// the width of the widest column saying the least. What the eye wants from
/// this column is "recently, or long ago".
function when(row) {
    if (!row.modified) return '';
    const d = new Date(row.modified * 1000);
    const p = (n) => String(n).padStart(2, '0');
    const md = `${p(d.getMonth() + 1)}-${p(d.getDate())}`;
    return d.getFullYear() === new Date().getFullYear()
        ? `${md} ${p(d.getHours())}:${p(d.getMinutes())}`
        : `${d.getFullYear()}-${md}`;
}

/// Which look is showing, and it *is* written down now.
///
/// The question was open for months because the answer looked expensive:
/// reading `init.lua` needs Lua, which needs a C compiler. It turned out the
/// terminal build already carries one — mlua, vendored, built green on
/// Windows every release — so the property being protected had been spent long
/// ago. The engine reads and writes the terminal build's own state file, so a
/// look chosen here is the look `cian` opens with, and the other way round.
let look = 0;

/// Forget any ground or palette a single pane was wearing.
///
/// A pane's own skin is set as inline custom properties on the pane element,
/// which beat `:root` for that subtree — that is the whole mechanism, and it
/// is also why choosing a *window* theme afterwards appeared to do nothing to
/// that pane. Both are "make this look like X"; the later one is the answer.
/// (Reported after choosing a pane theme and then a whole-window one: the
/// pane kept the first choice.)
function clearPaneSkins() {
    for (const which of ['left', 'right']) {
        paneSkin[which].ground = null;
        paneSkin[which].theme = null;
        paintPane(which);
    }
}

/// Tell the frame which way round we are.
///
/// **The title bar stayed light in every dark palette.** It is drawn by the
/// OS, not by the page, so no amount of CSS reaches it — Windows reads what
/// the *application* says its theme is. The window disagreed with itself
/// along its own top edge.
///
/// Read from the ground that is actually painted rather than from the name of
/// the look: there are eighteen palettes and `is_light` in cian-core judges
/// them by luminance for the same reason — 宵闇 and 墨 are not dark because of
/// how they are spelled. One door, called from both places that can change
/// the ground.
function tellFrame() {
    if (!window.cian || !window.cian.frame) return;
    const raw = getComputedStyle(document.documentElement).getPropertyValue('--bg').trim();
    const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(raw);
    if (!m) return;
    const [r, g, b] = [1, 2, 3].map((i) => parseInt(m[i], 16));
    // Rec. 601, the same arithmetic `theme.rs` uses, so the two builds never
    // disagree about which side of the line a palette is on.
    return window.cian.frame((299 * r + 587 * g + 114 * b) / 1000 > 128);
}

function setLook(i, remember = true) {
    look = (i + LOOKS.length) % LOOKS.length;
    clearPaneSkins();
    const [value] = LOOKS[look];
    // At the look's own base size, follow the new look's base — 端末譲り is
    // 14px on purpose. An explicit Ctrl+= choice survives the switch.
    const wasBase = FONT.at === baseFont();
    clearPalette();
    if (value) document.documentElement.dataset.look = value;
    else delete document.documentElement.dataset.look;
    if (wasBase) setFont(baseFont(), false);
    if (viewer.ed) viewer.ed.updateOptions({ theme: editorTheme() });
    tellFrame();
    if (remember) ask('remember', { key: 'gui_look', value: LOOKS[look][0] || 'hakuji' });
}

/// One of the eighteen. Same key, same list, same file the terminal build
/// reads its own choice out of.
function setPalette(name, remember = true) {
    const t = palettes.get(name);
    if (!t) { say(tr(`${name}? — :theme lists them`, `${name}? — :theme で一覧`), true); return; }
    clearPaneSkins();
    delete document.documentElement.dataset.look;
    paintPalette(t);
    palette = name;
    if (viewer.ed) viewer.ed.updateOptions({ theme: editorTheme() });
    tellFrame();
    if (remember) {
        ask('remember', { key: 'theme', value: name });
        // Choosing a palette *is* choosing the window's ground. Left in
        // place, a `gui_look` of 陰翳 from last month silently overrode this
        // palette — and the terminal build's — on every startup after.
        look = 0;
        ask('remember', { key: 'gui_look', value: 'hakuji' });
    }
}

/// Which named palette is on, or null when one of the window's own looks is.
let palette = null;

/// What the keys do here, right now.
///
/// **Taken from cian-tui's own hint table, translated key for key.** The
/// terminal build carries this bar and the window did not, which is most of
/// why the two felt like different programs: cian tells you what you can
/// press, continuously, and a window that stayed silent made you remember
/// instead. Ordered by how often each is reached for, so a narrow window
/// drops from the end.
function hintsNow() {
    // The chat is in front of the viewer when both are up, so it answers
    // first. A bar naming the editor's keys over a conversation would be
    // naming keys that do not fire.
    if (chat.on) {
        return [['Enter', tr('send', '送信')], ['Shift+Enter', tr('newline', '改行')],
                ['Ctrl+V', tr('paste', '貼り付け')], ['Esc', tr('close', '閉じる')]];
    }
    if (viewer.on) {
        // `STYLES[0]` is notepad and `STYLES[1]` is vim; this asked for 1 and
        // returned the notepad row. It had never been on screen to disagree
        // with — `drawHints()` was not called when a file opened — so an
        // inverted test sat here saying Shift+←→ 選択 under a --NORMAL-- line.
        if (style === 0) {
            // `メニュー — キー操作切替` as cian-tui names it here: from inside
            // notepad style `T` is a character, so this menu is the only way
            // back to the other grammar and the bar should say so.
            return [['Ctrl+S', tr('save', '保存')], ['Shift+←→', tr('select', '選択')], ['Ctrl+C / V', tr('copy / paste', 'コピー / 貼り付け')],
                ['Ctrl+F', tr('search', '検索')], ['Esc ×3', tr('close', '閉じる')], ['Shift+Enter', tr('menu — editor keys', 'メニュー — キー操作切替')]];
        }
        // `Shift+Enter` is on this row now. The menu is where the encoding,
        // blame, the preview and the external editor live, and in vim style
        // it was the one thing the bar never named — so "there is no way to
        // change the encoding in the editor" was a fair reading of the
        // screen, though `:enc` and the menu both had it all along.
        return [['Ctrl+S', tr('save', '保存')], ['Esc', tr('leave the editor', '編集終了')], ['/', tr('search', '検索')], ['i', tr('edit', '編集')],
            ['v', tr('select', '選択')], ['y', tr('copy', 'コピー')], ['d c y', tr('+ motion', '＋モーション')], [':q', tr('close', '閉じる')],
            ['Shift+Enter', tr('menu — encoding, blame, preview', 'メニュー — 文字コード・blame・プレビュー')],
            ['?', tr('keys', 'キー一覧')]];
    }
    if (term.on && term.focused) {
        // Dynamic, as the terminal build's shell hints are: ^C only while a
        // drag selection exists (otherwise it names the gesture that makes
        // one), the pane keys only while there is a second pane to go to.
        const sel = window.getSelection();
        const hasSel = sel && !sel.isCollapsed && el.sPanes.contains(sel.anchorNode);
        const split = el.sPanes.querySelectorAll('.sgrid').length > 1;
        return [['Esc', tr('files', 'ファイル')],
            hasSel ? ['Ctrl+C', tr('copy the selection', '選択をコピー')] : [tr('drag', 'ドラッグ選択'), tr('select = copy', '= コピー')],
            ['Ctrl+V', tr('paste', '貼り付け')],
            ...(split ? [['Shift+F1/F2', tr('prev/next pane', '前/次のペイン')]] : []),
            ['F9', tr('new tab', '新規タブ')], ['F10', tr('close tab', 'タブを閉じる')], ['Shift+F8', tr('v-split', '左右分割')],
            ['Shift+F9', tr('h-split', '上下分割')],
            ...(split ? [['Shift+F10', tr('close split', '分割を閉じる')]] : []),
            ['F12', tr('zoom', 'ズーム')], ['Shift+Enter', tr('menu', 'メニュー')]];
    }
    if (visual.on) {
        return [['j/k', tr('extend', '伸ばす')], ['a', tr('all', '全選択')], ['gg/G', tr('top/bottom', '先頭/末尾')],
            ['Enter', tr('confirm', '確定')], ['Esc', tr('cancel', '取消')]];
    }
    if (filter.on) {
        if (filter.mode === 'cmd') return [[tr('type', '打つ'), tr('command', 'コマンド')], ['Enter', tr('run', '実行')], ['Esc', tr('cancel', '取消')], ['C', tr('from a list', '一覧から選ぶ')]];
        if (filter.mode === 'find') return [[tr('type', '打つ'), tr('narrow', '絞込')], ['↑↓', tr('choose', '選ぶ')], ['Enter', tr('go there', 'そこへ')], ['Esc', tr('cancel', '取消')]];
        return [[tr('type', '打つ'), tr('narrow', '絞込')], ['↑↓', tr('cursor', 'カーソル')], ['Enter', tr('keep', '適用')], ['Esc', tr('clear', '解除')], ['/', tr('search below', 'この下を探す')]];
    }
    const pane = state[state.focus];
    if (pane && pane.archive) {
        return [['Enter/l', tr('in', '入る')], ['Bksp', tr('out', '戻る')], ['F3', tr('view member', 'メンバー閲覧')],
            ['Space', tr('mark', 'マーク')], ['c', tr('extract →', '展開 →')], ['?', tr('help', 'ヘルプ')]];
    }
    if (pane && pane.remote) {
        return [['Esc', tr('disconnect', '切断')], ['Space', tr('mark', 'マーク')], ['c', tr('transfer', '転送')], ['r', tr('rename', 'リネーム')],
            ['d', tr('delete', '削除')], ['Enter', tr('open', '開く')], ['?', tr('help', 'ヘルプ')]];
    }
    if (pane && pane.flat) {
        return [['b/Esc', tr('out', '戻る')], ['Space', tr('mark', 'マーク')], ['/', tr('narrow', '絞込')],
            ['Enter', tr('open', '開く')], ['F3', tr('view', '閲覧')], ['?', tr('help', 'ヘルプ')]];
    }
    return [['←→', tr('panes', 'ペイン')], ['Shift+J', tr('shell', 'シェル')], ['Space', tr('mark', 'マーク')], ['/', tr('narrow', '絞込')],
        [',', tr('sort', '並替')], ['Shift+F', tr('search', '検索')], ['Ctrl+F', 'grep'], ['b', tr('branch', 'ブランチ')],
        ['F3', tr('view', '閲覧')], ['M', tr('menu', 'メニュー')], ['F1/F2', tr('prev/next tab', '前/次タブ')],
        ['F9', tr('new tab', '新規タブ')], ['F10', tr('close tab', 'タブを閉じる')], ['=', tr('diff', '差分')], ['?', tr('help', 'ヘルプ')]];
}

let hintsOn = true;

/// Tell the layout how tall the two fixed foot bars actually are.
///
/// Measured, not declared: their text is set at a size the person changes
/// with Ctrl+=, so a number in the stylesheet would be right until the first
/// press. A listing whose last row sits under the status bar is a row you
/// cannot see or reach.
function measureFoot() {
    const r = document.documentElement.style;
    r.setProperty('--status-h', `${el.status.offsetHeight}px`);
    r.setProperty('--hints-h', `${hintsOn ? el.hints.offsetHeight : 0}px`);
}

function drawHints() {
    el.hints.hidden = !hintsOn;
    if (!hintsOn) return;
    el.hints.replaceChildren(...hintsNow().map(([k, what]) => {
        const s = document.createElement('span');
        const b = document.createElement('b');
        // 押す鍵は同じでも、手元のキーボードに書いてある文字は違う。
        b.textContent = keyLabel(k);
        s.append(b, document.createTextNode(what));
        return s;
    }));
    // A narrow window gives up hints from just before the end, so the last
    // one — `? ヘルプ`, the door to all the others — is never the one lost.
    // It used to clip from the right, which dropped exactly that one first.
    while (el.hints.scrollWidth > el.hints.clientWidth && el.hints.children.length > 2) {
        el.hints.children[el.hints.children.length - 2].remove();
    }
    measureFoot();
}

/// The switches, on `T` — the key the terminal build puts them on.
///
/// Not a key each. cian-tui gathers the live settings into one menu rather
/// than spending a letter on every one of them, and a front end that scattered
/// them would be a second set of habits to learn.
const TOGGLES = {
    key: 'T',
    foot: () => tr("\u2191\u2193 choose  Enter toggle  Esc close", '↑↓ 選ぶ  Enter 切替  Esc 閉じる'),
    stay: true,
    // The rows are cian-tui's `toggle_rows()` (toggles.rs:41), in its order and
    // with its words — including its ON / OFF, which used to be four different
    // pairs here (出す・表示・する…). The window's own three come after, so the
    // shared part of the list is the same list in both builds.
    rows: () => {
        const pane = state[state.focus];
        const onoff = (b) => (b ? 'ON' : 'OFF');
        return [
            // ── the window's own three ──
            //
            // Put where it can be found. A view you can only leave by knowing
            // the words `:view classic` is a view you are stuck in — and
            // icons is the one that hides the listing you would have read the
            // help from.
            {
                label: tr("Mode", 'モード'),
                value: viewName(viewMode),
                run: () => {
                    const next = VIEWS[(VIEWS.indexOf(viewMode) + 1) % VIEWS.length];
                    setView(next);
                    drawMenu();
                    say(tr(`listing: ${viewName(next)}`, `一覧: ${viewName(next)}`));
                },
            },
            {
                label: tr("Dotfiles", '隠しファイル'),
                value: onoff(pane && pane.hidden_shown),
                run: () => toggleHidden(),
            },
            {
                label: tr("Input sync (all shells)", '入力同期（全シェル）'),
                value: onoff(term.sync),
                run: async () => { await cmdSync(); drawMenu(); },
            },
            {
                label: tr("Task-done notification", '完了通知'),
                value: onoff(switches.notify),
                run: () => {
                    switches.notify = !switches.notify;
                    drawMenu();
                    say(switches.notify
                        ? tr(`task-done notification on — anything longer than ${Math.round(notifyAfterMs / 1000)}s`, `完了通知 ON — ${Math.round(notifyAfterMs / 1000)} 秒より長い処理が終わったら知らせます`)
                        : tr('task-done notification off', '完了通知 OFF'));
                },
            },
            {
                label: tr("Verify transfers", '転送後ベリファイ'),
                value: onoff(switches.verify),
                run: async () => {
                    const r = await ask('switches', { verify: !switches.verify });
                    if (!r) return;
                    switches.verify = r.verify;
                    drawMenu();
                    say(switches.verify
                        ? tr('verify transfers on — read back and compared after sending (twice the round trips)', '転送後ベリファイ ON — 送ったあと読み直して照合します（往復が倍になります）')
                        : tr('verify transfers off', '転送後ベリファイ OFF'));
                },
            },
            {
                label: tr("Cursor preview (shell panel)", 'カーソル追従プレビュー'),
                value: onoff(preview.on),
                run: () => { togglePreview(); drawMenu(); },
            },
            {
                // 印を先頭に置かない ── この行だけ左端がずれる（端末版の
                // `toggles.rs` に同じ註）。**同じ言葉であること**が parity の
                // 見ているところなので、二つを一緒に動かす。
                label: tr("Read \u2601 cloud-only files", 'クラウド上（☁）のファイルも読む'),
                value: onoff(switches.cloud),
                run: async () => {
                    const r = await ask('switches', { cloud: !switches.cloud });
                    if (!r) return;
                    switches.cloud = r.cloud;
                    drawMenu();
                    say(switches.cloud
                        ? tr('⚠ searches and checksums will now actually download cloud-only files', '⚠ 検索やチェックサムがクラウド上のファイルを実際に落とすようになります')
                        : tr('☁ cloud-only files are left alone', '☁ クラウド上のファイルは読みません'));
                },
            },
            {
                label: tr('Language', '言語'),
                value: lang === 'en' ? 'English' : '日本語',
                run: () => { setLang(lang === 'en' ? 'ja' : 'en'); drawMenu(); },
            },
            // Named by what it *is*, not by on/off — as cian-tui names it, and
            // for its reason: neither of the two is the absence of the other.
            {
                label: tr("Editor keys", 'エディタのキー操作'),
                value: styleName(style),
                run: () => { setStyle(style + 1); drawMenu(); say(tr(`editor: ${styleName(style)}`, `エディタ: ${styleName(style)}`)); },
            },
            {
                label: tr("Theme (whole app)", 'テーマ（全体）'),
                value: palette || LOOKS[look][1],
                // Opens the gallery rather than cycling: there are twenty-one
                // of them now, and stepping through twenty-one with one key is
                // not choosing, it is waiting.
                run: () => { closeMenu(); cmdTheme(); },
            },
            {
                label: tr("Key hints", 'キーヒント'),
                value: onoff(hintsOn),
                run: () => {
                    hintsOn = !hintsOn;
                    drawHints();
                    drawMenu();
                    ask('remember', { key: 'gui_hints', value: hintsOn ? '1' : '0' });
                },
            },
        ];
    },
};

const SORT_MENU = {
    key: ',',
    foot: () => tr("\u2191\u2193 choose  Enter apply  n s d e direct  Esc close", '↑↓ 選ぶ  Enter 決定  n s d e で直接  Esc 閉じる'),
    stay: false,
    at: () => sorts().findIndex(([k]) => k === (state[state.focus]?.sort_key ?? 'name')),
    rows: () => sorts().map(([k, label, letter]) => ({
        label,
        value: k === (state[state.focus]?.sort_key ?? 'name') ? '●' : letter,
        run: () => applySort(k),
    })),
    // The letters, so the picker is skippable once it is in the fingers —
    // the terminal build has the same four.
    letters: Object.fromEntries(sorts().map(([k, , letter]) => [letter, () => applySort(k)])),
};

/// `M` — everything you can do to the row under the cursor.
///
/// Built fresh each time from what the row actually is, so a directory is not
/// offered "extract" and a plain file is not offered it either. The terminal
/// build's menu does the same, and it is the discoverable half of a program
/// whose other half is a hundred and forty keys.
/// The context menu, built the way cian-tui builds it.
///
/// **Taken from the terminal build's own tree, group for group.** It was
/// twelve flat items here and about a hundred in five zones there, which is
/// most of what "全然違う" meant: in the terminal, `M` is how you reach
/// everything cian does without remembering a key for it, and a short flat
/// list is not that. The zones are its zones — launchers, then the frequent
/// file operations, then the groups, then the OS, then quit — so items sit
/// where the hand already expects them.
///
/// A group with nothing in it is not offered: an entry that can only refuse
/// is worse than no entry.
/// The viewer's own menu — cian-tui's `open_viewer_menu` (menu.rs:10), item
/// for item. Right-clicking an open file used to raise Monaco's own editor
/// menu, which is a menu about a text box rather than about the file.
///
/// The one that loses work is last and on its own, as it is there.
function viewerRows() {
    const v = [];
    // The AI heading is unconditional here as it is in the file menu — the
    // engine says so when no model is configured, which is a better answer
    // than a menu that quietly has one item fewer on some machines.
    // Only when it is configured, as in the listing's menu and as cian-tui
    // does here (menu.rs open_viewer_menu: `if self.ai.is_some() && ai_ready`).
    if (cfg.ai) v.push(group('AI - simple ▸', aiRows));
    v.push({ label: tr("Copy", 'コピー'), value: 'Ctrl+C', run: () => document.execCommand('copy') });
    if (cfg.ai) v.push({ label: tr("Summarise this file", 'このファイルを要約'), value: ':summary', run: cmdSummary });
    v.push({ label: tr("Save", '保存'), value: 'Ctrl+S', run: saveFile });
    v.push({ label: tr("Open in my editor", '外部エディタで開く'), value: ':edit', run: cmdEditExternal });
    v.push({ label: tr("Text encoding\u2026", '文字コードを指定…'), value: ':enc', run: () => cmdEncoding() });
    // `?` はキーの取り合いになりうる ── vim の後方検索、IME、配列。
    // **鍵の取り合いにならない道**を一本、メニューに置く。

    v.push({ label: tr("Who changed each line", '各行の最終変更者'), value: ':blame', run: cmdBlame });
    // cian-tui's row here is `mermaid 図をブラウザで開く`; the window draws the
    // diagrams in the preview instead, so this is the same row by another road.
    v.push({ label: tr("Markdown preview", 'Markdown プレビュー'), value: 'Ctrl+E', run: togglePreview2 });
    v.push({ label: tr("Draw the mermaid diagrams", 'mermaid 図を描く'), value: ':mermaid', run: cmdMermaid });
    v.push({ label: tr("Mermaid diagrams in a browser", 'mermaid 図をブラウザで開く'), value: ':mermaid!', run: cmdMermaidOut });
    v.push({ label: tr("Jump by heading", '見出しから飛ぶ'), value: 'Ctrl+Shift+O', run: cmdOutline });
    // The `:` family, which notepad style has no command line to reach.
    v.push(group(tr("Line operations \u25b8", '行の操作 ▸'), lineOpRows));
    // Where the file lives, for when reading it raises a question about the
    // folder it is in. The cursor is already on it, so this is just the way
    // back out of the viewer.
    v.push({ label: tr("Show where this file is", 'このファイルの場所を開く'), value: '', run: () => closeView(false) });
    v.push({ label: tr("Editor keys: vim / notepad", 'エディタのキー操作: vim / メモ帳'), value: styleName(style), run: () => setStyle(style + 1) });
    v.push({ label: tr("Theme (whole app)", 'テーマ（全体）'), value: ':theme', run: () => cmdTheme() });
    v.push({ label: tr("Close without saving", '保存せずに閉じる'), value: '', run: () => closeView(false) });
    // `?` はこの中のキー、`:help` は cian 全体。**別のもの**で、
    // ここは `?` と書いておきながら全体のほうを開いていた ── 押した人は
    // 「? を押したのに違うものが出た」としか思えない。
    v.push({ label: tr("Keys in here", 'ここのキー一覧'), value: '?', run: viewerHelp });
    v.push({ label: tr("The whole manual", 'cian のキー一覧（全体）'), value: ':help', run: openHelp });
    // **`:key` に、ここから届く道が無かった。**
    //
    // あれは押されたキーを全部飲み込む ── それが目的なので正しい。だが
    // Enter も飲み込むので、**echo を点けてからファイルを開くことはできない**。
    // そしてエディタの中では `:` は vim のもので、cian のコマンド行ではない。
    // つまり「エディタでこのキーが効かない」を調べる道が、エディタの中には
    // 一本も無かった。ファイルを開いてから、ここで点ける。
    v.push({ label: tr("Watch the keys", 'キーを見る'), value: ':key', run: toggleKeyEcho });
    return v;
}

const VIEWER_MENU = {
    key: 'M',
    foot: () => tr('↑↓ choose   Enter run   Esc close', '↑↓ 選ぶ   Enter 実行   Esc 閉じる'),
    stay: false,
    rows: viewerRows,
};

/// The line operations, as a menu rather than as `:` commands only.
///
/// **They were unreachable in notepad style, in both builds.** Every one of
/// them has a `:` command and nothing else, and the editor's command line is
/// vim's — so `:sort` exists for a person using vim keys and does not exist at
/// all for a person using the other grammar. The default grammar is vim, which
/// is why this went unnoticed: the style that could not reach them is the one
/// nobody testing was in.
///
/// Nothing new is implemented here. It is the same `textOp` and `setEol` the
/// commands call, given the second door the menu was already the right place
/// for.
function lineOpRows() {
    return [
        { label: tr("Sort the lines", '行をソート'), value: ':sort', run: () => textOp('sort') },
        { label: tr("Sort in reverse", '行を逆順ソート'), value: ':rsort', run: () => textOp('rsort') },
        { label: tr("Drop duplicate lines", '重複行を落とす'), value: ':uniq', run: () => textOp('uniq') },
        { label: tr("Replace\u2026", '置換…'), value: ':s/…/…/', run: () => cmdSubstitutePrompt() },
        { label: tr("Full-width ASCII \u2192 half-width", '全角ASCII → 半角'), value: ':han', run: () => textOp('han') },
        { label: tr("Half-width kana \u2192 full-width", '半角カナ → 全角'), value: ':zen', run: () => textOp('zen') },
        { label: tr("Leading tabs \u2192 spaces", '行頭のタブ → スペース'), value: ':expand', run: () => textOp('expand') },
        { label: tr("Leading spaces \u2192 tabs", '行頭のスペース → タブ'), value: ':unexpand', run: () => textOp('unexpand') },
        { label: tr("Re-indent to a consistent step", 'インデントを揃える'), value: ':reindent', run: () => textOp('reindent') },
        { label: tr("Line endings to LF", '改行を LF にする'), value: ':lf', run: () => setEol('lf') },
        { label: tr("Line endings to CRLF", '改行を CRLF にする'), value: ':crlf', run: () => setEol('crlf') },
    ];
}

/// `:s/old/new/g`, asked for rather than typed — the menu's way in.
async function cmdSubstitutePrompt() {
    const spec = await askFor(tr('replace s/old/new/g', '置換 s/古い/新しい/g'), 's///g');
    if (!spec) { say(tr('stopped', 'やめました')); return; }
    await cmdSubstitute(spec);
}

/// What init.lua turned on, as far as the menu is concerned. Filled from the
/// `settings` reply at startup; false until then, which is the safe way round
/// — a row that is missing for a moment beats a row that leads nowhere.
const cfg = { ai: false, snippets: false, macros: false, hosts: false, tabWidth: 0,
              /// The face `cian.font{ face = … }` asked for, so the opening line
              /// can say whether it is the one that drew. See [`applyFace`].
              faceAsked: null };

/// init.lua の `tab_width` を、いま開いているモデルに当てる。
///
/// **`create` の options ではなくモデルの側。** タブ幅は Monaco では文書の
/// 性質（`ITextModelUpdateOptions`）で、エディタの表示設定ではない ── 作る
/// ときに渡しても効かない。差分エディタは2つのモデルを持つので両方。
function applyTabWidth() {
    if (!cfg.tabWidth) return;
    const opts = { tabSize: cfg.tabWidth };
    for (const m of (window.monaco ? window.monaco.editor.getModels() : [])) {
        m.updateOptions(opts);
    }
}

/// What this platform will do, from the engine — not from the user agent.
///
/// `navigator.platform` knows which browser this is, and the file manager is a
/// different question: "Open with…" is a Windows shell verb and the properties
/// panel exists on two of the three. cian-tui gates the same two rows on the
/// same facts (menu.rs `OsMenu`), so the engine answers for both.
const osCan = { open_with: false, properties: false, file_manager: 'Finder' };

/// Where the synced Office libraries are on this disk, from init.lua's
/// `cian.sharepoint{…}`. Empty until the engine answers, which is the safe way
/// round — the two Office rows appear a moment late rather than appearing and
/// then only being able to refuse.
let sharepoint = [];

/// Would the Office rows do anything for the row under the cursor?
///
/// The two tests `cloud_url` and `classify` do, run here: is this an Office
/// document, and is it inside a configured library. cian-tui asks the same
/// pair before it pushes the rows (menu.rs: `!sharepoint.is_empty() &&
/// office_target_ok()`).
function officeTarget(row) {
    if (!sharepoint.length || !row || row.is_dir || row.parent) return false;
    if (!/\.(docx?|docm|xlsx?|xlsm|xlm|pptx?|pptm|pdf)$/i.test(row.name)) return false;
    return sharepoint.some((root) => row.path && row.path.startsWith(root));
}

function contextRows() {
    const pane = state[state.focus];
    const row = pane && pane.entries[pane.cursor];
    const has = row && !row.parent;
    const inShell = term.on && term.focused;
    const v = [];

    // ── first, because it is the switch reached for most ──
    //
    // 2026-09-05: 「クラシックやアイコン・cian モードへの遷移はトグル・
    // メニューの最上位に移動してほしい」. It changes what the window *is* —
    // a file manager or a wall of tiles — and everything below is a thing to
    // do inside whichever one you are in.
    if (!inShell) {
        v.push(group(tr('Mode ▸', 'モード ▸'), () => VIEWS.map((m) => ({
            label: viewName(m) + (m === viewMode ? '  \u25cf' : ''),
            value: `:view ${m}`,
            run: () => { closeMenu(); setView(m); say(tr(`listing: ${viewName(m)}`, `一覧: ${viewName(m)}`)); },
        }))));
    }

    // ── launchers ──
    //
    // Each only when it has something to offer, which is the rule cian-tui
    // follows (menu.rs open_context_menu: `if ai`, `if !snippets.is_empty()`,
    // `if !macros.is_empty()`). Offered unconditionally here, three of the
    // first four rows led to "there is nothing here" on a machine with no
    // init.lua — and pushed everything that does work further down.
    if (cfg.ai) v.push(group('AI - simple ▸', aiRows));
    if (cfg.snippets) v.push({ label: tr("Snippets", 'スニペット'), value: ':snip', run: cmdSnippets });
    if (cfg.macros) v.push({ label: tr("Macros", 'マクロ'), value: '@', run: cmdMacros });
    // Bookmarks are a launcher too, so cian-tui keeps them in this cluster
    // rather than down among the connect rows — but only in a file pane.
    if (!inShell) v.push({ label: tr("Shortcuts", 'ショートカット'), value: 's', run: cmdShortcuts });

    if (inShell) {
        // `:` is a character in a shell, so the command line needs a way in
        // that is not a keystroke — which is why cian-tui puts this row here
        // and only here (menu.rs). In a file pane the key works, and a row
        // that duplicates a working key is a row in the way.
        v.push({ label: tr("Command", 'コマンド入力'), value: 'Ctrl+Enter', run: () => commandLine() });
        // The shell's own menu: what can be done to a terminal, not to a file.
        v.push({ label: tr("Paste", '貼り付け'), value: 'Ctrl+V', run: shellPaste });
        v.push({ label: tr("Copy the selection", '選択をコピー'), value: 'Ctrl+C', run: shellCopy });
        // **Where Ctrl+C went.** 2026-09-05: the shell's Ctrl+C/X/V are the
        // clipboard now, which is what hands coming from Windows expect —
        // so the interrupt needs somewhere to live, and this is the menu
        // that is already one keystroke away. Right-click opens the same one.
        v.push({ label: tr("Interrupt  (sends Ctrl+C)", '中断  （Ctrl+C を送る）'), value: '', run: () => ask('shellinput', { text: '\x03' }) });
        v.push(group(tr('Session ▸', 'セッション ▸'), () => [
            // Named for what it will do, as cian-tui names it (`StartLog` /
            // `StopLog`, chosen in `submenu_children` from whether this pane is
            // already recording). One row that says "start／stop" makes the
            // reader work out which of the two they are about to get.
            el.shell.classList.contains('logging')
                ? { label: tr("Stop session log  \u25cf", 'セッションログ停止  ●'), value: ':sessionlog', run: cmdShellLog }
                : { label: tr("Start session log", 'セッションログ開始'), value: ':sessionlog', run: cmdShellLog },
            { label: tr("Text encoding", '文字コード'), value: 'e', run: () => cmdEncoding() },
            // 同じ理由で ── シェルでも `:` は文字なので、ここが唯一の道。
            { label: tr("Watch the keys", 'キーを見る'), value: ':key', run: toggleKeyEcho },
        ]));
        v.push(group(tr('Window ▸', 'ウィンドウ ▸'), () => [
            { label: tr("Split left / right", '左右に分割'), value: 'S-F8', run: () => splitShell(false) },
            { label: tr("Split top / bottom", '上下に分割'), value: 'S-F9', run: () => splitShell(true) },
            { label: tr("New tab", '新規タブ'), value: 'F9', run: shellTab },
            // Whichever close matches what is active — a split pane when this
            // tab is split, otherwise the tab (cian-tui `WindowMenu`). The
            // window only ever offered the split one, so from an unsplit shell
            // the menu's close key answered nothing.
            ...(shellPaneCount() > 1
                ? [{ label: tr("Close split pane", '分割パネルを閉じる'), value: 'S-F10', run: () => closePane() }]
                : [{ label: tr("Close tab", 'タブを閉じる'), value: 'F10', run: () => shellCloseTab() }]),
            { label: tr("Zoom", 'ズーム'), value: 'F12', run: zoomFocused },
            { label: tr("This pane only", 'このペインだけ'), value: 'S-F12', run: () => ask('shellpanezoom', {}).then((r) => r && takeShell(r)) },
        ]));
        v.push({ label: tr("Name this shell", 'このシェルに名前を付ける'), value: ':shellname', run: cmdShellName });
        // cian-tui offers this only when there is more than one pane to
        // synchronise (menu.rs: `if active_pane_count() > 1`). One pane
        // "broadcasting" to itself is a row that describes nothing.
        if (shellPaneCount() > 1) {
            // cian-tui offers the member row only while sync is on: with it
            // off the group is a thing that does not exist yet, and narrowing
            // nothing is not an action.
            if (term.sync) {
                v.push({ label: tr("Stop synchronize  \u21c4", '同時入力を停止  ⇄'), value: 'Ctrl+S', run: cmdSync });
                v.push({ label: tr("Toggle this pane in sync group  \u21c4", 'このペインを同時入力に含める/外す  ⇄'), value: '', run: cmdSyncMember });
            } else {
                v.push({ label: tr("Synchronize input  \u21c4", '同時入力を開始  ⇄'), value: 'Ctrl+S', run: cmdSync });
            }
        }
        v.push({ label: tr("Back to the files", 'ファイルへ戻る'), value: 'Esc', run: () => { setShellFocus(false); say(tr('files', 'ファイル')); } });
        // and on into the shared block below — the shell menu used to stop
        // here, so connecting to a server, changing the colours and quitting
        // were all unreachable by mouse from the shell. cian-tui runs both
        // menus through the same tail for exactly that reason.
    }

    // ── the frequent file operations ──
    if (has && !inShell) {
        // cian-tui's order, item for item (menu.rs `open_context_menu`):
        // copy, the two ways of copying *what it is*, cut, paste, rename,
        // delete, open in a tab. The window had `開く` first and the three
        // copies scattered, which is a different menu wearing the same words.
        v.push({ label: tr("Copy", 'コピー'), value: 'Ctrl+C', run: () => hold('copy') });
        v.push({ label: tr("Copy path text", 'パスをコピー'), value: 'p', run: copyPaths });
        v.push({ label: tr("Copy file(s) \u2014 paste into Finder/Explorer", 'ファイルをコピー — Finder/エクスプローラに貼り付け'), value: 'Shift+P', run: clipFiles });
        v.push({ label: tr("Cut", '切り取り'), value: 'Ctrl+X', run: () => hold('cut') });
        v.push({ label: tr("Paste", '貼り付け'), value: 'Ctrl+V', run: paste });
        v.push({ label: tr("Rename", 'リネーム'), value: 'r', run: rename });
        v.push({ label: tr("Delete", '削除'), value: 'd', run: () => operate('delete') });
        v.push({ label: tr("Open in a new tab", '新規タブで開く'), value: 't', run: tabNew });
        // cian-tui's EditTab: the file in $EDITOR, in a shell tab of its own.
        // It was reachable only by knowing `:vim`.
        v.push({ label: tr("Edit in new tab", '新規タブで編集'), value: ':vim', run: () => cmdEditorTab('') });
        v.push(group(tr("File \u25b8", 'ファイル操作 ▸'), () => [
            { label: tr("Copy to other pane", '反対ペインへコピー'), value: 'c', run: () => operate('copy') },
            { label: tr("Move to other pane", '反対ペインへ移動'), value: 'm', run: () => operate('move') },
            { label: tr("Copy to", '指定先へコピー'), value: ':copyto', run: () => commandLine('copyto ') },
            { label: tr("Rename by pattern", 'パターンでリネーム'), value: ':renamepattern', run: cmdRenamePattern },
            { label: tr("Rename in editor", 'エディタでリネーム'), value: ':renamelist', run: cmdRenameList },
        ]));
        // cian-tui nests this one: `アーカイブ ▸` holds `圧縮 ▸` and, only on
        // an archive, `ここに解凍`. Same two levels here.
        v.push(group(tr("Archive \u25b8", 'アーカイブ ▸'), () => {
            const rows = [group(tr("Compress \u25b8", '圧縮 ▸'), () => [
                { label: '→ .zip', value: ':zip', run: () => cmdCompress('zip') },
                { label: tr("\u2192 .zip  (password)", '→ .zip  (パスワード)'), value: ':zip -e', run: () => cmdCompress('zipenc') },
                { label: '→ .tar.gz', value: ':targz', run: () => cmdCompress('targz') },
            ])];
            if (isArchive(row)) {
                rows.push({ label: tr("List contents", '中身を見る'), value: ':lsar', run: cmdArchiveList });
                rows.push({ label: tr("Extract here", 'ここに解凍'), value: ':extract', run: cmdExtract });
            }
            return rows;
        }));
    }

    // Inspect and the version control groups belong to a listing; cian-tui
    // keeps them inside the non-shell branch, and a shell has no cursor for
    // them to act on.
    if (!inShell) {
        v.push(group(tr("Inspect \u25b8", '調べる ▸'), () => [
            { label: tr("Attributes", '属性'), value: ':attr', run: cmdAttr },
            { label: tr("Checksum", 'チェックサム'), value: ':hash', run: () => cmdHash('') },
            { label: tr("Compare left \u2194 right", '左右を比較'), value: '=', run: cmdCompare },
            { label: tr("Count files & steps", 'ファイル・ステップ数を数える'), value: ':count', run: cmdCount },
            { label: tr("Disk usage", '容量分析'), value: ':du', run: cmdDu },
            { label: tr("Find duplicate files", '重複ファイルを検出'), value: ':duplicate', run: cmdDedup },
        ]));
        // One of the two, and only where there is a repository. Both were
        // offered everywhere, so a plain directory showed thirteen version
        // control rows that could only answer "not a repository" — and in a
        // git checkout, `svn ▸` sat there claiming otherwise. cian-tui matches
        // on the kind and pushes one (menu.rs `match self.vcs_kind()`).
        const kind = repo[state.focus] && repo[state.focus].v && repo[state.focus].v.kind;
        if (kind === 'git') {
            v.push(group('Git ▸', () => [
                { label: tr("Stage", 'ステージ'), value: 'git add', run: () => cmdVcs('stage') },
                { label: tr("Unstage", 'アンステージ'), value: 'git reset', run: () => cmdVcs('unstage') },
                { label: tr("Discard changes", '変更を破棄'), value: 'git checkout', run: () => cmdVcs('discard') },
                { label: tr("Diff vs HEAD", 'HEADとの差分'), value: 'git diff', run: () => cmdVcsDiff(null) },
                { label: tr("History / log", '履歴 / ログ'), value: 'git log', run: () => cmdLog(false) },
                // cian-tui's GitHistory is one row (repo, or the file's own
                // history). The file-scoped one has no counterpart there, so
                // it keeps its own row rather than changing what the shared
                // one means.
                { label: tr("This file's history", 'このファイルの履歴'), value: ':filelog', run: () => cmdLog(true) },
            ]));
        } else if (kind === 'svn') {
            v.push(group('SVN ▸', () => [
                { label: tr("Add", '追加'), value: 'svn add', run: () => cmdSvn('stage') },
                { label: tr("Discard changes", '変更を破棄'), value: 'svn revert', run: () => cmdSvn('discard') },
                { label: tr("Resolve conflict", '競合を解決'), value: 'svn resolve', run: () => cmdSvn('resolve') },
                { label: tr("Diff vs BASE", 'BASEとの差分'), value: 'svn diff', run: () => cmdVcsDiff('svn') },
                { label: tr("History / log", '履歴 / ログ'), value: 'svn log', run: () => cmdLog(false, 'svn') },
                { label: tr("Update", '更新'), value: 'svn update', run: () => cmdSvn('update') },
                { label: tr("Commit", 'コミット'), value: 'svn commit', run: () => cmdSvn('commit') },
            ]));
        }
    }
    // ── shared: connect, then appearance, then the way out ──
    v.push({ label: tr("SSH connect", 'SSH接続'), value: ':ssh', run: cmdSshPicker });
    // cian-tui offers the remote pane right under SSH (menu.rs RemotePane).
    // The window had `:sftp` as a command only, so the one way to reach a
    // server by mouse was the ssh picker — which opens a shell, not a pane.
    v.push({ label: tr("Open server in pane", 'サーバをペインで開く'), value: ':sftp', run: cmdSftpPicker });
    // cian-tui offers this group only when init.lua names a host — with none
    // configured the two rows can only say so (menu.rs: `if has_hosts`).
    if (cfg.hosts) {
        v.push(group(tr("Transfer \u25b8", '転送 ▸'), () => [
            { label: tr("Upload \u2192 server", 'アップロード → サーバ'), value: '', run: () => cmdSend('up') },
            { label: tr("Download \u2190 server", 'ダウンロード ← サーバ'), value: '', run: () => cmdSend('down') },
        ]));
    }
    v.push({ label: tr("Running operations", '動いている処理'), value: ':queue', run: cmdQueue });
    // Appearance sits at the top level in cian-tui (ThemePick, between
    // Background and Lang), not folded into the view group. It was two levels
    // down here, which is how someone with twenty-one palettes in front of
    // them came to ask whether themes could be chosen at all.
    // cian-tui's appearance zone, in its order: Background, then the whole-app
    // theme (menu.rs pushes them adjacent, ahead of Lang and the view group).
    // Menu-only, as it is there: cian-tui gives this no key and no `:` verb.
    if (!inShell) v.push({ label: tr("Background color", '背景色'), value: '', run: cmdPaneGround });
    v.push({ label: tr("Theme (whole app)", 'テーマ（全体）'), value: ':theme', run: cmdTheme });
    // Labelled with the language it switches *to*, so the row is clear
    // whichever language the menu is currently in — cian-tui's own reasoning
    // for the same row (`MenuItem::Lang`).
    v.push({
        label: lang === 'en' ? '日本語に切替' : 'Switch to English',
        value: '',
        run: () => setLang(lang === 'en' ? 'ja' : 'en'),
    });
    v.push({ label: tr("Full screen", '全画面'), value: 'F11', run: cmdFullscreen });
    v.push({ label: tr("Zoom this surface", 'この面を広げる'), value: 'F12', run: zoomFocused });
    // The listing's own groups, and the OS group: both act on a file under a
    // cursor, so cian-tui leaves them out of the shell menu.
    if (!inShell) {
        // cian-tui's ViewMenu, dotfiles, then the switches. Its glyphs too —
        // they are what the corner switcher shows.
        //
        // **並びは一つに揃えた**（2026-09-05）: クラシック → 詳細一覧。
        // 最上位の `モード ▸` は `VIEWS` を並べ、ここは同じ順を写す。端末版の
        // `menu.rs` も同じ順 ── 順番が二つあるのは順番が無いのと同じで、
        // どちらが「正しい並び」か言えなくなる。
        v.push(group(tr("View \u25b8", '表示 ▸'), () => [
            { label: tr("\u25a5 Classic", '▥ クラシック'), value: ':view classic', run: () => { setView('classic'); say(tr('listing: classic', '一覧: クラシック')); } },
            { label: tr("\u25a4 Details", '▤ 詳細一覧'), value: ':view details', run: () => { setView('details'); say(tr('listing: details', '一覧: 詳細一覧')); } },
            { label: tr("Show / hide dotfiles", 'ドットファイルの表示切替'), value: ':hidden', run: toggleHidden },
            { label: tr("Theme (this pane)", 'テーマ（このペイン）'), value: '', run: cmdPaneTheme },
            { label: tr("Switches\u2026", '各種スイッチ…'), value: 'T', run: () => openMenu(TOGGLES) },
        ]));
        v.push(group(tr("Open / reveal  \u25b8", '開く / 場所 ▸'), () => {
            // cian-tui's OsMenu, row for row and gated the same way: a row that
            // could only answer "not on this platform" is worse than no row.
            const rows = [
                { label: tr("Open", '開く'), value: 'Ctrl+Enter', run: openOut },
            ];
            if (osCan.open_with) {
                rows.push({ label: tr("Open with", 'プログラムから開く'), value: '', run: cmdOpenWith });
            }
            // Only where there are libraries to resolve against and the file
            // is one of theirs — an item that can only ever refuse is not
            // worth the row (cian-tui's words for its own version of this).
            if (officeTarget(row)) {
                rows.push({ label: tr("Open in Office (the cloud copy)", 'Office で開く（クラウド側）'), value: ':office', run: () => cmdOffice('office') });
                rows.push({ label: tr("Shortcut to the cloud copy", 'クラウド側へのショートカットを作成'), value: ':officelink', run: () => cmdOffice('officelink') });
            }
            rows.push({ label: tr("Open in my editor", '外部エディタで開く'), value: ':edit', run: cmdEditExternal });
            rows.push({ label: tr(`Show in ${osCan.file_manager}`, `${osCan.file_manager} で表示`), value: ':revealos', run: cmdRevealOs });
            if (osCan.properties) {
                rows.push({ label: ON_MAC ? tr("Get Info", '情報を見る') : tr("Properties", 'プロパティ'), value: '', run: cmdProperties });
            }
            return rows;
        }));
    }
    // cian-tui ends Quit then Manual — the way out, then the way to find out.
    v.push({ label: tr("Quit cian", 'cian を終了'), value: 'q', run: cmdQuit });
    v.push({ label: tr("Key manual", 'キー一覧'), value: '?', run: openHelp });
    return v;
}

/// A row that opens a submenu instead of doing something.
///
/// `rows` is a function, not a list: what a group offers depends on what is
/// under the cursor at the moment it is opened, and a list built when the
/// parent was drawn would be a list about the file you were on then.
/// Which modifier means "add this to the selection". Not the same key
/// everywhere: Ctrl+click is the secondary click on macOS.
const ON_MAC = navigator.userAgent.includes('Mac');
const ADD_TO_MARKS = (e) => (ON_MAC ? e.metaKey && !e.ctrlKey : e.ctrlKey);

function group(label, rows) {
    return { label, value: '▸', group: rows };
}

function isArchive(row) {
    return row && !row.is_dir && /\.(zip|tar|gz|tgz|bz2|xz|7z|rar|jar)$/i.test(row.name);
}

/// The AI group, in cian-tui's three shapes (menu.rs `submenu_children`,
/// `MenuItem::AiMenu`): chat leads all three, then what the surface underneath
/// is for. The names are its names — this list said 自由に訊く・不要さがし・
/// 畳み方の案 for rows the terminal build calls チャット・ゴミファイル検出・
/// ディレクトリ構成を提案, which is one program with two vocabularies.
function aiRows() {
    if (viewer.on) {
        return [
            { label: tr("Chat", 'チャット'), value: ':ai', run: () => cmdAiAsk('') },
            { label: tr("Improve this writing", 'この文章を推敲'), value: '', run: () => cmdAiOverText('writing') },
            { label: tr("Explain / write this command", 'コマンドを説明・作成'), value: '', run: () => cmdAiOverText('command') },
            { label: tr("Review and fix this code", 'このコードを点検・修正'), value: '', run: () => cmdAiOverText('code') },
        ];
    }
    if (term.on && term.focused) {
        return [
            { label: tr("Chat", 'チャット'), value: ':ai', run: () => cmdAiAsk('') },
            { label: tr("Command from description", '説明からコマンド生成'), value: ':aicmd', run: () => cmdAiCmd('') },
            { label: tr("Explain the last error", '直近のエラーを説明'), value: ':explain', run: cmdAiError },
        ];
    }
    return [
        { label: tr("Chat", 'チャット'), value: ':ai', run: () => cmdAiAsk('') },
        { label: tr("Triage this log", 'このログを診断'), value: ':ailog', run: cmdAiLog },
        { label: tr("Detect junk files", 'ゴミファイル検出'), value: ':aijunk', run: () => cmdAiScan('aijunk') },
        { label: tr("Suggest folder structure", 'ディレクトリ構成を提案'), value: ':organize', run: () => cmdAiScan('aistructure') },
        { label: tr("Semantic search", 'セマンティック検索'), value: ':ask', run: () => commandLine('aisearch ') },
        { label: tr("AI rename", 'AIリネーム'), value: ':airename', run: () => commandLine('airename ') },
        { label: tr("Draft commit message", 'コミットメッセージ生成'), value: ':aicommit', run: cmdAiCommit },
    ];
}

const CONTEXT = {
    key: 'M',
    foot: () => tr('↑↓ choose   Enter run   ← / Esc back', '↑↓ 選ぶ   Enter 実行   ← / Esc 戻る'),
    stay: false,
    rows: contextRows,
};

/// Called as the menu cursor passes a row, for a menu whose rows *are* the
/// thing being chosen — a colour is looked at, not read. The sheet is a small
/// centred panel, so the panes are visible around it and the preview is
/// actually on screen; the full-width report is not (which is why the colour
/// pickers use this and not `show()`).
function menuMoved(spec, rows) {
    if (spec.move) spec.move(rows[menu.at]);
}

/// One menu driver, not one per menu.
///
/// The switches and the sort picker are the same object with different rows,
/// and a third near-copy of "draw a list, move a cursor, run the row" is how
/// they would start behaving differently from each other.
const menu = { spec: null, at: 0, byKey: false };

/// Where a submenu came from, so ← and Esc go back one level rather than
/// dropping you out of the menu entirely — which in a tree this size means
/// starting the whole search again.
const menuStack = [];

function openMenu(spec) {
    if (!spec.child) menuStack.length = 0;
    menu.spec = spec;
    menu.at = Math.max(0, spec.at ? spec.at() : 0);
    el.find.hidden = false;
    // A function when the text depends on the language: a menu built as a
    // `const` had its foot translated once, at startup, and then said the
    // wrong language for ever after the first switch.
    el.findFoot.textContent = typeof spec.foot === 'function' ? spec.foot() : spec.foot;
    drawMenu();
}

/// Do what a row says: open its submenu, or run it and close.
function runMenuRow(row, spec) {
    if (!row) return;
    if (row.back) { menuBack(); return; }
    if (row.group) {
        const rows = row.group();
        if (!rows.length) { say(tr(`${row.label} — nothing to do here`, `${row.label} — できることがありません`), true); return; }
        menuStack.push(spec);
        openMenu({
            key: spec.key,
            foot: () => tr("\u2191\u2193 choose   Enter run   \u2190 / Esc back", '↑↓ 選ぶ   Enter 実行   ← / Esc 戻る'),
            child: true,
            // A *copy* each time. The submenu closed over one array, and
            // menuRows() appends the `◂ 戻る` row to what it is given — so
            // asking twice put two of them on, and the drawing and the keys
            // ask separately.
            rows: () => rows.slice(),
        });
        return;
    }
    // What the row opened, if it opened anything. `各種スイッチ…` and the two
    // colour pickers all answer by raising another menu — and this line then
    // closed it again, one statement after it appeared. The switches menu was
    // therefore reachable by `T` and not by the menu row that names it.
    const before = menu.spec;
    row.run();
    if (!spec.stay && menu.spec === before) closeMenu();
}

/// Up one level, or out. cian-tui's `menu_back()` — written once here now
/// that the mouse wants it too (right-click inside a submenu).
function menuBack() {
    if (menuStack.length) openMenu(menuStack.pop());
    else closeMenu();
}

function closeMenu() {
    // A menu that has been changing something while you looked at it gets to
    // put it back when you leave without choosing — the same promise `show()`
    // keeps with `leave`, and the reason Esc on the colour pickers restores
    // the ground rather than leaving whatever the cursor last passed over.
    if (menu.spec && menu.spec.leave) menu.spec.leave();
    menuStack.length = 0;
    menu.spec = null;
    el.find.hidden = true;
}

/// Close without the `leave` promise — for the row that *chose* something.
function closeMenuChosen() {
    if (menu.spec) menu.spec.leave = null;
    closeMenu();
}

/// The rows the menu is showing, including the `◂ 戻る` cian-tui ends every
/// submenu with. One function, because the drawing and the keys both need
/// them and a Back row added to only one of the two lists puts the cursor and
/// the row it runs one apart.
function menuRows() {
    const rows = menu.spec.rows();
    if (menuStack.length) rows.push({ label: tr("\u25c2 Back", '◂ 戻る'), value: 'Esc', back: true });
    return rows;
}

/// Move the highlight without rebuilding the menu.
///
/// **`mouseenter` fires when the element arrives under the pointer, not only
/// when the pointer arrives over the element.** `drawMenu()` replaces every
/// row, so pressing ↓ built a fresh row underneath a pointer that had not
/// moved — which fired mouseenter, which put `menu.at` straight back where
/// the mouse was resting. The highlight could not be moved off whatever the
/// pointer happened to be over, in T and in M both, while the listing was
/// fine because it binds no such handler.
///
/// The heuristic version of this fix compared pointer coordinates and could
/// not tell the two cases apart — they have identical coordinates. Not
/// rebuilding the rows is the whole fix: no element is created, so no
/// synthetic mouseenter exists to be filtered.
function paintMenuCursor() {
    // The keyboard has the cursor until the pointer is actually moved. Set
    // *before* the scroll, because the scroll is the thing that fires the
    // event this is guarding against.
    menu.byKey = true;
    const hits = el.findHits.children;
    for (let i = 0; i < hits.length; i += 1) hits[i].classList.toggle('on', i === menu.at);
    hits[menu.at]?.scrollIntoView({ block: 'nearest' });
}

// A real pointer movement, as opposed to the list moving underneath a
// stationary one. Captured, so an overlay cannot hide it.
document.addEventListener('mousemove', () => { menu.byKey = false; }, true);

function drawMenu() {
    const rows = menuRows();
    const frag = document.createDocumentFragment();
    let cursorRow = null;
    rows.forEach((row, i) => {
        const div = document.createElement('div');
        div.className = 'hit' + (i === menu.at ? ' on' : '');
        if (i === menu.at) cursorRow = div;
        const l = document.createElement('span');
        l.className = 'p';
        l.textContent = row.label;
        const v = document.createElement('span');
        v.textContent = keyLabel(row.value);
        div.append(l, v);
        div.addEventListener('mousedown', () => {
            menu.at = i;
            runMenuRow(row, menu.spec);
        });
        // The pointer moves the cursor, as it does in cian-tui (mouse.rs:609).
        // Without it the highlight and the pointer disagree about which row
        // a click is going to land on.
        // The pointer moves the cursor, as it does in cian-tui (mouse.rs:609)
        // — but only when the pointer is what moved.
        //
        // **`mouseenter` fires when the row arrives under the pointer too**,
        // and there are two ways for that to happen. Rebuilding the list was
        // the first, and not rebuilding it fixed that one. Scrolling is the
        // second, and it is the one that bit at the bottom of a long menu:
        // ↓ past the last visible row scrolls the list, the row under the
        // resting mouse changes, mouseenter fires, and the cursor jumps back
        // to the pointer. So the keyboard holds the cursor until a genuine
        // `mousemove` says otherwise. Coordinates cannot tell these apart —
        // in both cases the pointer is exactly where it was.
        div.addEventListener('mouseenter', () => {
            if (menu.at === i || menu.byKey) return;
            menu.at = i;
            // Not `paintMenuCursor()`: that would hand the cursor back to the
            // keyboard, and the pointer is what just moved it.
            const hits = el.findHits.children;
            for (let n = 0; n < hits.length; n += 1) hits[n].classList.toggle('on', n === menu.at);
        });
        // Right-click climbs one level, which is the mouse's Esc here.
        div.addEventListener('contextmenu', (e) => {
            e.preventDefault();
            e.stopPropagation();
            menuBack();
        });
        frag.append(div);
    });
    el.findHits.replaceChildren(frag);
    // Keep the highlight on screen. ↓ moved `menu.at` past the bottom of a
    // long menu and nothing scrolled after it, so the chosen row was somewhere
    // below the fold and the menu looked as though the key had stopped
    // working. `nearest` so a row already in view does not jump.
    if (cursorRow) cursorRow.scrollIntoView({ block: 'nearest' });
}

// Clicking away from a sheet closes it. cian-tui works out whether the click
// landed on the popup's ink and sends Esc when it did not (mouse.rs:966) —
// here the sheet simply swallowed the click and stayed open, which is the one
// thing every window in the world does differently.
function dismissFind() {
    if (menu.spec) closeMenu();
    else if (finder.open) closeFinder();
    else if (help.on) closeHelp();
}

// Written out rather than looped: a call through a destructured name is a
// call the audit cannot follow to a definition, and a checker that shrugs at
// one call shrugs at the next one too.
// Right-click on an open file opens cian's menu, not Monaco's. Monaco's is a
// menu about a text box — cut, copy, the command palette — where the question
// is about the file: save it, re-read it in another encoding, who changed
// this line, close it without saving.
// Captured, because Monaco handles `contextmenu` on its own container and
// stops it there — a listener waiting for the bubble never hears the click.
el.view.addEventListener('contextmenu', (e) => {
    if (!viewer.on) return;
    e.preventDefault();
    e.stopPropagation();
    openMenu(VIEWER_MENU);
}, true);

el.find.addEventListener('mousedown', (e) => {
    if (e.target !== e.currentTarget) return;   // the sheet itself, not its ink
    dismissFind();
});
el.report.addEventListener('mousedown', (e) => {
    if (e.target !== e.currentTarget) return;
    closeReport(true);
});

document.addEventListener('keydown', (e) => {
    if (!menu.spec) return;
    e.stopPropagation();
    const spec = menu.spec;
    const rows = menuRows();
    const pick = spec.letters && spec.letters[e.key];
    if ((e.key === 'Escape' || e.key === 'ArrowLeft' || e.key === 'h') && menuStack.length) {
        // Up one level, not out. The terminal build's `Back` row, on the key
        // a vi user's hand is already on.
        menuBack();
    }
    else if (e.key === 'Escape' || e.key === spec.key) closeMenu();
    else if (e.key === 'ArrowDown' || e.key === 'j') { menu.at = (menu.at + 1) % rows.length; paintMenuCursor(); menuMoved(spec, rows); }
    else if (e.key === 'ArrowUp' || e.key === 'k') { menu.at = (menu.at + rows.length - 1) % rows.length; paintMenuCursor(); menuMoved(spec, rows); }
    else if (pick) { closeMenu(); pick(); }
    else if (e.key === 'Enter' || e.key === ' ' || e.key === 'ArrowRight' || e.key === 'l') {
        runMenuRow(rows[menu.at], spec);
    } else return;
    e.preventDefault();
    // Not `stopPropagation`: that stops the event travelling *onward*, and
    // every one of these handlers is on `document`, so it stops nothing here.
    // The row this Enter ran may have opened a list — and that list's handler,
    // registered later on the same element, then saw the same Enter and picked
    // its first row. `テーマ（全体）` from this menu therefore chose 白磁 and
    // closed before anyone could look at the gallery, which is what was
    // reported on Windows as "配色が選べない" and diagnosed as the menu being
    // two levels deep. It was two faults wearing one symptom.
    e.stopImmediatePropagation();
}, true);

/// What `?` shows.
///
/// **Taken from cian's own key table, not written afresh.** Two keys in this
/// front end had drifted — sorting had wandered off `,`, and the look cycle had
/// taken `T`, which is the switches — and both were found by reading the
/// terminal build's list rather than by anyone noticing while using it. A help
/// screen written from memory would have recorded the drift as if it were the
/// design.
/// Built when `?` is pressed, not when the file loads.
///
/// `tr()` answers for the language that is on *now*, and a `const` array
/// evaluated at startup would freeze whichever language the window opened in
/// — the switch would work everywhere except the two screens made entirely of
/// words.
/// The editor's own keys, as a list you can read while the file waits behind
/// it. cian-tui's `?` in the viewer, over the section the help already has —
/// one list rather than two that would drift.
function viewerHelp() {
    const want = tr("Reading and writing (F3 / Enter)", '読み書き（F3・Enter）');
    const section = helpRows().find(([name]) => name === want);
    // **キーは `label`、説明は `sub`。**
    //
    // 前は逆で、キーを `n`（行番号のための右詰め 9ch の列）に入れ、説明を
    // `label` に入れていた ── だから短いキーは右に寄り、長いキーは列から
    // はみ出し、`align` は説明のほうを揃えていた。届いた画像がそのまま
    // 「ぐっちゃぐちゃ」で、そのとおりでした。
    const rows = (section ? section[1] : []).map(([k, what]) => ({ label: keyLabel(k), sub: what }));
    show(tr('The editor', 'エディタ'),
        tr('the keys in here — Esc goes back to the file', 'この中のキー ── Esc でファイルに戻ります'),
        rows, {
            align: true,
            // 読むための一覧なので、説明は折り返す。1行に詰めて `…` で
            // 切ると、いちばん知りたい後半が消える。
            wrap: true,
            foot: tr('↑↓ scroll   Esc back to the file', '↑↓ 送る   Esc ファイルに戻る'),
        });
}

function helpRows() {
  return [
    [tr("Navigation", '移動'), [
        ['j / k / ↑ ↓', tr("one down / up", 'ひとつ下 / 上')],
        ['Shift+D / Shift+U', tr("ten lines at a time", '10行ずつ')],
        ['gg / G', tr("top / bottom", '先頭 / 末尾')],
        ['Enter', tr("enter a folder / read the file / go into an archive", 'ディレクトリへ入る / ファイルを読む / アーカイブの中へ')],
        [tr("F3 inside an archive", 'アーカイブの中で F3'), tr("read and edit a member; Ctrl+S writes it back", '中のファイルを読む・直す。Ctrl+S で書き戻す')],
        ['Ctrl+Enter', tr("a folder opens in the other pane; a file in its default app", 'ディレクトリは反対ペインへ / ファイルは既定のアプリで')],
        ['Backspace', tr("up one level", '親ディレクトリへ')],
        ['z', tr("go to a typed path", '入力したパスへ移動')],
        ['Tab', tr("the other pane", '反対のペインへ')],
        ['t / F9', tr("new tab, here (it asks first)", '新規タブ（いまの場所で開く。先に訊きます）')],
        ['w  /  F10', tr("close the tab (F10 asks first)", 'タブを閉じる（F10 は確認あり）')],
        ['F1 / F2', tr("previous / next tab (Shift+Tab also goes forward)", '前 / 次のタブ（Shift+Tab でも次へ）')],
        ['← → / Ctrl+h / Ctrl+l', tr("focus the left / right pane", '左 / 右のペインにフォーカス')],
        ['Shift+H / Shift+L', tr("move focus between panes (Shift+J goes to the shell, Esc comes back)", 'ペイン間でフォーカス移動（Shift+J はシェルへ、Esc で戻る）')],
        ['F5', tr("reload", '読み直す')],
        [':view', tr("how the listing is laid out \u2014 classic (two panes) / details. Also on T", '一覧の見せ方 — classic（2画面） / details（詳細一覧）。T でも')],
        ['Ctrl+= / Ctrl+- / Ctrl+0', tr("bigger / smaller / back to the base size", '文字を大きく / 小さく / 元に戻す')],
    ]],
    [tr("Find", '探す'), [
        ['f  →  n / N', tr("search this listing, next, previous", 'この一覧を検索・次・前')],
        ['/', tr("narrow this listing", 'この一覧を絞り込み')],
        ['/ /  、Ctrl+P', tr("fuzzy-find a file anywhere below here", 'この下のどこかにあるファイルをあいまい検索')],
        ['Shift+F', tr("find by name, through the whole tree \u2014 :find", '名前で探す（この下すべて）── :find')],
        ['Ctrl+F / Ctrl+G', tr("search inside files (:grep)", 'ファイルの中を探す（:grep）')],
        [tr('  p on the results', '  結果で p'), tr("load the hits into a pane and use the ordinary keys on them", '一覧に読み込んで、いつものキーで操作する')],
        [tr('  r on the results', '  結果で r'), tr("replace across every matched file, confirming line by line", 'マッチした全ファイルを一括置換（1行ずつ確認）')],
        ['  Ctrl+N / Ctrl+Shift+N', tr("next / previous hit without closing the file", 'ファイルを開いたまま次 / 前のヒットへ')],
        ['b', tr("branch view: everything below here, one file per line (b / Esc leaves)", 'ブランチビュー：この配下を1ファイル1行に平坦化（b / Esc で戻る）')],
        ['h', tr("this pane's directory history (also :back)", 'このペインの移動履歴（:back でも）')],
        ['Z', tr("fuzzy-jump to a recent or bookmarked directory (also :jump)", '最近 / ブックマークのディレクトリへあいまいジャンプ（:jump でも）')],
        ['s', tr("the shortcuts menu", 'ショートカットメニュー')],
        [':bookmark', tr("bookmark where you are", 'いまの場所を登録する')],
        [tr("drop", 'ドラッグして落とす'), tr("from the desktop into a pane \u2014 it MOVES (it asks first)", 'デスクトップからペインへ ── 移動します（先に確認）')],
        ['Alt+← / Alt+→', tr("back / forward through this pane\u2019s history", '前 / 先のディレクトリへ')],
        [',', tr("sort by name / size / date / extension (n s d e go straight there; the same key reverses)", 'ソート：名前／サイズ／日時／拡張子（n s d e で直接、同じキーで昇降反転）')],
        ['T', tr("the switches: dotfiles, input sync, notifications\u2026 (also :toggle)", 'UIトグルメニュー：隠しファイル/入力同期/通知…（:toggle でも）')],
    ]],
    [tr("Commands", 'コマンド'), [
        [':', tr("type a command (:count :du :grep \u2026)", 'コマンドを打つ（:count :du :grep …）')],
        ['C  、Ctrl+Shift+P  、Ctrl+,', tr("the command palette: fuzzy-find any command", 'コマンドパレット：全コマンドをあいまい検索')],
        [':count', tr("count files and steps (the marks, or the whole tree)", 'ファイル・ステップ数を数える（マーク or ツリー全体）')],
        [':du', tr("disk usage \u2014 what is big here (Enter goes in)", '容量分析 — 何が大きいか（Enter で中へ）')],
        [':attr / :chmod / :readonly', tr("see and change the attributes", '属性を見る・変える')],
        [':hash', tr("checksum (sha256 by default; :hash md5 too)", 'チェックサム（既定 sha256、:hash md5 も）')],
        ['=  /  :diff', tr("compare left and right \u2014 two files line by line, two folders recursively", '左右を比較 — ファイル同士は行差分、ディレクトリ同士は再帰')],
        [tr('  Enter on a comparison', '  比較で Enter'), tr("open them side by side \u2014 both editable, Ctrl+S saves both", '並べて開く — 左右とも編集でき、Ctrl+S で両方保存')],
        ['  F7 / Shift+F7', tr("next / previous difference", '次 / 前の相違へ')],
        ['  > / <', tr("folder compare: copy that entry to the other side", 'ディレクトリ比較：そのエントリを反対側へコピー')],
        ['  c / w', tr("the comparison to the clipboard / to a file", '比較結果をクリップボードへ / ファイルに保存')],
        [':renamepattern', tr("bulk rename, {name}_{n3}.{ext} (the plan first)", '一括リネーム {name}_{n3}.{ext}（先にプレビュー）')],
        [':renamelist', tr("rename by editing the list of names (Ctrl+S applies)", '名前の一覧を編集してリネーム（Ctrl+S で適用）')],
        [':zip / :tar / :targz', tr("pack the marks into an archive", 'マークをアーカイブにまとめる')],
        [':unzip / :lsar', tr("extract here / list the contents", 'ここに展開 / 中身を見る')],
        [':log / :filelog', tr("the commit log / this file's history (git and svn)", 'コミットログ / このファイルの履歴（git・svn）')],
        [':gitdiff', tr("the selected file's diff", '選択ファイルの差分')],
        [':stage / :unstage / :discard', tr("git add / reset / discard the changes", 'git add / reset / 変更の破棄')],
        [':svnupdate :svncommit :svnresolve', tr("the three svn ones", 'svn の3つ')],
        [':dup', tr("find duplicate files \u2014 same contents (also :duplicate)", '重複ファイルを検出 — 中身が同じもの（:duplicate でも）')],
        [':df / :wc / :stat', tr("free space / lines, words, bytes / attributes", '空き容量 / 行・単語・バイト / 属性')],
        [':mark *.rs  :unmark *', tr("mark by wildcard", 'ワイルドカードでマーク')],
        [':copyto / :moveto', tr("somewhere other than the opposite pane", '反対ペイン以外の場所へ')],
        [':edit', tr("open in the external editor ($EDITOR)", '外部エディタ（$EDITOR）で開く')],
        [':where', tr("where the config files actually are", '設定ファイルがどこにあるか')],
        [':key', tr("report each key as received (for a key that does nothing)", '押したキーをそのまま表示（効かないキーの調査に）')],
        [':reload', tr("re-read init.lua", 'init.lua を読み直す')],
        [':office / :officelink', tr("open the cloud copy of an Office document / write a .url to it", 'Office 文書のクラウド側を開く / .url を作る')],
    ]],
    [tr("Servers (SFTP)", 'サーバ（SFTP）'), [
        ['Shift+S', tr("the SSH picker \u2014 the hosts in init.lua\u2019s cian.ssh", 'SSHピッカー — init.lua の cian.ssh から選ぶ')],
        [':remote  /  :ssh', tr("or type one \u2014 user@host[:port][:/path]", '手で打つなら — user@host[:port][:/path]')],
        ['Enter / Backspace', tr("move around on the server", 'サーバの中を移動')],
        ['c', tr("to the other pane \u2014 which side you stand on decides upload or download", '反対ペインへ — 立っている側でアップロードか転送かが決まる')],
        [tr("a / A / r / d", 'a / A / r / d'), tr("the same keys on the server (a delete there has no trash \u2014 it is gone)", 'サーバ上でも同じキー（削除はゴミ箱なし＝戻せません）')],
        ['Enter / F3', tr("open a file on the server \u2014 Ctrl+S writes it back there", 'サーバのファイルを開く — Ctrl+S でサーバへ書き戻す')],
        [tr("Ctrl+V / a drop", 'Ctrl+V / ドロップ'), tr("upload a local file", 'ローカルのファイルをアップロード')],
        [':local', tr("close the server and come back to this disk", 'サーバを閉じてローカルへ戻る')],
        [tr("the frame changes", '枠が変わります'), tr("a pane showing a server wears a different colour of frame", 'サーバを表示しているペインは色の違う枠になります')],
    ]],
    [tr("AI (when init.lua configures it)", 'AI（init.lua で設定したとき）'), [
        [tr(":aicmd <description>", ':aicmd 説明'), tr("a shell command from a description \u2014 it is placed, never run", '説明からシェルコマンド生成 ── 置くだけで、実行はしません')],
        [':ailog', tr("triage the selected log (errors, likely cause, what to check next)", '選択中のログを診断（エラー・原因・次の確認）')],
        [':aijunk / :aistructure', tr("detect junk / suggest a folder structure (no contents are sent; the plan is shown first)", 'ゴミファイル検出 / ディレクトリ構成を提案（中身は送らない・実行前に全部見せる）')],
        [tr(":airename <instruction>", ':airename 指示'), tr("rename by instruction (e.g. :airename to snake_case)", '指示でリネーム（例 :airename snake_case に）')],
        [tr(":aisearch <what>", ':aisearch 探しもの'), tr("semantic search \u2014 find by meaning", 'セマンティック検索 — 意味で探す')],
        [':aierror  、:explain', tr("explain the shell's last error", 'シェルの直近のエラーを説明')],
        [':aicommit', tr("a commit message from the staged diff (Enter signs it)", 'ステージ済み差分からコミットメッセージ（Enter で署名）')],
        [':ime', tr("switch the input method off in vim's normal mode (init.lua's cian.ime)", 'vim のノーマルモードで IME を自動オフ（init.lua の cian.ime）')],
        [tr("  with the IME on", '  IME オンのまま'), tr("the listing keys still work \u2014 this build reads the physical key, so no helper is needed", '一覧のキーはそのまま効きます — 窓版は物理キーを読みます（ヘルパー不要）')],
        [tr(":ai <question>", ':ai 質問'), tr("AI - simple: chat with the local model", 'AI - simple: ローカルモデルとチャット')],
        [':aidiff', tr("explain the diff on screen (x on the comparison)", '表示中の差分を説明（差分画面で x）')],
    ]],
    [tr("Shell panel", 'シェル'), [
        ['Shift+J  /  :shell', tr("the shell panel (it lives in the lower half)", 'シェルパネル（下半分に出る）')],
        ['Esc', tr("back to the files (twice hands Esc to the shell)", 'ファイルへ戻る（Esc 2回でシェルへ渡る）')],
        ['Shift+PgUp / PgDn', tr("back through the output that scrolled past", '流れた出力を遡る')],
        [tr(":!command", ':!コマンド'), tr("run in the shell \u2014 % the selection, %f the file, %d the folder", 'シェルで実行 — % 選択、%f ファイル、%d ディレクトリ')],
        ['Ctrl+Shift+Enter / :snip', tr("a saved command, sent to the shell (cian.snippets)", '保存したコマンドを選んでシェルへ（cian.snippets）')],
        [':vi / :vim / :nvim', tr("open the file in that editor, in a shell tab of its own", 'そのエディタを新しいシェルタブで開く')],
        [tr(":each command", ':each コマンド'), tr("run once per marked file \u2014 {} is the path", 'マーク各ファイルに実行 — {} がパス')],
        ['F9 / F10', tr("open / close a shell tab (while the panel has the keys)", 'シェルのタブを開く / 閉じる（パネルにいるとき）')],
        ['F1 / F2', tr("previous / next shell tab", '前 / 次のシェルタブ')],
        ['Shift+F8 / Shift+F9', tr("split left-right / top-bottom", '左右 / 上下に分割')],
        ['Shift+F10', tr("close the split pane (it asks)", '分割パネルを閉じる（確認あり）')],
        ['Shift+F1 / Shift+F2', tr("previous / next pane", '前 / 次のペインへ')],
        ['F1-F8', tr("go straight to shell tab 1-8", 'シェルタブ 1-8 に切替')],
        [tr("Ctrl+Shift+arrows", 'Ctrl+Shift+矢印'), tr("move the dividers", '分割の境界を動かす')],
        ['Ctrl+S  /  :sync', tr("type into every pane at once (one command, four machines)", '全ペインに同時入力（同じコマンドを4台へ）')],
        ['F12  /  :zoom', tr("zoom whichever surface has the keys", 'フォーカス中の面をズーム（トグル）')],
        ['Shift+F12', tr("show only this pane / back to the split", 'いまのペインだけを表示／分割に戻す')],
        [':sessionlog', tr("record the shell to a file (again stops it)", 'シェルの写しをファイルに取る（もう一度で止める）')],
        [tr("drag to select", 'ドラッグで選択'), tr("it is on the clipboard the moment you let go", '放した瞬間にクリップボードへ')],
        [':preview', tr("follow the cursor and show what it is on (again stops)", 'カーソルのファイルを追って表示（もう一度で止める）')],
        ['@  /  :macro', tr("run a macro \u2014 it splits and opens the layout it describes", 'マクロを実行 ── レイアウトどおりに分割して開きます')],
    ]],
    [tr("Reading and writing (F3 / Enter)", '読み書き（F3・Enter）'), [
        [tr("images, PDFs", '画像・PDF'), tr("F3 or Enter shows it as it is (with its dimensions)", 'F3 か Enter でそのまま表示（寸法も出ます）')],
        [tr("a binary", 'バイナリ'), tr("shown in hex; i edits \u2014 0-9 a-f overwrite, Ctrl+S saves (keeping a .bak)", '16進で表示。i で編集 — 0-9 a-f で上書き、Ctrl+S 保存（.bak を残す）')],
        [tr("  overwrite only", '  上書きのみ'), tr("nothing shifts, so the file cannot change size", 'ずれないので、ファイルの大きさは変わりません')],
        ['Ctrl+S', tr("save (same encoding, same line endings, same BOM)", '保存（元の文字コード・改行・BOM のまま）')],
        ['Esc ×3', tr("close \u2014 three in a row (unsaved, the third asks)", '閉じる ── 3回連続（未保存なら3回目で確認）')],
        ['Backspace ×3', tr("the same, in vim style and only in normal mode", '同じ。vim でノーマルモードのときだけ')],
        ['F3', tr("closes on one press", '1回で閉じる')],
        [tr("F3 with marks", 'F3（マーク中）'), tr("opens every marked file", 'マークした全部を開く')],
        ['F2 / Shift+F2', tr("next / previous open file", '次 / 前の開いているファイル')],
        ['Ctrl+Shift+O', tr("jump by heading (:outline in vim style)", '見出し一覧から飛ぶ（vim では :outline）')],
        ['Ctrl+Shift+B', tr("who last changed each line (:blame in vim style; again hides it)", '各行を最後に変えた人（vim では :blame、もう一度で消す）')],
        [tr("\u2500\u2500 the rest are typed at the command line in vim style \u2500\u2500", '── 以下は vim のキー操作のコマンド行から ──'), tr("set \u201cEditor keys\u201d to vim in T's menu", 'T のメニューで「エディタのキー操作」を vim に')],
        [':sort :rsort :uniq', tr("sort the lines / reverse / drop duplicates", '行をソート / 逆順 / 重複を落とす')],
        [tr(":s/old/new/g", ':s/古い/新しい/g'), tr("replace in the open file", '開いているファイルを置換')],
        [':han :zen', tr("full-width ASCII \u2192 half / half-width kana \u2192 full", '全角ASCII→半角 / 半角カナ→全角')],
        [':expand :unexpand :reindent', tr("tabs \u2194 spaces, and a consistent indent", 'タブ↔スペース、インデントを揃える')],
        [':lf :crlf', tr("change the line endings (written on save)", '改行コードを変える（保存時に反映）')],
        // cian-tui's words for this pair, and its default: vim is what cian
        // was built around, メモ帳 is the one you hand to a colleague.
        [tr("Editor keys", 'エディタのキー操作'), tr("vim (the default) / notepad \u2014 back in the listing, inside T\u2019s menu (:editstyle vim / :notepad too)", 'vim（既定）／ メモ帳 ── 一覧に戻って T のメニューの中（:editstyle vim / :notepad でも）')],
        [tr("  in vim style", '  vim のとき'), tr("opens in normal mode. :w saves, :q closes, :wq both", 'ノーマルモードで開く。:w 保存 :q 閉じる :wq 両方')],
        ['  % ', tr("to the matching bracket (monaco-vim's)", '対応する括弧へ（monaco-vim のもの）')],
        ['  ]] / [[', tr("next / previous heading", '次 / 前の見出しへ')],
        ['  za', tr("fold and unfold", '折り畳む・開く')],
        ['  :enc', tr("re-read under another encoding (no argument cycles)", '文字コードを変えて読み直す（引数なしで順に）')],
        ['  :ws / :ruler', tr("the invisible characters / the column ruler", '見えない文字 / 桁の目盛り')],
        ['Ctrl+E', tr("set the Markdown / back to the source (:render, or :preview in vim style)", 'Markdown を組んで表示 / ソースへ戻る（:render・vim では :preview）')],
        [tr("  :s/old/new/g", '  :s/古い/新しい/g'), tr("replace in this file", 'このファイルを置換')],
        ['  :g/re/d  :v/re/d', tr("delete the matching lines / keep only them", '一致した行を削除 / 一致した行だけ残す')],
        ['  :combine [n][!]', tr("join the next line (! without a space)", '次の行を連結（! は空白なし）')],
        [tr("rectangle", '矩形'), tr("Alt+Shift+arrows selects; Alt+Shift+I/A/C/D for left edge / right edge / replace / delete", 'Alt+Shift+矢印 で選び、Alt+Shift+I/A/C/D で 左端/右端/置換/削除')],
        ['Ctrl+] / Ctrl+[', tr("move by heading (works in notepad style too)", '見出し移動（メモ帳のキー操作でも使えます）')],
        [tr("  in notepad style", '  メモ帳のとき'), tr("Ctrl+C/V/Z/F and the rest of the Windows hand", 'Ctrl+C/V/Z/F など Windows の手が効く')],
        ['jj  /  ｊｊ  /  っｊ', tr("leave insert mode — the last two are what a Japanese IME makes of pressing j twice", '挿入モードを抜ける ── 後ろ2つは、IME オンで j を2回押したときに出るもの')],
        ['ZZ  /  ZQ', tr("save and close / close without saving", '保存して閉じる ／ 保存せずに閉じる')],
    ]],
    [tr("Marks and file operations", 'マークと操作'), [
        ['Space', tr("toggle the mark and step down", 'マーク切替して下へ')],
        ['Shift+Space', tr("toggle the mark and step up", 'マーク切替して上へ')],
        ['v', tr("visual selection (Enter confirms, Esc cancels)", 'ビジュアル選択（Enter 確定・Esc 取消）')],
        [':nobom', tr("strip UTF-8 BOMs (UTF-16 is left alone)", 'UTF-8 BOM を除去（UTF-16 は触らない）')],
        ['Ctrl+A  、:markall', tr("mark everything here \u2014 in the viewer, select the whole file", 'ここにある全部をマーク — ビューアではファイル全体を選択')],
        ['V', tr("invert the marks", '全マークを反転')],
        ['c / m / d', tr("copy / move to the other pane / delete (to the trash)", '反対ペインへコピー / 移動 / 削除（ゴミ箱へ）')],
        ['Ctrl+C / Ctrl+X', tr("hold the files (copy / cut)", 'ファイルを保持（コピー / 切り取り）')],
        ['Ctrl+V / y', tr("paste the held files here", '保持したファイルをここへ貼り付け')],
        ['r', tr('Rename', 'リネーム')],
        ['a / A', tr("new file / new folder", '新規ファイル / 新規ディレクトリ')],
        ['p', tr("the path, as text, to the clipboard", 'パス文字列をクリップボードへ')],
        ['Shift+P', tr("the file itself to the clipboard (Finder and Explorer can paste it)", 'ファイルそのものをクリップボードへ（Finder/エクスプローラで貼れます）')],
        [tr("drag a row", '行をドラッグ'), tr("to the desktop or another application \u2014 the file itself (the terminal build cannot do this)", 'デスクトップや他のアプリへ、ファイルそのものを渡します（端末版にはできません）')],
        // **Both halves were the wrong way round.** `o` brings the *other*
        // pane's directory here and `O` sends this one across (keys.rs:2604,
        // and cian-tui's own help says so in as many words) — the window's
        // one-line version had it backwards, so pressing `o` expecting to
        // push did the opposite and read as a key that was not implemented.
        // Written out as two rows now, in the terminal build's own words,
        // because the pair is exactly where a single line invites a mix-up.
        ['o', tr("this pane → the other pane’s directory", 'このペインを反対ペインと同じ場所に')],
        ['O', tr("the other pane → this pane’s directory", '反対ペインをこのペインと同じ場所に')],
        ['u / Ctrl+z', tr("undo the last operation", '直前の操作を取り消す')],
        ['Ctrl+r / Ctrl+Shift+z', tr("redo it", 'やり直す')],
        [tr("M / Shift+Enter / right-click", 'M / Shift+Enter / 右クリック'), tr("what can be done to this entry", 'このエントリにできること')],
        ['Esc', tr("clear marks and filter \u2192 then stop what is running", 'マーク・フィルタ解除 → 実行中の操作を中止')],
        [':queue', tr("what is running \u2014 x stops one of them", '実行中の操作を見る — x で1つだけ止める')],
    ]],
    // The window and how it looks. Absent until 2026-08-31, which is why the
    // first person to run this asked whether themes could be chosen at all —
    // twenty-one of them, and `?` did not say the word once.
    [tr("The window and how it looks", '窓と見た目'), [
        [tr(":theme  /  \u201cTheme\u201d in T\u2019s menu", ':theme  /  T のメニューの「配色」'), tr("twenty-one palettes \u2014 \u2191\u2193 dresses the window as you pass", '配色 21 種 ── ↑↓ で選ぶだけで着せ替わります')],
        [tr(":theme <name>", ':theme 名前'), tr("straight to one by name (dracula, nord, solarized-light \u2026)", '名前で直に（dracula, nord, solarized-light …）')],
        ['F11', tr("full screen, and back", '全画面／戻す')],
        ['F12', tr("zoom whichever surface has the keys (files or shell)", 'キーのある面を広げる／戻す（ファイルでもシェルでも）')],
        ['Ctrl+= / Ctrl+- / Ctrl+0', tr("bigger / smaller / back", '文字を大きく / 小さく / 戻す')],
        [tr("Ctrl+Shift+arrows", 'Ctrl+Shift+矢印'), tr("move the pane divider (dragging it works too)", 'ペインの境界を動かす（境界のドラッグでも）')],
        [':where', tr("where the config files being read actually are", 'いま読んでいる設定ファイルの場所')],
        [':version', tr("which build this is \u2014 the first thing to ask when a fix seems not to have landed", 'いま動いている版 ── 直らないときは、まずこれ')],
    ]],
  ];
}

const help = { on: false };

function openHelp() {
    help.on = true;
    el.find.hidden = false;
    el.find.classList.add('help');
    el.findFoot.textContent = tr('Esc or ? closes  ── the same keys as cian in a terminal', 'Esc か ? で閉じる  ── 端末版の cian と同じキーです');
    const frag = document.createDocumentFragment();
    for (const [group, rows] of helpRows()) {
        const h = document.createElement('div');
        h.className = 'group';
        h.textContent = group;
        frag.append(h);
        for (const [keys, what] of rows) {
            const div = document.createElement('div');
            div.className = 'hit';
            const l = document.createElement('span');
            l.className = 'k';
            l.textContent = keyLabel(keys);
            const v = document.createElement('span');
            v.className = 'w';
            v.textContent = what;
            div.append(l, v);
            frag.append(div);
        }
    }
    el.findHits.replaceChildren(frag);
    el.findHits.scrollTop = 0;
}

function closeHelp() {
    help.on = false;
    el.find.classList.remove('help');
    el.find.hidden = true;
}

/// Help's keys. It scrolls, because the terminal build's help did not and
/// the bottom of it could not be read.
document.addEventListener('keydown', (e) => {
    if (!help.on) return;
    e.stopPropagation();
    if (e.key === 'Escape' || e.key === '?') closeHelp();
    else if (e.key === 'ArrowDown' || e.key === 'j') el.findHits.scrollTop += 40;
    else if (e.key === 'ArrowUp' || e.key === 'k') el.findHits.scrollTop -= 40;
    else if (e.key === 'PageDown' || e.key === ' ') el.findHits.scrollTop += el.findHits.clientHeight - 40;
    else if (e.key === 'PageUp') el.findHits.scrollTop -= el.findHits.clientHeight - 40;
    else if (e.key === 'g') el.findHits.scrollTop = 0;
    else if (e.key === 'G') el.findHits.scrollTop = el.findHits.scrollHeight;
    else return;
    e.preventDefault();
}, true);

function focusPane(which) {
    state.focus = which;
    draw('left');
    draw('right');
}

async function invert() {
    const which = state.focus;
    const pane = await ask('invert', { pane: which });
    if (!pane) return;
    state[which] = pane;
    draw(which);
    say(pane.marked ? tr(`${pane.marked} marked`, `${pane.marked} 件をマーク`) : tr('nothing marked', 'マークなし'));
}

/// `o` brings this pane to the other one; `O` sends the other one here.
///
/// The pair is easy to get backwards, so the message names the direction
/// rather than saying "done" — the same reason `u` names what it undid.
async function syncPane(pullToHere) {
    const here = state.focus;
    const there = here === 'left' ? 'right' : 'left';
    const [to, from] = pullToHere ? [here, there] : [there, here];
    const path = state[from].cwd;
    // Said and stopped, as cian-tui says and stops (`sync_active_from_other`).
    // A move to where you already are is not a move: it used to re-list, and
    // the engine's `list` then rebuilt the pane, so `o` on two panes already
    // together threw that pane's history away and put its own directory at
    // the top of what was left.
    if (state[to].cwd === path) {
        say(tr('panes already in the same directory', '両ペインは既に同じディレクトリです'));
        return;
    }
    const pane = await ask('list', { pane: to, path });
    if (!pane) return;
    state[to] = pane;
    draw(to);
    say(tr(`${to === 'left' ? 'left' : 'right'} to ${path}`, `${to === 'left' ? '左' : '右'}を ${path} へ`));
}

async function goToPath(given) {
    const path = given || await askFor(tr('where to', '移動先'), state[state.focus].cwd, {
        wide: true,
        hint: tr('a path — ~ and environment variables are expanded', 'パス — ~ や環境変数も展開します'),
    });
    if (!path) return;
    const which = state.focus;
    const pane = await ask('list', { pane: which, path });
    if (!pane) return;
    state[which] = pane;
    draw(which);
    say(pane.cwd);
}

/// `p` puts the paths on the clipboard — the marked ones, or the one under
/// the cursor. The text, not the files: copying the files is Ctrl+C, and
/// conflating the two is how you paste a path into a folder.
async function copyPaths() {
    const pane = state[state.focus];
    const marked = pane.entries.filter((x) => x.marked);
    const rows = marked.length ? marked : [pane.entries[pane.cursor]].filter(Boolean);
    if (!rows.length) return;
    const text = rows.map((x) => x.path).join('\n');
    await navigator.clipboard.writeText(text);
    say(tr(`${rows.length} paths copied`, `${rows.length} 件のパスをコピー`));
}

/// F5 goes back to the disk. `refresh()` above asks the engine what it
/// already holds, which is right at startup and wrong here — the point of
/// the key is that something changed underneath us.
/// Hold the selection for a later paste.
///
/// The pair to `c`/`m`, which go straight to the other pane. This one is for
/// when the destination is not on screen yet: hold here, walk there, paste.
/// The Windows letters, because that is what the hands do.
async function hold(op) {
    const r = await ask('clip', { pane: state.focus, op });
    if (!r) return;
    say(tr(`${r.held} ${r.op === 'cut' ? 'cut' : 'copied'}`, `${r.held} 件を${r.op === 'cut' ? '切り取り' : 'コピー'}`));
}

async function paste() {
    const r = await ask('paste', { pane: state.focus });
    if (!r) return;
    // The engine decides whether this is a copy or a move — it is holding the
    // register — so the verb comes back with the job rather than being
    // guessed here from which key was pressed.
    const verb = r.kind === 'move' ? tr('move', '移動') : tr('copy', 'コピー');
    beginOp(r, r.kind, verb);
}

async function reread() {
    for (const which of ['left', 'right']) {
        const pane = await ask('list', { pane: which, path: state[which].cwd });
        if (!pane) return;
        state[which] = pane;
        draw(which);
    }
    say(tr('reloaded', '読み直しました'));
}

/// The filter's keys, while it is up.
document.addEventListener('keydown', (e) => {
    if (!filter.on) return;
    // **Immediate**, not `stopPropagation`. The two are not the same thing:
    // `stopPropagation` stops the event reaching further *nodes*, and every
    // one of these handlers is on `document`, so the listing's keys ran on
    // the same keystroke regardless. It did not show while a prompt only ever
    // closed itself — but a `:` command that opens a list (`:ssh`, `:recent`)
    // opens it *during* this handler, and the listing's Enter then picked the
    // first row of the list that had just appeared. You typed the command,
    // were shown nothing, and landed on whichever row happened to be first.
    // (Found on `:notes`, back when amber mode was in this window.)
    // While a prompt is up it owns the keyboard; this is that sentence.
    e.stopImmediatePropagation();
    const k = e.key;
    const mode = filter.mode;
    if (k === 'Escape') {
        if (mode === 'filter') { endFilter(false); say(tr('filter cleared', '絞り込みを解除')); }
        else if (mode === 'find') { closeFinder(); say(tr('stopped', 'やめました')); }
        else { closePrompt(); say(tr('stopped', 'やめました')); }
    }
    else if (k === 'Enter') {
        if (mode === 'filter') endFilter(true);
        else if (mode === 'find') goToHit();
        else { const line = el.fInput.value; closePrompt(); runTypedCommand(line); }
    }
    else if (k === '/' && mode === 'filter' && el.fInput.value === '') {
        // Two slashes: this listing was not it, so look underneath.
        endFilter(true);
        openFinder();
    }
    // The cursor still walks while the box is open — the terminal build's
    // filter mode does the same, and it is what makes "type three letters,
    // arrow down, Enter" one motion. In the finder the arrows walk the hits.
    else if (k === 'ArrowDown' || (mod(e) && k === 'n')) {
        if (mode === 'find') { finder.at = Math.min(finder.rows.length - 1, finder.at + 1); drawHits(finder.rows.length); }
        else if (mode === 'filter') move(1);
        else return;
    }
    else if (k === 'ArrowUp' || (mod(e) && k === 'p')) {
        if (mode === 'find') { finder.at = Math.max(0, finder.at - 1); drawHits(finder.rows.length); }
        else if (mode === 'filter') move(-1);
        else return;
    }
    else return;
    e.preventDefault();
}, true);

document.addEventListener('input', (e) => {
    if (!filter.on || e.target !== el.fInput) return;
    if (filter.mode === 'filter') applyFilter(el.fInput.value);
    else if (filter.mode === 'find') rankNow();
});

// The finder's keys are the prompt row's now — it types there like the other
// two, so a second handler for the same keystrokes would be a second answer.

// Each keystroke re-ranks. Not debounced: the answer comes from a pipe, and
// waiting on a timer to save a round trip that costs nothing would only make
// the picker feel slower than it is.


/// While the bar is up it owns the keyboard — two keys, the terminal build's
/// (keys.rs, the progress popup): Esc stops the work, `b` stops only the
/// screen. Registered before the listing's handler so neither reaches it.
document.addEventListener('keydown', (e) => {
    if (!running || prog.hidden) return;
    if (e.key === 'Escape') {
        e.stopPropagation();
        e.preventDefault();
        window.cian.call('cancel', { op: running.op });
        say(tr('stopping…', '中止しています…'));
        return;
    }
    if (e.key === 'b' || e.key === 'Enter') {
        e.stopPropagation();
        e.preventDefault();
        prog.hidden = true;
        drawProg();
        say(tr('running in the background — :queue manages it', 'バックグラウンドで実行中 — :queue で管理'));
        return;
    }
    // Everything else is swallowed rather than passed on. A `d` typed at a
    // bar is a `d` meant for the listing behind it, and that one deletes.
    e.stopPropagation();
    e.preventDefault();
}, true);

/// The 49 action names `cian.set_keymap` accepts, each pointing at what this
/// build already does for that key.
///
/// The terminal build resolves the same names out of the same init.lua; a
/// binding that worked in one and not the other would be two programs wearing
/// one name. `unbind` is the one that does nothing on purpose — it exists so
/// a key can be made to shadow its own default.
const ACTIONS = {
    cursor_down: () => move(1),
    cursor_up: () => move(-1),
    cursor_top: () => jumpTo(0),
    cursor_bottom: () => jumpTo(state[state.focus].entries.length - 1),
    page_up: () => move(-20),
    page_down: () => move(20),
    parent: () => parent(),
    enter: () => enter(),
    quit: () => cmdQuit(),
    search: () => searchHere(),
    search_next: () => hopHere(1),
    search_prev: () => hopHere(-1),
    history: () => cmdHistory(),
    shortcuts: () => cmdShortcuts(),
    copy: () => operate('copy'),
    move: () => operate('move'),
    paste: () => paste(),
    cut: () => hold('cut'),
    delete: () => operate('delete'),
    rename: () => rename(),
    new_file: () => create(false),
    new_dir: () => create(true),
    open_other: () => openOut(),
    open_other_tab: () => tabNew(),
    sync_from_other: () => syncPane(true),
    sync_to_other: () => syncPane(false),
    // One function, because Ctrl+Enter is one key with two answers here: a
    // folder to the other pane, a file to your own application. Both names
    // land on it, and which half happens is decided by what is under the
    // cursor — as it is in the terminal build.
    open_external: () => openOut(),
    copy_path: () => copyPaths(),
    copy_file_ref: () => clipFiles(),
    mark_down: () => mark(false),
    mark_up: () => mark(false, -1),
    invert_marks: () => invert(),
    select_all: () => mark(true),
    visual: () => startVisual(),
    command: () => commandLine(),
    filter: () => startFilter(),
    find_recursive: () => runCommand(findCommand('find'), ''),
    grep_recursive: () => runCommand(findCommand('grep'), ''),
    sort: () => openMenu(SORT_MENU),
    jump_path: () => goToPath(),
    view: () => lookInsideAll(),
    diff: () => cmdCompare(),
    refresh: () => reread(),
    menu: () => openMenu(CONTEXT),
    ssh: () => cmdSshPicker(),
    new_tab: () => tabNew(),
    close_tab: () => tabClose(),
    manual: () => openHelp(),
    unbind: () => {},
};

/// What init.lua bound, keyed the way a keydown arrives: "ctrl+alt+x".
const bound = new Map();

/// `cian.set_keymap("alt+g", …)` → the string a keydown makes.
///
/// The terminal build's own spec parser, in the terminal build's order:
/// modifiers before the key, `shift` folded into an upper-case letter rather
/// than carried as a flag — because that is what a terminal actually
/// delivers, and a window that disagreed would need its own documentation.
function keySpec(spec) {
    const parts = String(spec).trim().split('+');
    let key = parts.pop();
    if (!key || [...key].length !== 1) return null;
    let ctrl = false;
    let alt = false;
    for (const m of parts) {
        const w = m.trim().toLowerCase();
        if (w === 'ctrl' || w === 'control' || w === 'c') ctrl = true;
        else if (w === 'alt' || w === 'opt' || w === 'option' || w === 'meta' || w === 'm') alt = true;
        else if (w === 'shift' || w === 's') key = key.toUpperCase();
        else return null;
    }
    return (ctrl ? 'ctrl+' : '') + (alt ? 'alt+' : '') + key;
}

/// **Ctrl と ⌘ は同じ修飾キー。**
///
/// 「Windows で Ctrl+なにか としているショートカットを、Mac では
/// Meta+なにか と一律読み替えて」── そのとおりで、`Ctrl+C` と `⌘C` は
/// 二つの鍵ではなく、二つの機械が同じ鍵に付けている名前です。
/// 23 箇所は既に両方を見ていて、残りが Ctrl だけを見ていた ── つまり
/// Mac では `⌘h` でペインが移らず、`Ctrl+n` だけが下に動いた。ばらついた
/// のは、site ごとに書いていたから。**一つの関数にすれば、次に足すものも
/// 迷わない。**
///
/// **シェルパネルだけは別。** そこでの Ctrl は端末の制御文字（Ctrl+C は
/// SIGINT）で、Mac の ⌘C は「コピー」です。名前が同じでも別のものなので、
/// `shellBytes` は `ctrlKey` だけを見ます。
function mod(e) {
    return e.ctrlKey || e.metaKey;
}

/// キーの見せ方。Mac では `Ctrl+` を `⌘` と書く ── 押す鍵は同じでも、
/// 手元のキーボードに書いてある文字は違う。
const MAC = navigator.platform.startsWith('Mac');
function keyLabel(k) {
    return MAC ? String(k).replace(/Ctrl\+/g, '⌘') : String(k);
}

function pressSpec(e) {
    return (e.ctrlKey || e.metaKey ? 'ctrl+' : '') + (e.altKey ? 'alt+' : '') + e.key;
}

/// `:key` — 何が届いているかを見る。**どこにいても。**
///
/// これは一覧のキー処理の中にあった。つまり**一覧でしか効かなかった** ──
/// エディタでは viewer の capture が先に止め、シェルではシェルの capture が
/// 先に止めるので、いちばん知りたい二つの場所で答えが出なかった。
/// 「押しても何も起きない」を追うための道具が、押しても何も起きない場所で
/// 使えない、というのは道具のほうの穴。
///
/// **いちばん早い capture** に置く。同じ node の capture は登録順なので、
/// ファイルのここ（viewer は 4600 台、シェルは 8700 台）に置けば先に届く。
/// ダイアログの listener だけは開いた時点で足されるので後になるが、
/// ダイアログが開いているときにキーを調べたい場面は無い。
document.addEventListener('keydown', (e) => {
    if (!keyEcho.on) return;
    // 飲み込む。この機能は「何も起きないキー」を試すためのもので、最初に
    // 試されるのは大抵は和音 ── その半分は切り取り・削除・上書きをする。
    // Ctrl+X を表示して**かつ**ファイルを切り取るのは、最悪の答え方。
    if (e.key === 'Escape') { toggleKeyEcho(); return; }
    e.stopImmediatePropagation();
    e.preventDefault();
    const bits = [
        e.ctrlKey && 'Ctrl', e.altKey && 'Alt', e.shiftKey && 'Shift', e.metaKey && 'Meta',
    ].filter(Boolean);
    // どこで押されたかも言う ── 同じキーが場所によって別の扱いを受けるので、
    // 「どこで」が抜けていると答えが半分になる。
    // どこで押されたか **と、その面がいまどういう状態か**。
    //
    // `code=Slash` まで出ているのに `?` がキー一覧を開かない、という報告が
    // あった ── キーは届いていて、残る条件は「エディタが vim の流儀か」と
    // 「挿入モードでないか」の二つだけ。それを言わない限り、一往復ぶん
    // 当て推量が要る。**押されたキーの話だけでは足りない。**
    let where;
    if (viewer.on) {
        // `tr()` をテンプレート literal の中に書かない。中身は訳されて
        // いても、外側の literal は日本語を含む一つの文字列に見えるので、
        // scripts/i18n.py が「まだ日本語だけ」と数える ── 検査のほうを
        // 黙らせるより、書き方を変えるほうが安い。
        const surface = tr('editor', 'エディタ');
        where = `${surface}/${STYLES[style][0]}`;
        if (viewer.vim) where += vimTyping() ? tr('/typing', '/入力中') : tr('/normal', '/ノーマル');
        if (hex.editing) where += tr('/hex', '/16進');
    } else if (term.on && term.focused) {
        where = tr('shell', 'シェル');
    } else {
        where = tr('listing', '一覧');
    }
    say(`[${where}] ${[...bits, e.key].join('+')}   code=${e.code}   keyCode=${e.keyCode}`
        + tr('   — Esc stops it', '   — Esc で止める'));
}, true);

/// Take what init.lua bound. Names that are not actions and keys that are not
/// keys are said out loud — a binding that silently does nothing is worse than
/// no binding, because the person goes looking in the wrong place.
function applyKeymaps(list) {
    bound.clear();
    const bad = [];
    for (const { key, action } of list || []) {
        const spec = keySpec(key);
        if (!spec) { bad.push(tr(`${key} (not a key I can read)`, `${key}（キーとして読めません）`)); continue; }
        if (!ACTIONS[action]) { bad.push(tr(`${action} (no such action)`, `${action}（そんな動作はありません）`)); continue; }
        bound.set(spec, action);
    }
    keymapErrors = bad;
}

/// What was wrong with the bindings, held until the opening line is out of
/// the way. A config error that scrolls past in 200ms is a config error
/// nobody sees, and a binding that silently does nothing sends the person
/// looking in the wrong place.
let keymapErrors = [];

document.addEventListener('keydown', (e) => {
    // The chat first, because it opens *over* the viewer (summarise the file
    // you are reading) as well as over the listing.
    //
    // Handled here rather than on the textarea, deliberately. A listener on
    // the node would be a second receiver on the same path, and this program
    // has lost four rounds to that: `stopPropagation` silences other *nodes*
    // and not the other handlers on `document`, and a capture listener
    // silences the bubble phase on the same node. One receiver, one order.
    if (chat.on) { chatKey(e); return; }
    // Not while a file is open. The editor no longer stops every key on its
    // way past — it cannot, or its own bindings never fire — so the listing's
    // keys have to decline for themselves.
    if (viewer.on) return;
    // **And not before there is a listing to steer.**
    //
    // Twenty-seven branches below read a field off a pane — `.remote`,
    // `.entries`, `.cwd` — and every one of them was a crash waiting for the
    // moment a pane is null: the first keystroke before the engine has
    // answered, and the gap while one is being replaced. `Backspace` found it
    // in the standard round. One guard here rather than twenty-seven, because
    // the assumption is the same in all of them: a key aimed at a listing
    // needs a listing.
    if (!state.left || !state.right || !state[state.focus]) return;
    // What init.lua bound comes before what cian ships: rebinding a key is
    // saying "not the default", and a default that still fired would make the
    // binding a suggestion.
    const mine = bound.get(pressSpec(e));
    if (mine) {
        e.preventDefault();
        e.stopPropagation();
        ACTIONS[mine]();
        return;
    }
    // cian's own keys first; anything not claimed here is left to Chromium,
    // which is what makes Ctrl+C and friends work without being written out.
    const k = e.key;
    // Every bare letter is guarded with !ctrl && !meta. Four chords in this
    // chain were dead because their plain letter matched first — and Ctrl+D
    // *deleted*, because `d` did not care about its modifiers. An unclaimed
    // chord now falls through to the report at the bottom instead of quietly
    // running the letter it happens to contain.
    const bare = !e.ctrlKey && !e.metaKey;
    // The dividers, and *first*: the plain arrows below carry no modifier
    // guard, so tested later this key would have moved the cursor instead —
    // the same shape as the four chords that were dead in this chain before.
    if (mod(e) && e.shiftKey && k.startsWith('Arrow')) resizeSplit(k);
    else if (k === 'ArrowDown' || (k === 'j' && bare)) move(1);
    else if (k === 'ArrowUp' || (k === 'k' && bare)) move(-1);
    else if (k === 'PageDown') move(20);
    else if (k === 'PageUp') move(-20);
    else if (k === 'D' && bare) move(10);
    else if (k === 'U' && bare) move(-10);
    else if (k === 'G' && bare) jumpTo(state[state.focus].entries.length - 1);
    else if (k === 'g' && bare) {
        // `gg`, two keystrokes and therefore a small state machine — a lone
        // `g` means nothing here, as in vim.
        const now = performance.now();
        if (now - lastGG < 1000) { lastGG = 0; jumpTo(0); }
        else lastGG = now;
    }
    else if (k === ' ' && e.shiftKey && bare) mark(false, -1);
    else if (k === 'Home') jumpTo(0);
    else if (k === 'End') jumpTo(state[state.focus].entries.length - 1);
    // Shift+H / Shift+L cross the panes, as in the terminal build.
    else if (k === 'H' && bare) focusPane('left');
    else if (k === 'L' && bare) focusPane('right');
    else if (k === 'ArrowLeft' && !e.altKey) focusPane('left');
    else if (k === 'h' && mod(e)) focusPane('left');
    else if (k === 'ArrowRight' && !e.altKey) focusPane('right');
    else if (k === 'l' && mod(e)) focusPane('right');
    // `l` はアーカイブの中でだけ「入る」── ヒント行が `Enter/l 入る` と
    // 書いているのはその画面だけで、書いてある以上のことはしない。
    // 両前端とも、書いてあるのに割り当てが無かった。
    else if (k === 'l' && bare && state[state.focus].archive) enter();
    // Shift+Tab before Tab, which swallowed it — the same shape as Enter below.
    else if (k === 'Tab' && e.shiftKey) goTab(state.focus, { step: 1 });
    else if (k === 'Tab') { state.focus = state.focus === 'left' ? 'right' : 'left'; draw('left'); draw('right'); }
    // The most-modified Enter first: Ctrl+Shift lands on the Ctrl arm if it
    // is tested second, and the snippet launcher was unreachable for it.
    else if (k === 'Enter' && mod(e) && e.shiftKey) cmdSnippets();
    else if (k === 'Enter' && mod(e)) openOut();
    // Before the plain Enter, which used to swallow it: the menu was written,
    // listed in the help, and never once opened from this key. A modified key
    // has to be tested before the key it modifies.
    else if (k === 'Enter' && e.shiftKey) openMenu(CONTEXT);
    // Visual selection first: Enter there means "keep these", and entering a
    // directory in the middle of choosing files is never what was meant.
    else if (k === 'Enter' && visual.on) endVisual(true);
    else if (k === 'Enter') enter();
    else if (k === 'Backspace' && state[state.focus].remote) remoteStep({ up: true });
    else if (k === 'Backspace') parent();
    else if (k === ' ' && bare) mark(false);
    else if (k === 'a' && mod(e)) mark(true);
    // `c` is `c` either way: whether it is a copy or an upload is decided by
    // which pane you are standing in, which the program already knows.
    else if (k === 'c' && !e.ctrlKey && !e.metaKey
             && (state.left.remote || state.right.remote)) transfer();
    else if (k === 'c' && !e.ctrlKey && !e.metaKey) operate('copy');
    else if (k === 'm' && bare) {
        // Moving across the network is a download that then deletes the
        // original, and nothing here does the second half yet. `c` copies;
        // saying so beats an error per file from a move that never could.
        if (state[state.focus].remote) say(tr('moving to or from a server is not built yet — use c to copy', 'サーバとの移動はまだです — c でコピーしてください'), true);
        else operate('move');
    }
    else if (k === 'd' && bare) {
        if (state[state.focus].remote) remoteOp('delete'); else operate('delete');
    }
    else if (k === 'T' && bare) openMenu(TOGGLES);
    else if (k === 'M' && bare) openMenu(CONTEXT);
    // `q` — 終了。cian-tui は確認を出してから終わる（keys.rs:2350）。
    // 一画面表示（詳細一覧・アイコン）では**文字**として扱うのも同じ ──
    // そこは端末ではなくデスクトップの見た目で、頭文字でファイルを探すのが
    // 当たり前だから、ディレクトリ名を打っている人に「出ますか」と訊かない。
    else if (k === 'q' && bare && !ONE_PANE.includes(viewMode)) cmdQuit();
    else if (k === 'Z' && bare) cmdJump();
    else if (k === 's' && bare) cmdShortcuts();
    else if (k === 'S' && bare) cmdSshPicker();
    else if (k === '@' && bare) cmdMacros();
    else if (k === 'F11') cmdFullscreen();
    else if (k === 'F12') zoomFocused();
    else if ((k === '=' || k === '+') && mod(e)) { setFont(FONT.at + 1); say(tr(`text ${FONT.at}px`, `文字の大きさ ${FONT.at}px`)); }
    else if (k === '-' && mod(e)) { setFont(FONT.at - 1); say(tr(`text ${FONT.at}px`, `文字の大きさ ${FONT.at}px`)); }
    else if (k === '0' && mod(e)) { setFont(baseFont()); say(tr('text back to its base size', '文字の大きさを戻しました')); }
    else if ((k === 't' && bare) || k === 'F9') tabNew();
    // `w` closes, F10 asks — cian-tui's split (keys.rs:2394 / 2407). An F-key
    // is easy to hit by accident and a tab can be the only thing holding a
    // place you walked to; `w` is deliberate enough not to need the question.
    else if (k === 'w' && bare) tabClose();
    else if (k === 'F10') tabClose(true);
    else if (k === 'F1') goTab(state.focus, { step: -1 });
    else if (k === 'F2') goTab(state.focus, { step: 1 });
    else if (k === 'J' && bare) { if (term.on) { setShellFocus(true); say(tr('shell', 'シェル')); } else openShell(); }
    else if (k === 'r' && bare) {
        if (state[state.focus].remote) remoteOp('rename'); else rename();
    }
    else if (k === 'a' && bare) {
        if (state[state.focus].remote) remoteOp('touch'); else create(false);
    }
    else if (k === 'A' && bare) {
        if (state[state.focus].remote) remoteOp('mkdir'); else create(true);
    }
    // Modified before bare, or bare `z` (go to a path) would answer first.
    //
    // The undo every other program on both platforms has. `u` was the only
    // way in, so the key a hand reaches for after a copy went to the one
    // place in cian that has no undo of its own — nowhere.
    // Both cases of the letter: with Shift down the key *is* `Z`, and Chromium
    // does not always agree with itself about that while a modifier is held.
    else if ((k === 'z' || k === 'Z') && mod(e)) { if (e.shiftKey) redo(); else undo(); }
    else if (k === 'u' && bare) undo();
    else if (k === 'V' && bare) invert();
    else if (k === 'v' && bare) startVisual();
    else if (k === 'o' && bare) syncPane(true);
    else if (k === 'O' && bare) syncPane(false);
    else if (k === 'z' && bare) goToPath();
    else if (k === 'P' && bare) clipFiles();
    else if (k === 'p' && bare) copyPaths();
    else if (k === 'F5') reread();
    else if (k === '?' && bare) openHelp();
    else if (k === 'F3') lookInsideAll();
    else if (k === ':' && bare) commandLine();
    // Three ways in, as cian-tui advertises three (`Ctrl+Shift+P, Ctrl+,, C`).
    // The window had only `C`, so the two a VS Code hand reaches for first
    // both did nothing. Modified before bare, or `,` would open the sort
    // picker and `P` the file clipboard.
    else if (k === 'P' && e.shiftKey && mod(e)) openPalette();
    else if (k === ',' && mod(e)) openPalette();
    else if (k === 'C' && bare) openPalette();
    // `//` has this as its second name in the terminal build's manual.
    else if (k === 'p' && mod(e) && !e.shiftKey) openFinder();
    // The modified ones first: `f` on its own would otherwise swallow Ctrl+F.
    else if ((k === 'f' || k === 'g') && mod(e)) runCommand(findCommand('grep'), '');
    else if (k === 'f' && bare) searchHere();
    else if (k === 'F' && bare) runCommand(findCommand('find'), '');
    else if (k === 'n' && bare) hopHere(1);
    else if (k === 'N' && bare) hopHere(-1);
    else if (k === 'b' && bare) cmdBranch();
    else if (k === '=' && bare) cmdCompare();
    else if ((k === 'r' || k === 'y') && mod(e)) redo();
    else if (k === 'h' && bare) cmdHistory();
    else if (k === 'ArrowLeft' && e.altKey) step('back');
    else if (k === 'ArrowRight' && e.altKey) step('forward');
    // The file clipboard holds *local* paths. A remote row's path names a
    // place on the server, and holding it would paste a path that exists
    // nowhere on this disk — quietly, later, somewhere else.
    else if ((k === 'c' || k === 'x') && mod(e) && state[state.focus].remote) {
        say(tr('a file on a server cannot be held on the clipboard — use c', 'サーバ上のファイルはクリップボードに持てません — c で転送してください'), true);
    }
    // Pasting into a server pane uploads what the register holds. The
    // register's paths never travel to the window — the engine owns both
    // halves of the gesture.
    else if (((k === 'v' && mod(e)) || (k === 'y' && bare)) && state[state.focus].remote) {
        uploadHeld();
    }
    else if (k === 'c' && mod(e)) hold('copy');
    else if (k === 'x' && mod(e)) hold('cut');
    else if ((k === 'v' && mod(e)) || (k === 'y' && bare)) paste();
    else if (k === '/' && bare) startFilter();
    else if (k === ',' && bare) openMenu(SORT_MENU);
    // Esc backs out of whatever the listing is showing that is not a
    // directory. A branch view and a panelized search are both "here is a set
    // of files"; leaving them is the same gesture.
    else if (k === 'Escape' && visual.on) endVisual(false);
    // A preview is showing but the keys are here, so Esc has to reach it from
    // the listing — `viewer.on` is false precisely so that j and k still move.
    else if (k === 'Escape' && preview.on) togglePreview();
    else if (k === 'Escape' && state[state.focus] && state[state.focus].flat) leaveFlat();
    // Leaving a server is Esc, as the terminal build has it (":remote … Esc
    // leaves"). :local stays as the spoken form of the same thing.
    else if (k === 'Escape' && state[state.focus] && state[state.focus].remote) cmdDisconnect();
    // The terminal build's listing Esc: clear marks and filter. Both at once,
    // because "get me back to the plain listing" is one intention.
    else if (k === 'Escape' && state[state.focus]
             && (state[state.focus].marked > 0 || state[state.focus].filter)) clearMarksAndFilter();
    else if (k === 'Escape' && running) {
        window.cian.call('cancel', { op: running.op });
        say(tr('stopping…', '中止しています…'));
    }
    else {
        // Nothing claimed it. Said out loud rather than swallowed: a key that
        // does nothing and a key that is not bound look identical from the
        // outside, and the terminal build grew `:key` for exactly this.
        if (k.length === 1 || k.startsWith('Arrow') || k.startsWith('F')) {
            console.log(`key: ${JSON.stringify(k)} code=${e.code}`
                + (e.ctrlKey ? ' ctrl' : '') + (e.metaKey ? ' meta' : '')
                + (e.shiftKey ? ' shift' : '') + (e.altKey ? ' alt' : ''));
        }
        return;
    }
    e.preventDefault();
});

// ─────────────────────────────────────────────────────────────────────────
// The AI chat.
//
// cian-tui's `Popup::AiChat` in a window: the same transcript, the same six
// doors into it (`:ai`, summarise, explain the error, explain the diff,
// triage the log, the three over a selection), and the same words on screen —
// あなた / AI - simple — because `parity.py` counts them and because someone
// who learned one build must not have to learn a second vocabulary.
//
// **The window had no chat at all.** It asked one question, printed the
// answer into a read-only list, and that was the end of it; "and the bigger
// one?" meant starting again with the whole context retyped.
// ─────────────────────────────────────────────────────────────────────────

/// The open conversation. `log` is `[{ user, text, sent? }]`, oldest first.
///
/// `text` is what is on screen and `sent` — when they differ — is what went to
/// the model. They differ for the doors that open with something already
/// asked: "summarise this file" reads better than the file, but a follow-up
/// about line 30 needs the file. Showing the payload would make the transcript
/// unreadable; sending the label would make the follow-up meaningless.
const chat = { on: false, log: [], pending: false, title: '' };

/// The one door. Everything that opens or closes the chat comes through here.
///
/// Written as a door because of what happened to the viewer: state set in
/// four places, one of which forgot the element, and a sheet that reported
/// itself open while nothing was drawn. The hint bar reads `chat.on`, so the
/// flag and the element have to move together or the keys it advertises are
/// not the keys that work.
function setChatOn(on) {
    chat.on = on;
    el.chat.hidden = !on;
    if (!on) {
        chat.log = [];
        chat.pending = false;
        el.cIn.value = '';
        el.cIn.blur();
    }
    drawHints();
}

/// Open a conversation. `seed` is an opening turn already spoken — the log to
/// triage, the diff to explain — or nothing, for `:ai`.
///
/// `sent` is what that turn really was, when the readable version is a label
/// for something much larger. Left out for the doors the *engine* composes
/// (`:ailog`, `:aierror`): the window never sees what they sent, so a
/// follow-up there leans on the answer being in the transcript — which it is,
/// and which is what a follow-up usually refers to anyway.
function openChat(title, seed = null, sent = null) {
    chat.title = title;
    chat.log = seed ? [{ user: true, text: seed, sent: sent || undefined }] : [];
    chat.pending = !!seed;
    setChatOn(true);
    el.cName.textContent = title;
    el.cAbout.textContent = tr("the AI's answers — check them before you use them",
                               'AI の答え — 確かめてから使ってください');
    el.cHint.textContent = tr('Enter send   Shift+Enter newline   Esc close',
                              'Enter 送信   Shift+Enter 改行   Esc 閉じる');
    el.cIn.value = '';
    drawChat();
    el.cIn.focus();
}

function drawChat() {
    const frag = document.createDocumentFragment();
    for (const m of chat.log) {
        const div = document.createElement('div');
        div.className = 'cturn ' + (m.user ? 'you' : 'ai');
        const who = document.createElement('span');
        who.className = 'cwho';
        // The same two names the terminal build prints. Not "User" and
        // "Assistant": `parity.py` compares the words, and two builds that
        // name the speakers differently are two programs.
        who.textContent = m.user ? tr('you', 'あなた') : 'AI - simple';
        const body = document.createElement('div');
        body.className = 'ctext';
        body.textContent = m.text;
        div.append(who, body);
        frag.append(div);
    }
    if (chat.pending) {
        const w = document.createElement('div');
        w.className = 'cturn ai cwait';
        w.textContent = tr('thinking…', '考えています…');
        frag.append(w);
    }
    el.cBody.replaceChildren(frag);
    // The newest turn, always. A transcript that keeps its scroll where it
    // was is a transcript whose answer arrives off-screen.
    el.cBody.scrollTop = el.cBody.scrollHeight;
}

/// Send what is typed, carrying the conversation it belongs to.
///
/// `prior` is the log *before* this question — the engine appends the
/// question itself, and a log that already holds it would send it twice.
async function sendChat() {
    const text = el.cIn.value.trim();
    if (!text || chat.pending) return;
    el.cIn.value = '';
    el.cIn.style.height = '';
    await askChat(text);
}

async function askChat(text) {
    const prior = chat.log.map((m) => ({ user: m.user, text: m.sent || m.text }));
    chat.log.push({ user: true, text });
    chat.pending = true;
    drawChat();
    const r = await ask('ai', {
        pane: state.focus,
        what: 'text',
        system: 'You are a concise assistant embedded in a file manager. '
              + 'Answer briefly in plain text.',
        prior,
        text,
    });
    if (!r) { chat.pending = false; drawChat(); return; }
    answerIntoChat();
}

/// Hand the next AI answer to the open chat.
///
/// The five doors that used to print into a read-only list all end here now.
/// A list you can only read was the whole of the window's AI: "and the bigger
/// one?" meant starting again with the context retyped.
function answerIntoChat() {
    aiWaiting = (answer) => {
        chat.pending = false;
        // Closed while the answer was in flight. The engine has no idea a
        // window shut, so the reply still arrives; pushed onto a cleared log
        // it would be a stray turn waiting in the next conversation.
        if (!chat.on) return;
        chat.log.push({ user: false, text: String(answer) });
        drawChat();
        el.cIn.focus();
    };
}

/// The chat's own keys. Everything not claimed here is left to the textarea,
/// which is what makes typing, selecting and Ctrl+V work without writing any
/// of it out.
function chatKey(e) {
    if (e.key === 'Escape') {
        e.preventDefault();
        setChatOn(false);
        // The viewer may be underneath — the chat can be opened from it. It
        // was never closed, so revealing it is the whole of putting it back.
        if (!viewer.on) refresh();
        return;
    }
    // Enter sends; Shift+Enter is a newline, as in the terminal build. A
    // question is usually one line and a pasted stack trace is not, and the
    // key that ends the sentence must not be the key that ends the paragraph.
    if (e.key === 'Enter' && !e.shiftKey && !e.altKey) {
        e.preventDefault();
        sendChat();
        return;
    }
    // Anything else types. Grown after the key lands, so the box is the size
    // of what is in it rather than the size of what was in it.
    queueMicrotask(growChatInput);
}

/// Fit the input to its content, up to the cap the CSS sets.
///
/// Reset first: a textarea's `scrollHeight` never shrinks below the height it
/// already has, so measuring without clearing gives a box that only ever
/// grows — three lines deleted and the room for them stays.
function growChatInput() {
    el.cIn.style.height = 'auto';
    el.cIn.style.height = `${el.cIn.scrollHeight}px`;
}

/// Is this family actually here, in a way that can be believed?
///
/// **`document.fonts.check()` cannot answer this.** It reports whether the
/// *stack* can draw the text, so it says yes to a name no machine has ever
/// heard of — measured 2026-09-06: `check('16px "Nonexistent Nerd Font XYZ"')`
/// is `true`. Everything built on it was reporting the first name in the list
/// and calling that the answer, which is the guess it was meant to replace.
///
/// So it is measured. The same string is drawn in `"<name>", <generic>` and in
/// `<generic>` alone; if the family exists it displaces the generic and the
/// width moves. Three generics, because a face that happens to match one of
/// them in metrics would look absent against that one — matching all three is
/// not a thing real faces do. The probe mixes wide Latin, narrow Latin and
/// 日本語 so a Latin-only face still parts from a CJK-capable generic.
function faceIsHere(name) {
    if (!name || /^(monospace|serif|sans-serif|ui-monospace|system-ui)$/i.test(name)) return false;
    const c = faceIsHere.ctx || (faceIsHere.ctx = document.createElement('canvas').getContext('2d'));
    const probe = 'MMMMMWWWWWiiiii日本語0123';
    const quoted = `"${name.replace(/"/g, '')}"`;
    return ['monospace', 'serif', 'sans-serif'].some((generic) => {
        c.font = `72px ${generic}`;
        const alone = c.measureText(probe).width;
        c.font = `72px ${quoted}, ${generic}`;
        return c.measureText(probe).width !== alone;
    });
}

/// Put the face `cian.font{ face = "…" }` named at the front of the queue.
///
/// **Only the front.** The stacks in index.html stay behind it, bundled face
/// included, because a chosen face is a preference about the letters cian
/// mostly draws and not a promise about every codepoint a listing can contain:
/// a Latin-only Nerd Font asked to draw 日本語 would otherwise fall through to
/// whatever the machine calls its default, which on Japanese Windows is 明朝 —
/// proportional, and this is a program that draws in columns.
///
/// Silence is the failure mode worth guarding: a face that is not installed
/// looks exactly like one that is, because the browser walks past it without a
/// word. So the name asked for is kept, and the opening line below compares it
/// with the one that actually drew.
///
/// **Not said from here.** It was, and it was never seen: the status line is
/// one slot, and the listing's own "9 件 / 9 件" lands after the config does.
/// A message that is written and immediately painted over is worse than no
/// message, because it looks like it was handled.
function applyFace(name) {
    const face = typeof name === 'string' ? name.trim() : '';
    if (!face) return;
    cfg.faceAsked = face;
    document.documentElement.style.setProperty('--user-face', `"${face.replace(/"/g, '')}"`);
}

/// Which face the listing actually got.
///
/// Naming a font is a request, not an instruction: the browser walks the list
/// and takes the first one installed, and if none is, the answer is whatever
/// the machine calls `sans-serif` — or worse, its default, which on Japanese
/// Windows is 明朝. That happened, and it took a person at the machine to
/// notice. A guess about type is not worth having when the answer can be
/// measured — and measuring is [`faceIsHere`], because the API that looks like
/// it answers this does not.
function resolvedFace() {
    const asked = getComputedStyle(document.body).fontFamily.split(',');
    for (const raw of asked) {
        const name = raw.trim().replace(/^["\']|["\']$/g, '');
        if (faceIsHere(name)) return name;
    }
    return tr('(none of them — the browser chose)', '(どれも無く、ブラウザが選びました)');
}

// ─────────────────────────────────────────────────────────────────────────
// Anything that answers with a list.
//
// A search, disk usage, checksums, the history: one screen, not one per
// question. They differ in what Enter does and in nothing else, so the screen
// takes a `pick` and the rest is the same rows, the same j/k, the same Esc.
// Written the other way, cian's twenty-odd reports would be twenty-odd
// almost-identical lists, and they would stop agreeing within a month.
// ─────────────────────────────────────────────────────────────────────────
const report = { on: false, rows: [], at: 0, pick: null, act: null, move: null, leave: null };

/// Show a list. `rows` are `{ n, label, sub, path }` — `n` is the right-aligned
/// left column (a size, a line number, nothing), `label` the thing itself,
/// `sub` the dimmed remainder.
function show(title, about, rows, opts = {}) {
    report.on = true;
    report.rows = rows;
    // The unfiltered set, kept so narrowing is reversible by backspacing —
    // a filter that discards what it hides can only ever be typed forwards.
    report.all = rows;
    report.about = about;
    report.query = !!opts.filter;
    // A list you answer per row rather than all at once. cian-tui's four
    // review screens (junk, dupes, structure, rename) are all this shape:
    // the model proposes, and a person ticks off the ones they meant. All-or-
    // nothing turns "mostly right" into "start again by hand".
    report.checks = !!opts.checks;
    if (report.checks) for (const r of rows) if (r.on === undefined) r.on = true;
    report.at = 0;
    report.pick = opts.pick || null;
    report.act = opts.act || null;
    // Called as the cursor passes, not on Enter. For a list whose rows *are*
    // the thing — the palettes — where reading a name is no substitute for
    // seeing it.
    report.move = opts.move || null;
    // Called when the list is dismissed rather than chosen from — for a list
    // that has been changing things while you looked at it.
    report.leave = opts.leave || null;
    // A sheet raised by a search wears the search colour, as the pane does
    // while you are typing into it — `Ctrl+F` and `Shift+F` end in a list,
    // and the list is where you are still searching.
    if (opts.mode) el.report.dataset.mode = opts.mode;
    else delete el.report.dataset.mode;
    // Line the second column up.
    //
    // `.hit .p` grows to fill, so the path beside each name started at a
    // different x on every row — fine for a list whose second column is a
    // count, unreadable for one whose second column is a path you are
    // comparing down. Asked for on the bookmarks (`s`), and true of any list
    // built this way, so the option lives here rather than there: the widest
    // name decides one column width for the whole list. Capped, because one
    // very long name must not push every path off the right-hand edge.
    if (opts.align) {
        const widest = rows.reduce((n, r) => Math.max(n, String(r.label || '').length), 0);
        el.report.dataset.align = '1';
        el.report.style.setProperty('--name-w', `${Math.min(widest + 2, 34)}ch`);
    } else {
        delete el.report.dataset.align;
        el.report.style.removeProperty('--name-w');
    }
    // 折り返すか、1行で切るか。パスの一覧は切るのが正しく（末尾が効く）、
    // 説明の一覧は折り返すのが正しい（後半が本文）。
    if (opts.wrap) el.report.dataset.wrap = '1';
    else delete el.report.dataset.wrap;
    el.rName.textContent = title;
    el.rAbout.textContent = about;
    el.rFoot.textContent = opts.foot
        || (report.checks ? tr('Space off/on   a all   n none   Enter run   Esc cancel', 'Space 外す／戻す   a 全部   n 全部外す   Enter 実行   Esc 取消')
            : report.query ? tr('type to narrow   ↑↓ choose   Enter open   Esc close', '打って絞る   ↑↓ 選ぶ   Enter 開く   Esc 閉じる')
            : rows.length ? tr('↑↓ choose   Enter open   Esc close', '↑↓ 選ぶ   Enter 開く   Esc 閉じる') : tr('Esc close', 'Esc 閉じる'));
    el.rQ.hidden = !report.query;
    el.rQ.value = '';
    el.rQ.placeholder = opts.hint || tr('type to narrow', '打って絞り込み');
    el.report.hidden = false;
    drawReport();
    drawCheckCount();
    if (report.query) el.rQ.focus();
}

/// Narrow the list to what was typed.
///
/// A plain case-insensitive substring, over the label and whatever is beside
/// it. Deliberately *not* fuzzy: the file finder's ranking lives in Rust so
/// there is only one of it, and a second matcher written here would drift
/// from it within a month. These lists are a hundred-odd known names, where
/// "contains what I typed" is both predictable and enough.
function filterReport() {
    const q = el.rQ.value.trim().toLowerCase();
    report.rows = q
        ? report.all.filter((r) => `${r.label} ${r.sub || ''}`.toLowerCase().includes(q))
        : report.all;
    report.at = 0;
    el.rAbout.textContent = q
        ? tr(`${report.rows.length} / ${report.all.length}`, `${report.rows.length} / ${report.all.length} 件`)
        : report.about;
    drawReport();
    // The preview follows the narrowing, not just the arrows: with the
    // palettes, the top row of what you have typed *is* the answer you are
    // looking at.
    if (report.move && report.rows[report.at]) report.move(report.rows[report.at]);
}

/// How many are ticked, said where the total was. The number is the whole
/// point of ticking, and a list that does not show it makes you count.
function drawCheckCount() {
    if (!report.checks) return;
    const on = report.rows.filter((r) => r.on).length;
    el.rAbout.textContent = tr(`${on} / ${report.rows.length}   ${report.about}`, `${on} / ${report.rows.length} 件   ${report.about}`);
}

function closeReport(abandoned = false) {
    if (abandoned && report.leave) report.leave();
    report.on = false;
    report.move = null;
    report.leave = null;
    report.rows = [];
    report.all = [];
    report.query = false;
    el.rQ.hidden = true;
    el.rQ.blur();
    el.report.hidden = true;
}

function drawReport() {
    const frag = document.createDocumentFragment();
    report.rows.forEach((row, i) => {
        const div = document.createElement('div');
        div.className = 'hit' + (i === report.at ? ' on' : '')
            + (report.checks && !row.on ? ' off' : '');
        if (report.checks) {
            const box = document.createElement('span');
            box.className = 'box';
            box.textContent = row.on ? '✓' : '·';
            box.addEventListener('mousedown', (e) => {
                // The click ticks the box and does *not* run the list —
                // cian-tui's review rows toggle on click (lib.rs:955).
                e.stopPropagation();
                row.on = !row.on;
                report.at = i;
                drawReport();
            });
            div.append(box);
        }
        if (row.n !== undefined && row.n !== null) {
            const n = document.createElement('span');
            n.className = 'n';
            n.textContent = row.n;
            div.append(n);
        }
        // A proportion, drawn rather than written. `row.bar` is 0..1 and the
        // strip sits behind the name: "what is big here" is a question about
        // relative size, and a column of numbers makes you do the comparing.
        // A terminal can only spend cells on this; a window can put it under
        // the text and cost nothing.
        if (typeof row.bar === 'number') {
            const b = document.createElement('span');
            b.className = 'bar';
            b.style.width = `${Math.max(1, Math.round(row.bar * 100))}%`;
            div.append(b);
            div.classList.add('barred');
        }
        const l = document.createElement('span');
        l.className = 'p';
        l.textContent = row.label;
        div.append(l);
        if (row.sub) {
            const sub = document.createElement('span');
            sub.className = 'sub';
            sub.textContent = row.sub;
            div.append(sub);
        }
        div.addEventListener('mousedown', () => {
            report.at = i;
            drawReport();
            if (report.pick) report.pick(row);
        });
        frag.append(div);
    });
    el.rRows.replaceChildren(frag);
    const on = el.rRows.children[report.at];
    if (on) on.scrollIntoView({ block: 'nearest' });
}

document.addEventListener('keydown', (e) => {
    if (!report.on) return;
    e.stopPropagation();
    const last = report.rows.length - 1;
    const go = (to) => {
        report.at = Math.max(0, Math.min(last, to));
        drawReport();
        if (report.move && report.rows[report.at]) report.move(report.rows[report.at]);
    };
    const k = e.key;
    const ctrl = e.ctrlKey || e.metaKey;
    // What means the same thing whether or not there is a box to type in.
    // Ctrl+n / Ctrl+p are here because with a filter the letters are text,
    // and the terminal build's palette takes exactly these (keys.rs:813).
    if (k === 'Escape') closeReport(true);
    else if (k === 'ArrowDown' || (ctrl && k === 'n')) go(report.at + 1);
    else if (k === 'ArrowUp' || (ctrl && k === 'p')) go(report.at - 1);
    else if (k === 'PageDown') go(report.at + 20);
    else if (k === 'PageUp') go(report.at - 20);
    else if (k === 'Enter' && report.checks && report.pick) {
        // The ticked ones, not the row under the cursor.
        report.pick(report.rows.filter((r) => r.on));
    }
    else if (k === 'Enter' && report.pick && report.rows[report.at]) report.pick(report.rows[report.at]);
    else if (report.checks && (k === ' ' || k === 'a' || k === 'n')) {
        if (k === ' ') { const r = report.rows[report.at]; if (r) r.on = !r.on; }
        else for (const r of report.rows) r.on = (k === 'a');
        // The cursor stays put. A list that jumps to the top on every tick is
        // a list you cannot work down.
        drawReport();
        drawCheckCount();
    }
    else if (report.query) {
        // Everything else is text. Not swallowed and not acted on: the box
        // has the focus and the character belongs to it.
        return;
    }
    else if (k === 'q') closeReport(true);
    else if (k === 'j') go(report.at + 1);
    else if (k === 'k') go(report.at - 1);
    else if (k === 'g') go(0);
    else if (k === 'G') go(last);
    else if (report.act && report.act[k]) report.act[k]();
    else if (k === 'y') {
        // The whole list, as text. cian-tui's Notice copies with `y`/`c`, and
        // the reason is the same here: a result screen is a thing you paste
        // into a ticket, and without this you retype it off the screen.
        const text = report.rows
            .map((r) => [r.n, r.label, r.sub].filter(Boolean).join('\t'))
            .join('\n');
        navigator.clipboard.writeText(text);
        say(tr(`${report.rows.length} lines copied`, `${report.rows.length} 行をコピー`));
    }
    else return;
    e.preventDefault();
}, true);

document.addEventListener('input', (e) => {
    if (report.on && report.query && e.target === el.rQ) filterReport();
});

/// Bytes, the way a person reads them.
function human(n) {
    if (n < 1024) return `${n} B`;
    const u = ['KB', 'MB', 'GB', 'TB'];
    let v = n / 1024;
    let i = 0;
    while (v >= 1024 && i < u.length - 1) { v /= 1024; i += 1; }
    return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${u[i]}`;
}

/// The editor's runtime, loaded once and only when a file is opened.
///
/// **One component reads and writes.** The terminal build has no separate
/// editor either — its viewer becomes editable where you stand — and a
/// hand-written viewer beside a real editor would be two implementations of
/// the same motions and the same search, which is the pair that always drifts.
///
/// It is not in the repository. `node gui/vendor.js` puts it there out of
/// node_modules, trimmed to what actually runs; the release builds carry it.
let monacoLoading = null;

function loadMonaco() {
    if (monacoLoading) return monacoLoading;
    monacoLoading = new Promise((ok, no) => {
        const s = document.createElement('script');
        s.src = 'vendor/monaco/vs/loader.js';
        s.onerror = () => no(new Error(
            tr('gui/vendor is missing — run `node vendor.js` in gui/', 'gui/vendor がありません — gui/ で `node vendor.js` を実行してください')));
        s.onload = () => {
            // Absolute, because the editor starts a worker and a worker has
            // no idea what this page's directory was. A relative path sent it
            // looking for `file:///vendor/...` — the root of the disk — and
            // the editor came up as an exception with an empty window behind
            // it.
            const vs = new URL('vendor/monaco/vs', document.baseURI).href;
            // eslint-disable-next-line no-undef
            require.config({
                paths: { vs },
                'vs/nls': { availableLanguages: { '*': 'ja' } },
            });
            // eslint-disable-next-line no-undef
            require(['vs/editor/editor.main'], () => withVim().then(() => ok(window.monaco), no), no);
        };
        document.head.append(s);
    });
    return monacoLoading;
}

/// Reading a file, without leaving cian.
///
/// A window of lines is drawn rather than the whole file. The terminal build
/// works this way because a terminal has no choice; here it is a choice, and
/// the right one — a hundred thousand rows is a hundred thousand elements
/// otherwise, and the files worth opening in a viewer are exactly the long
/// ones. Which lines are on screen is arithmetic either way.
/// Add the vim grammar, which will not load itself.
///
/// monaco-vim ships a UMD bundle that checks for `define.amd` first — and
/// Monaco's own loader defines one, so it takes the AMD branch and goes
/// looking for `monaco-editor/esm/vs/editor/editor.api`, which is not in the
/// trimmed runtime and would not be the same copy of Monaco if it were. With
/// `define` out of sight for the length of the load it takes the plain-global
/// branch instead and picks up the `monaco` already on the window: one copy of
/// the editor, which is the only arrangement in which the vim keys reach the
/// editor the file is open in.
function withVim() {
    return new Promise((ok, no) => {
        const saved = window.define;
        window.define = undefined;
        const s = document.createElement('script');
        s.src = 'vendor/monaco-vim.js';
        const restore = () => { window.define = saved; };
        s.onload = () => { restore(); ok(); };
        s.onerror = () => { restore(); no(new Error(tr('vendor/monaco-vim.js is missing', 'vendor/monaco-vim.js がありません'))); };
        document.head.append(s);
    });
}

/// Reading and writing a file, without leaving cian.
///
/// **One component does both.** The terminal build has no separate editor —
/// its viewer becomes editable where you stand — and a hand-written viewer
/// beside a real editor would be two implementations of the same motions and
/// the same search. That pair always drifts; it is the reason the clipboard
/// rules and the copy guard live in cian-core.
/// The files open in the viewer, and which one is showing.
///
/// More than one because the answer to "which of these has the error" is found
/// by opening several and stepping between them, and closing one to look at
/// the next loses your place in the first.
const openFiles = { list: [], at: 0 };

const viewer = {
    on: false, opening: false, ed: null, vim: null,
    name: '', about: '', dirty: false, readOnly: false,
    /// The model's version at the last read or write.
    ///
    /// Dirtiness is this compared against the current version, not a flag set
    /// by the first edit. A flag says "changed" for ever, including after the
    /// edits have been undone back to what is on disk — and it also went up
    /// the moment the file was loaded, because filling an editor is a change
    /// like any other. Monaco's alternative version id is exactly this
    /// question already answered.
    base: 0,
};

/// Open or close the reading panel, and tell the hint bar.
///
/// The bar had a whole branch for the viewer (`hintsNow`) that nobody ever
/// saw: `drawHints()` is called from `say()`, and opening a file says nothing,
/// so a file open in front of you was advertised with the *listing's* keys —
/// ペイン, マーク, 並替, F3 閲覧 — none of which do anything while the editor
/// has the keyboard. cian-tui swaps its bar for the panel's; this is the one
/// door that makes the window do the same, rather than eight remembered calls.
function setViewerOn(on) {
    viewer.on = on;
    // A diagram parked in the editor belongs to the file that was open. Left
    // behind, the next file inherits somebody else's picture at whatever line
    // number it happened to be on.
    if (!on) { clearDiagramZones(); zones.on = false; }
    drawHints();
}

/// Which grammar the editor speaks.
///
/// notepad by default, because that is what a Windows desktop expects and
/// what was decided for this build. vim for Taketan, and for anyone else who
/// would rather. Where the choice is remembered is still open — the same
/// question as the look, and answering it in two places would be worse than
/// leaving it unanswered in one.
const STYLES = [['notepad'], ['vim']];
/// What to call a grammar on screen. Separate from `STYLES` above, which is
/// the *identity* of each — an id is written into the settings file and must
/// not change with the language; a label is read by a person and must.
function styleName(i) {
    return STYLES[i][0] === 'vim' ? 'vim' : tr('notepad', 'メモ帳');
}
/// vim, as in cian-tui (`edit_style: … unwrap_or(EditStyle::Vim)`, lib.rs:3013):
/// "the default, and the one cian was built around". This started on notepad,
/// so a window with no remembered setting and no `edit_style` in init.lua
/// opened files in a different grammar than the terminal build did — the one
/// difference in this pair that changes what every key in the editor means.
/// A remembered setting or init.lua still wins, as it does there.
let style = 1;

/// Every look is one of two grounds. Monaco ships a light and a dark theme, and
/// the editor sitting in the wrong one is the sort of thing that reads as
/// broken rather than as unstyled.
function editorTheme() {
    if (palette) return palettes.get(palette).light ? 'vs' : 'vs-dark';
    return LOOKS[look][0] === 'inei' || LOOKS[look][0] === 'terminal' ? 'vs-dark' : 'vs';
}

/// Is the window dark right now? Asked by mermaid and the preview, which have
/// to choose a diagram theme and a code theme before they draw.
function isDark() {
    if (palette) return !palettes.get(palette).light;
    return LOOKS[look][0] === 'inei' || LOOKS[look][0] === 'terminal';
}

const MONACO_LANG = {
    Rust: 'rust', TypeScript: 'typescript', JavaScript: 'javascript', Python: 'python',
    Json: 'json', Toml: 'ini', Yaml: 'yaml', Markdown: 'markdown', Html: 'html',
    Css: 'css', Shell: 'shell', C: 'c', Cpp: 'cpp', Go: 'go', Java: 'java',
    Lua: 'lua', Sql: 'sql', Xml: 'xml', Ruby: 'ruby', Php: 'php',
};

async function lookInside() {
    const which = state.focus;
    const pane = state[which];
    const row = pane && pane.entries[pane.cursor];
    if (!row || row.parent) return;
    if (row.is_dir) { await enter(); return; }
    // Opening takes a second the first time — the editor's runtime has to load
    // — and `viewer.on` is not true until it has. Enter followed by F3 in that
    // second started a second open, and the second one's setValue landed on
    // the first one's editor.
    if (viewer.opening || viewer.on) return;
    viewer.opening = true;
    try {
        // Inside an archive the row names nothing on this disk, so the member
        // is extracted first and read from there.
        if (pane.remote) {
            await openRemoteMember(which);
        } else if (pane.archive) {
            await openArchiveMember(which);
        } else {
            await openInEditor(which);
        }
    } finally {
        viewer.opening = false;
    }
}

/// Which lines differ from the file on disk, drawn down the gutter.
///
/// **The editor knew it was dirty and not where.** One bit for the whole file
/// — so after ten minutes of typing the only way to find your own edits was to
/// remember them. Every other editor draws this and it is the cheapest
/// orientation there is.
///
/// The comparison is the engine's (`cian_core::diff`), not one written here.
/// A second line-differ in JavaScript would eventually disagree with the
/// first, and then the diff panel, the F7 hop and this gutter would be three
/// opinions about the same two files.
///
/// Debounced, because it reads the file and diffs it: on every keystroke that
/// is a round trip per character, and the answer is only interesting once the
/// hands stop.
let diskDiffAt = null;
let diskDiffMarks = [];

function markDiskDiff() {
    clearTimeout(diskDiffAt);
    diskDiffAt = setTimeout(() => { paintDiskDiff().catch(() => {}); }, 350);
}

async function paintDiskDiff() {
    if (!viewer.on || !viewer.ed || !viewer.path) return;
    const lines = viewer.ed.getModel().getLinesContent();
    const r = await ask('diskdiff', { path: viewer.path, lines });
    if (!r || !viewer.ed) return;
    const monaco = window.monaco;
    if (!monaco) return;
    const next = [];
    (r.marks || []).forEach((m, i) => {
        if (m === 'same') return;
        next.push({
            range: new monaco.Range(i + 1, 1, i + 1, 1),
            options: {
                isWholeLine: true,
                // In the gutter, not over the text: a colour behind the words
                // is a highlight and means "look here", and these lines are
                // not more important than the rest of the file — only newer.
                linesDecorationsClassName: m === 'new' ? 'gut-new' : 'gut-changed',
            },
        });
    });
    diskDiffMarks = viewer.ed.deltaDecorations(diskDiffMarks, next);
}

/// After a save the file *is* the disk, so the gutter has nothing to say.
function clearDiskDiff() {
    clearTimeout(diskDiffAt);
    if (viewer.ed && diskDiffMarks.length) {
        diskDiffMarks = viewer.ed.deltaDecorations(diskDiffMarks, []);
    }
    diskDiffMarks = [];
}

/// `jj` leaves insert mode — **and so do `ｊｊ` and `っｊ`.**
///
/// The first is the mapping half the vim world writes into its config, and
/// monaco-vim would take `Vim.map('jj', '<Esc>', 'insert')` for it. The other
/// two are why that is not enough here: with a Japanese IME on, pressing j
/// twice does not produce two `j` keystrokes at all. In kana mode it makes
/// `っｊ` — a sokuon and a half-formed consonant — and in full-width mode
/// `ｊｊ`. Those arrive as **committed text**, through composition, not as
/// keys, so nothing that maps keystrokes can see them.
///
/// So this watches what lands in the buffer instead. Three strings, one rule:
/// when insert mode has just taken one of them, take it back out and leave.
/// That nobody types `jj` on purpose is the reason it was chosen in the first
/// place, and it is as true of the other two.
///
/// Bounded: only the characters immediately before the cursor, only while vim
/// is actually in insert mode, and only for a small change — a paste or an
/// undo is not somebody pressing a key twice.
const JJ = ['jj', 'ｊｊ', 'っｊ'];

function armJJ() {
    if (!viewer.ed) return;
    if (viewer.jj) { viewer.jj.dispose(); viewer.jj = null; }
    viewer.jj = viewer.ed.onDidChangeModelContent((ev) => {
        if (!viewer.vim || !vimTyping()) return;
        if (!ev.changes.length || ev.changes.some((c) => c.text.length > 2)) return;
        const model = viewer.ed.getModel();
        const at = viewer.ed.getPosition();
        if (!model || !at) return;
        const before = model.getLineContent(at.lineNumber).slice(0, at.column - 1);
        const hit = JJ.find((seq) => before.endsWith(seq));
        if (!hit) return;
        viewer.ed.executeEdits('cian-jj', [{
            range: {
                startLineNumber: at.lineNumber, startColumn: at.column - hit.length,
                endLineNumber: at.lineNumber, endColumn: at.column,
            },
            text: '',
        }]);
        // Out the way Esc goes. **`viewer.vim` is the adapter, not the
        // editor** — what `initVimMode` returns is monaco-vim's CodeMirror
        // shim (`handleKeyDown`, `state`, `editor`), and that is what these
        // take. Passing the Monaco editor threw `Cannot read properties of
        // undefined (reading 'vim')` from inside the library.
        // eslint-disable-next-line no-undef
        const V = MonacoVim.VimMode.Vim;
        if (V.exitInsertMode) V.exitInsertMode(viewer.vim);
        else V.handleKey(viewer.vim, '<Esc>');
    });
}

/// F7 / Shift+F7 — the next and previous difference.
///
/// **`editor.action.diffReview.next` does not exist in this Monaco.** The
/// footer has advertised these two since the diff editor was added, and the
/// trigger was a no-op the whole time: `getSupportedActions()` on the pair
/// lists a hundred and one ids and not one of them matches `diff`. It was in
/// the set of keys the round had never pressed, which is exactly the argument
/// for pressing them — the same set held Ctrl+E and the vim Ctrl+C/X/V.
///
/// `getLineChanges()` is there, so the walk is arithmetic: find the first
/// change past the line the cursor is on, wrap at the end, and put the cursor
/// on it. Both editors move, because a difference is a fact about the pair.
function hopDiff(step) {
    const changes = (pair.ed.getLineChanges && pair.ed.getLineChanges()) || [];
    if (!changes.length) {
        say(tr('the two are identical', '2つに違いはありません'));
        return;
    }
    const mod = pair.ed.getModifiedEditor();
    const at = (mod.getPosition() || { lineNumber: 1 }).lineNumber;
    // A change that only deletes has `modifiedEndLineNumber === 0`; its place
    // in the modified file is the line it would have sat before.
    const lineOf = (c) => c.modifiedStartLineNumber || c.originalStartLineNumber || 1;
    let next;
    if (step > 0) next = changes.find((c) => lineOf(c) > at) || changes[0];
    else next = [...changes].reverse().find((c) => lineOf(c) < at) || changes[changes.length - 1];
    const line = lineOf(next);
    mod.setPosition({ lineNumber: line, column: 1 });
    mod.revealLineInCenter(line);
    const org = next.originalStartLineNumber || line;
    pair.ed.getOriginalEditor().revealLineInCenter(org);
    const n = changes.indexOf(next) + 1;
    say(tr(`difference ${n} / ${changes.length}   line ${line}`, `相違 ${n} / ${changes.length}   ${line} 行目`));
}

/// Zoom and pan a picture.
///
/// cian-tui draws images as half-blocks (`▀`, 24-bit colour) because that is
/// the most a terminal cell can hold, and at that fidelity zooming has nothing
/// to show. A window has the actual pixels, and "is that the right screenshot"
/// is usually a question about a detail — so: `+` / `-` / wheel to scale, `0`
/// for actual size, `f` to fit the window, and drag to move it around.
const pic = { at: 1, fit: true, node: null, ox: 0, oy: 0 };

function fitPicture(node, r) {
    pic.node = node;
    pic.at = 1;
    pic.fit = true;
    pic.ox = 0;
    pic.oy = 0;
    node.classList.add('zoomable');
    paintPicture(r);
    // Dragging moves the picture, which only means anything once it is bigger
    // than the box — but a gesture that works sometimes and is dead the rest
    // of the time reads as broken, so it always moves and simply has nowhere
    // to go when the whole picture fits.
    let from = null;
    node.addEventListener('mousedown', (e) => {
        from = { x: e.clientX - pic.ox, y: e.clientY - pic.oy };
        e.preventDefault();
    });
    window.addEventListener('mousemove', (e) => {
        if (!from || !pic.node) return;
        pic.ox = e.clientX - from.x;
        pic.oy = e.clientY - from.y;
        pic.fit = false;
        paintPicture();
    });
    window.addEventListener('mouseup', () => { from = null; });
    node.addEventListener('wheel', (e) => {
        e.preventDefault();
        zoomPicture(e.deltaY < 0 ? 1.15 : 1 / 1.15);
    }, { passive: false });
}

function paintPicture(r) {
    const n = pic.node;
    if (!n) return;
    if (pic.fit) {
        n.style.cssText = '';
        n.classList.add('fit');
    } else {
        n.classList.remove('fit');
        n.style.transformOrigin = 'center center';
        n.style.transform = `translate(${pic.ox}px, ${pic.oy}px) scale(${pic.at})`;
    }
    const size = n.naturalWidth ? `${n.naturalWidth} × ${n.naturalHeight}` : '';
    el.vAbout.textContent = pic.fit
        ? tr(`${size}   fitted to the window`, `${size}   窓に合わせています`)
        : `${size}   ${Math.round(pic.at * 100)}%`;
}

function zoomPicture(by) {
    if (!pic.node) return;
    // Coming out of "fit" starts from what is on screen, not from 1× — the
    // picture would otherwise jump to full size on the first press.
    if (pic.fit) {
        const box = pic.node.getBoundingClientRect();
        pic.at = pic.node.naturalWidth ? box.width / pic.node.naturalWidth : 1;
        pic.fit = false;
    }
    pic.at = Math.max(0.05, Math.min(20, pic.at * by));
    paintPicture();
}

/// Something the window can draw but not read: a picture, a PDF.
///
/// Tried before the text read, not after it fails. `read_text` refuses a PNG
/// with "looks binary", which is true and unhelpful — the answer to opening a
/// picture is the picture.
async function openAsPicture(which) {
    const r = await ask('bytes', { pane: which });
    if (!r || !r.kind) return false;
    setViewerOn(true);
    viewer.name = r.name;
    el.view.hidden = false;
    el.vBody.hidden = true;
    el.vPic.hidden = false;
    const node = document.createElement(r.kind === 'application/pdf' ? 'embed' : 'img');
    node.src = `data:${r.kind};base64,${r.b64}`;
    if (node.tagName === 'EMBED') { node.type = r.kind; node.style.cssText = 'width:100%;height:100%'; }
    node.addEventListener('load', () => {
        el.vAbout.textContent = node.naturalWidth
            ? `${node.naturalWidth} × ${node.naturalHeight}   ${human(r.len)}`
            : human(r.len);
        // The load lands after the first paint, so the zoom state has to be
        // written *again* here or the picture's own line goes back to saying
        // only its dimensions.
        if (node.classList.contains('zoomable')) paintPicture(r);
    });
    el.vPic.replaceChildren(node);
    if (node.tagName === 'IMG') {
        fitPicture(node, r);
        el.vFoot.textContent = tr('+ / − zoom   ·   0 actual size   ·   f fit   ·   drag to move   ·   E editor   ·   Shift+Enter where it is   ·   Esc ×3 closes', '+ / − 拡大・縮小   ·   0 原寸   ·   f 窓に合わせる   ·   ドラッグで移動   ·   E 外部エディタ   ·   Shift+Enter 場所   ·   Esc ×3 閉じる');
    }
    el.vName.textContent = r.name;
    el.vAbout.textContent = human(r.len);
    el.vFoot.textContent = tr('Esc ×3 closes', 'Esc ×3 閉じる');
    return true;
}

/// Make the editor once, or reuse it.
///
/// Extracted because two things open it now — a file, and the list of names
/// `:renamelist` edits — and the second was reaching it by opening a file and
/// closing it again, which worked exactly as badly as it sounds.
function makeEditor(monaco, text, lang) {
    if (!viewer.ed) {
    // 開くたびに当てる ── モデルは開くたびに新しく、既定の 4 で生まれる。
    queueMicrotask(applyTabWidth);
    viewer.ed = monaco.editor.create(el.vBody, {
        value: text,
        language: lang,
        theme: editorTheme(),
        automaticLayout: true,
        fontFamily: getComputedStyle(document.body).fontFamily,
        fontSize: parseFloat(getComputedStyle(document.body).fontSize),
        minimap: { enabled: false },
        // The one place this build differs from a code editor's defaults:
        // a file manager opens files it did not write, and reformatting
        // them on the way past is not its business.
        renderWhitespace: 'selection',
        scrollBeyondLastLine: false,
    });
    viewer.ed.onDidChangeModelContent(() => {
        // The gutter is asked for on every edit — debounced inside — because
        // *which* lines changed moves with every keystroke, while `dirty`
        // only flips twice in a session.
        markDiskDiff();
        const now = viewer.ed.getModel().getAlternativeVersionId();
        const dirty = now !== viewer.base;
        if (dirty === viewer.dirty) return;
        viewer.dirty = dirty;
        drawViewFoot();
    });
    viewer.ed.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => saveFile());
    // The outline needs a key of its own in here: `:` belongs to the editor
    // once a file is open, so the command line cannot reach it.
    viewer.ed.addCommand(
        monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyO,
        () => cmdOutline(),
    );
    viewer.ed.addCommand(
        monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyB,
        () => cmdBlame(),
    );
    // The rectangle's verbs. Monaco selects one; these are what vim does to
    // it, and they are the reason to select one at all.
    viewer.ed.addCommand(monaco.KeyMod.Alt | monaco.KeyMod.Shift | monaco.KeyCode.KeyI,
        () => blockEdit('insert'));
    viewer.ed.addCommand(monaco.KeyMod.Alt | monaco.KeyMod.Shift | monaco.KeyCode.KeyA,
        () => blockEdit('append'));
    viewer.ed.addCommand(monaco.KeyMod.Alt | monaco.KeyMod.Shift | monaco.KeyCode.KeyC,
        () => blockEdit('replace'));
    viewer.ed.addCommand(monaco.KeyMod.Alt | monaco.KeyMod.Shift | monaco.KeyCode.KeyD,
        () => blockEdit('delete'));
    // The rendered document, and back. The terminal build's key for it.
    // Ctrl+E is not registered here. It was, and it never ran: Monaco's own
    // binding for that chord won, so the key moved the cursor to the end of
    // the line instead of drawing the document. It is taken in a capture-phase
    // listener beside togglePreview2, ahead of the editor.
    viewer.ed.onDidChangeCursorPosition(drawViewFoot);
    } else {
    viewer.ed.updateOptions({ theme: editorTheme() });
    monaco.editor.setModelLanguage(viewer.ed.getModel(), lang);
    viewer.ed.setValue(text);
    }

}

/// `F3` on a marked set opens all of them; `F2` and `Shift+F2` step between.
async function lookInsideAll() {
    const pane = state[state.focus];
    const marked = pane.entries.filter((x) => x.marked && !x.is_dir);
    if (marked.length < 2) { await lookInside(); return; }
    openFiles.list = marked.map((x) => x.path);
    openFiles.at = 0;
    await openNth(0);
    say(tr(`opened ${marked.length} (F2 / Shift+F2 step between them)`, `${marked.length} 件を開きました（F2 / Shift+F2 で行き来）`));
}

async function openNth(at) {
    const n = openFiles.list.length;
    if (!n) return;
    openFiles.at = ((at % n) + n) % n;
    // The cursor has to move too: everything downstream — save, blame,
    // outline — asks the engine about "the selected file", and a viewer
    // showing one file while the engine holds another is the kind of
    // disagreement that writes to the wrong place.
    if (!await landOn(openFiles.list[openFiles.at])) return;
    if (viewer.on) await closeView(false);
    await lookInside();
    if (openFiles.list.length > 1) paintOpenFiles();
}

/// The open files, as a strip of tabs you can click.
///
/// cian-tui puts them in a tab strip (`mouse.rs:243`); the window had
/// `◂ [2/5] ▸ name`, which says *how many* and never says **what**. F3 on
/// five marked files then gave you one name and a pair of arrows, and there
/// was no way to tell from the screen that four other files were open at all,
/// let alone which. Reported as exactly that: 「複数ファイルがタブ表示されて
/// いることが分かりにくい」.
///
/// So: one tab per file, named, the current one lit, each clickable, with the
/// arrows kept for the hand that is already on F2. The strip scrolls sideways
/// rather than squeezing — a tab too narrow to read its name is not a tab.
function paintOpenFiles() {
    const n = openFiles.list.length;
    if (n <= 1) { el.vName.textContent = viewer.name; return; }
    const arrow = (glyph, step, what) => {
        const b = document.createElement('span');
        b.className = 'vnavb';
        b.textContent = glyph;
        b.title = what;
        b.addEventListener('mousedown', (e) => {
            e.stopPropagation();
            openNth(openFiles.at + step);
        });
        return b;
    };
    const strip = document.createElement('span');
    strip.className = 'vtabs';
    let here = null;
    openFiles.list.forEach((path, i) => {
        const tab = document.createElement('span');
        tab.className = 'vtab' + (i === openFiles.at ? ' on' : '');
        // The name, not the path: the path is what the tab is *for*, and it
        // is in the tooltip for when two files share a name.
        tab.textContent = path.split(/[\\/]/).pop();
        tab.title = path;
        if (i === openFiles.at) here = tab;
        tab.addEventListener('mousedown', (e) => {
            e.stopPropagation();
            if (i !== openFiles.at) openNth(i);
        });
        strip.append(tab);
    });
    el.vName.replaceChildren(
        arrow('◂', -1, tr('previous file', '前のファイル')),
        strip,
        arrow('▸', 1, tr('next file', '次のファイル')),
    );
    // The lit tab may be off the end of a long strip.
    here?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
}


/// A link in the rendered markdown.
///
/// **Relative links used to say "not opened yet".** A README is mostly links
/// to its neighbours — `./CONTRIBUTING.md`, `docs/setup.md`, `#usage` — so a
/// preview that opens only `http:` opens almost nothing a repository's own
/// documentation points at.
///
/// Three kinds, and each goes where it means:
///   * `http:` / `mailto:` — the desktop's browser, as before. A file manager
///     that navigates away from itself is one you have to restart.
///   * `#anchor` — inside this preview. The engine's markdown gives headings
///     an `id`, so this is a scroll, not a load.
///   * anything else — a path beside the open file. It opens the way F3 does,
///     so the tab strip and Esc behave as they always do.
async function followMarkdownLink(href) {
    if (!href) return;
    if (/^https?:|^mailto:/i.test(href)) { ask('openurl', { url: href }); return; }
    if (href.startsWith('#')) {
        // `decodeURIComponent`, because a Japanese heading arrives percent
        // encoded and `getElementById` wants the character it stands for.
        let id = href.slice(1);
        try { id = decodeURIComponent(id); } catch { /* leave it as typed */ }
        const to = el.vRead.querySelector(`[id="${CSS.escape(id)}"]`)
            || el.vRead.querySelector(`[id="${CSS.escape(href.slice(1))}"]`);
        if (!to) { say(tr(`no heading called ${id}`, `${id} という見出しはありません`), true); return; }
        to.scrollIntoView({ block: 'start', behavior: 'smooth' });
        return;
    }
    // A path, relative to the file being previewed — and only the path: a
    // `docs/x.md#usage` link is a file and an anchor, and the file is the
    // half that has to exist.
    const here = openFiles.list[openFiles.at] || '';
    const dir = here.replace(/[\\/][^\\/]*$/, '');
    const sep = here.includes('\\') ? '\\' : '/';
    const clean = decodeURI(href.split('#')[0].split('?')[0]);
    if (!clean) return;
    const target = /^([a-zA-Z]:[\\/]|[\\/])/.test(clean)
        ? clean
        : `${dir}${sep}${clean.replace(/\//g, sep)}`;
    const r = await ask('stat', { path: target });
    if (!r) return;
    if (!r.exists) { say(tr(`${clean} is not there`, `${clean} は見つかりません`), true); return; }
    if (r.is_dir) {
        // A folder is a place, not a document: close the reader and go there,
        // which is what Enter on a folder does everywhere else.
        await closeView(false);
        await revealPath(target, true);
        return;
    }
    await closeView(false);
    if (!await landOn(target)) return;
    openFiles.list = [target];
    openFiles.at = 0;
    await lookInside();
}

/// Reading a file that only exists inside an archive.
///
/// It is extracted first, because everything downstream — the viewer, the
/// editor, the encoding switch — works on a path. Ctrl+S puts it back, and
/// the engine remembers which member it came from: a temporary file with no
/// idea where it came from is a file that can only be lost.
const member = { on: false };

/// A file on the server: downloaded, opened, and Ctrl+S uploads it back.
/// The same shape as an archive member, for the same reason — everything
/// downstream works on a path.
const remoteMember = { on: false };

async function openRemoteMember(which) {
    say(tr('fetching…', '落としています…'));
    const r = await ask('remoteview', { pane: which });
    if (!r) return false;
    remoteMember.on = true;
    const f = await ask('viewpath', { path: r.path });
    if (!f) { remoteMember.on = false; return false; }
    await showFile(f);
    el.vName.textContent = tr(`${r.name} (on the server)`, `${r.name}（サーバ上）`);
    el.vFoot.textContent = tr('Ctrl+S writes it back to the server   ·   Esc ×3 closes', 'Ctrl+S でサーバへ書き戻す   ·   Esc ×3 閉じる');
    return true;
}

async function openArchiveMember(which) {
    const r = await ask('archiveview', { pane: which });
    if (!r) return false;
    member.on = true;
    member.writable = !!r.writable;
    const at = { path: r.path, name: r.name };
    // The listing is inside the archive, so the ordinary read cannot find the
    // file; it is opened from the temporary by path instead.
    const f = await ask('viewpath', { path: at.path });
    if (!f) { member.on = false; return false; }
    await showFile(f);
    el.vName.textContent = tr(`${at.name} (inside the archive)`, `${at.name}（アーカイブの中）`);
    el.vFoot.textContent = member.writable
        ? tr('Ctrl+S writes it back into the archive   ·   Esc ×3 closes', 'Ctrl+S でアーカイブに書き戻す   ·   Esc ×3 閉じる')
        : tr('read-only (writing back into a tar is not built yet)   ·   Esc ×3 closes', '読むだけ（tar への書き戻しはまだ）   ·   Esc ×3 閉じる');
    return true;
}

async function openInEditor(which) {
    const pane = state[which];
    const row = pane && pane.entries[pane.cursor];
    if (row && /\.(png|jpe?g|gif|webp|bmp|svg|avif|ico|pdf)$/i.test(row.name)) {
        if (await openAsPicture(which)) return;
    }
    const f = await ask('view', { pane: which });
    if (!f) return;
    await showFile(f);
}

/// Put a file the engine has read into the editor.
///
/// Split out because two things reach here now — a file in the listing,
/// and a member extracted from an archive — and the second was going to
/// need a copy of all of it.
async function showFile(f) {
    let monaco;
    try {
        monaco = await loadMonaco();
    } catch (e) {
        say(e.message, true);
        return;
    }

    const enc = { Utf8: 'UTF-8', ShiftJis: 'Shift_JIS', Utf16Le: 'UTF-16LE', Utf16Be: 'UTF-16BE' };
    // Named, always. Which encoding it turned out to be is the question a
    // Japanese Windows machine asks of every file it did not write, and the
    // answer decides whether saving it is safe.
    viewer.about = [
        f.binary ? tr('binary (hex)', 'バイナリ（16進）') : (enc[f.encoding] || f.encoding),
        f.bom ? 'BOM' : null,
        f.binary ? null : f.eol.toUpperCase(),
        tr(`${f.lines.length} lines`, `${f.lines.length} 行`),
        human(f.bytes),
        f.truncated ? tr('※ the head only', '※先頭のみ') : null,
    ].filter(Boolean).join('  ·  ');
    // A hex dump is a rendering of the file, not the file. Saving one back
    // would write the dump, so it opens read-only and says so.
    viewer.readOnly = !!f.binary;
    viewer.name = f.name;
    // Where it came from, so the gutter can ask the engine what is on disk.
    // A hex dump has no line-for-line disk to compare against.
    viewer.path = f.binary ? null : (f.path || null);
    viewer.dirty = false;
    clearDiskDiff();
    setViewerOn(true);
    if (f.path) noteRecent(f.path, f.name);
    el.view.hidden = false;

    const text = f.lines.join('\n');
    const lang = MONACO_LANG[f.lang] || 'plaintext';
    makeEditor(monaco, text, lang);
    viewer.ed.updateOptions({ readOnly: viewer.readOnly });
    // After the text is in, not before: loading it is a change to the model,
    // and a file is not modified by having been opened.
    viewer.base = viewer.ed.getModel().getAlternativeVersionId();
    viewer.dirty = false;
    // A different file has different sections.
    sections = null;
    setStyle(style);
    el.vName.textContent = f.name;
    el.vAbout.textContent = viewer.about;
    viewer.ed.setPosition({ lineNumber: 1, column: 1 });
    viewer.ed.focus();
    drawViewFoot();
}

/// Attach or drop the vim grammar. Called on open and whenever the switch is
/// flipped, so the running editor changes under you rather than needing to be
/// closed and reopened.
function setStyle(i, remember = true) {
    style = (i + STYLES.length) % STYLES.length;
    if (remember) ask('remember', { key: 'gui_editor', value: STYLES[style][0] });
    if (!viewer.ed) return;
    if (viewer.vim) { viewer.vim.dispose(); viewer.vim = null; }
    // Cleared before vim takes the line. Otherwise the footer keeps whatever
    // was last written into it and vim's mode line appends to it — two status
    // lines in one, which is how it looked the first time.
    el.vFoot.textContent = '';
    if (STYLES[style][0] === 'vim') {
        // eslint-disable-next-line no-undef
        viewer.vim = MonacoVim.initVimMode(viewer.ed, el.vFoot);
        // The IME follows the mode: off when keys are commands, back when
        // they are text. monaco-vim announces every change, and syncIme reads
        // the mode back out of the footer — one rule for the whole window
        // rather than one for the editor and none for anywhere else.
        viewer.vim.on('vim-mode-change', () => queueMicrotask(syncIme));
        // `:w` and `:q` where the fingers put them. Without these, vim style
        // would still need Ctrl+S and Esc — which is exactly the seam that
        // makes a vim mode feel like a costume.
        // eslint-disable-next-line no-undef
        const ex = MonacoVim.VimMode.Vim;
        ex.defineEx('write', 'w', saveFile);
        ex.defineEx('quit', 'q', () => closeView(false));
        ex.defineEx('wq', 'wq', async () => { if (await saveFile()) closeView(false); });
        ex.defineEx('outline', 'outline', () => cmdOutline());
        ex.defineEx('blame', 'blame', () => cmdBlame());
        ex.defineEx('enc', 'enc', (_cm, params) => cmdEncoding((params.args || [])[0]));
        ex.defineEx('ws', 'ws', () => toggleWs());
        ex.defineEx('ruler', 'ruler', () => toggleRuler());
        ex.defineEx('preview', 'preview', () => togglePreview2());
        // cian-tui's viewer verbs, reachable from vim's command line the way
        // they are there. Without these, `:mermaid` and `:summary` answered
        // "Not an editor command" — they existed in the dictionary the listing
        // reads, and the viewer has a different one.
        ex.defineEx('mermaid', 'mermaid', (_cm, p) => ((p.argString || '').trim() === '!' || p.commandName === 'mermaid!' ? cmdMermaidOut() : cmdMermaid()));
        ex.defineEx('summary', 'summary', () => cmdSummary());
        ex.defineEx('edit', 'edit', () => cmdEditExternal());
        ex.defineEx('theme', 'theme', (_cm, p) => cmdTheme((p.args || [])[0]));
        ex.defineEx('combine', 'combine', (_cm, p) => cmdCombine((p.args || []).join(' ') + (p.argString || '')));
        // The line operations, which until now could not be reached at all:
        // each needs a file open, and cian's own `:` belongs to the listing,
        // which declines every key while a file is open. They were in the
        // command table, in the help, and unreachable — found by measuring
        // the code for twins, not by anybody using it. Here is where a vim
        // user would look for them anyway.
        for (const op of ['sort', 'rsort', 'uniq', 'han', 'zen', 'expand', 'unexpand', 'reindent']) {
            ex.defineEx(op, op, () => textOp(op));
        }
        // `:s/old/new/g`. monaco-vim has its own substitute, but it does not
        // know cian's — the engine holds the same one the terminal build
        // uses, so the two builds agree on what a pattern means.
        ex.defineEx('subst', 's', (_cm, p) => cmdSubstitute('s' + (p.argString || '')));
        // `:g/re/d` and `:v/re/d`, spelled as vim spells them.
        ex.defineEx('global', 'g', (_cm, p) => runGlobal(p, false));
        ex.defineEx('vglobal', 'v', (_cm, p) => runGlobal(p, true));
        // `]]` and `[[`, which monaco-vim does not have. `%` it does — it is
        // `moveToMatchedSymbol` and it works; the first version of this
        // replaced it with a worse one, which is what comes of adding a
        // feature without checking whether it is already there.
        // eslint-disable-next-line no-undef
        const vim = MonacoVim.VimMode.Vim;
        vim.defineAction('cianNextSection', () => hopSection(1));
        vim.defineAction('cianPrevSection', () => hopSection(-1));
        vim.mapCommand(']]', 'action', 'cianNextSection', {}, { isJump: true });
        vim.mapCommand('[[', 'action', 'cianPrevSection', {}, { isJump: true });
        // Folding, which monaco-vim also leaves out. Monaco does the folding;
        // this is only the key.
        vim.defineAction('cianFold', () => viewer.ed.trigger('cian', 'editor.toggleFold'));
        vim.mapCommand('za', 'action', 'cianFold');
        vim.mapCommand('zA', 'action', 'cianFold');
        armJJ();
    }
    // Sections, in both grammars: `]]` and `[[` walk the outline the way they
    // walk headings in vim.
    // (`armJJ` is defined below.)
    if (viewer.ed) {
        viewer.ed.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.BracketRight,
            () => hopSection(1));
        viewer.ed.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.BracketLeft,
            () => hopSection(-1));
    }
    drawViewFoot();
}

/// `]]` / `[[` — the next or previous section.
///
/// The outline decides what a section is, which is the same answer `:outline`
/// gives. Two ideas of "section" in one editor would be one too many.
let sections = null;

async function hopSection(step) {
    if (!viewer.on || !viewer.ed) return;
    if (!sections) {
        const r = await ask('outline', {});
        sections = r ? r.items.map((i) => i.line) : [];
    }
    if (!sections.length) { say(tr('no headings here', '見出しがありません')); return; }
    const now = viewer.ed.getPosition().lineNumber - 1;
    const to = step > 0
        ? sections.find((n) => n > now)
        : [...sections].reverse().find((n) => n < now);
    if (to === undefined) { say(step > 0 ? tr('that is the last heading', '最後の見出しです') : tr('that is the first heading', '最初の見出しです')); return; }
    viewer.ed.setPosition({ lineNumber: to + 1, column: 1 });
    viewer.ed.revealLineInCenter(to + 1);
}

/// `:g/re/d` from vim's command line. Only `d` is supported as the action,
/// which is the one everybody means — `:g/re/s/…` is `:s` with a filter and
/// that is a different command.
function runGlobal(params, keep) {
    const raw = (params.argString || (params.args || []).join(' ') || '').trim();
    const m = raw.match(/^\/(.*)\/\s*d\s*$/);
    if (!m) { say(tr('write it as :g/regex/d', ':g/正規表現/d の形で書いてください'), true); return; }
    cmdLineFilter(m[1], keep);
}

function drawViewFoot() {
    // The list of names is not a file, so the footer must not offer to save
    // one — it applies a rename, and saying 保存 there would describe
    // something that is not about to happen.
    if (renameList.on) {
        el.vFoot.textContent = tr(`${renameList.paths.length} names   one per line, and keep the order`, `${renameList.paths.length} 件   1行に1つ、順番は変えないこと`)
            + tr('   ·   Ctrl+S applies   Esc ×3 cancels', '   ·   Ctrl+S 適用   Esc ×3 取消');
        return;
    }
    if (viewer.readOnly) {
        el.vFoot.textContent = hex.editing
            ? tr(`hex edit — 0-9 a-f overwrite   ·   offset ${hex.at.toString(16).padStart(8, '0')}`, `16進編集 — 0-9 a-f で上書き   ·   ${hex.at.toString(16).padStart(8, '0')} 番地`)
              + tr(`${hex.half ? ' (waiting for the low digit)' : ''}   ·   Ctrl+S saves (keeping a .bak)   Esc back`, `${hex.half ? '（下位けた待ち）' : ''}   ·   Ctrl+S 保存（.bak を残します）   Esc 戻る`)
            : tr('hex — i edits   ·   Esc ×3 closes', '16進表示 — i で編集   ·   Esc ×3 閉じる');
        return;
    }
    // In vim style the footer is vim's own — its mode line and its `:` prompt
    // live there, and writing over them would take the command line away.
    if (viewer.vim) return;
    const at = viewer.ed && viewer.ed.getPosition();
    const where = at ? `${at.lineNumber} : ${at.column}` : '';
    el.vFoot.textContent = [
        where,
        viewer.dirty ? tr('unsaved', '未保存') : null,
        styleName(style),
        tr('Ctrl+S saves   Esc ×3 closes', 'Ctrl+S 保存   Esc ×3 閉じる'),
    ].filter(Boolean).join('   ·   ');
}

async function saveFile() {
    if (!viewer.ed) return false;
    if (pair.on) { await savePair(); return true; }
    if (scratch.on) { return saveScratch(); }
    if (remoteMember.on) {
        say(tr('sending to the server…', 'サーバへ送っています…'));
        const r = await ask('remotesave', { lines: viewer.ed.getValue().split(/\r?\n/) });
        if (!r) return false;
        viewer.base = viewer.ed.getModel().getAlternativeVersionId();
        viewer.dirty = false;
        // The file *is* the disk now, so the gutter has nothing left to say.
        clearDiskDiff();
        drawViewFoot();
        say(tr(`${r.saved} written back to the server`, `${r.saved} をサーバへ書き戻しました`));
        return true;
    }
    if (member.on) {
        if (!member.writable) { say(tr('writing back into a tar is not built yet', 'tar への書き戻しはまだです'), true); return false; }
        const r = await ask('archivesave', { lines: viewer.ed.getValue().split(/\r?\n/) });
        if (!r) return false;
        viewer.base = viewer.ed.getModel().getAlternativeVersionId();
        viewer.dirty = false;
        // The file *is* the disk now, so the gutter has nothing left to say.
        clearDiskDiff();
        drawViewFoot();
        say(tr(`${r.saved} written back into ${r.archive}`, `${r.saved} を ${r.archive} に書き戻しました`));
        return true;
    }
    if (viewer.readOnly) { say(tr('the hex view cannot be saved', '16進表示は保存できません'), true); return false; }
    // The editor is holding a list of names rather than a file's contents.
    if (renameList.on) {
        const ok = await applyRenameList();
        if (ok) { renameList.on = false; await closeView(false); }
        return ok;
    }
    const lines = viewer.ed.getValue().split(/\r?\n/);
    let r = await ask('save', { lines });
    if (!r) return false;
    // **Somebody else wrote to it while it was open.** The engine refuses
    // rather than writing, and this is the question that gets asked — because
    // the alternative is what used to happen: the save went through and their
    // work was gone, with nothing on screen to say it had been there.
    //
    // Not merged. cian is not a merge tool, and guessing on top of somebody
    // else's writing is worse than stopping.
    if (r.conflict) {
        const pick = await confirm(tr('It changed while you had it open', '開いている間に、ファイルが変わりました'),
            `${r.conflict}\n\n` + tr('Overwriting loses what they wrote.', '上書きすると、向こうの書いたものが消えます。'),
            {
                yes: tr('look at the difference', '差分を見る'),
                extras: [
                    { key: 'o', label: tr('overwrite anyway', 'それでも上書き') },
                    { key: 'a', label: tr('save as…', '別名で保存…') },
                ],
            });
        if (!pick) { say(tr('stopped — nothing was written', 'やめました。何も書いていません')); return false; }
        if (pick === 'a') return saveAsPrompt();
        if (pick !== 'o') { await showDiskDiff(); return false; }
        r = await ask('save', { lines, force: true });
        if (!r) return false;
    }
    viewer.base = viewer.ed.getModel().getAlternativeVersionId();
    viewer.dirty = false;
    clearDiskDiff();
    drawViewFoot();
    say(tr(`saved ${r.saved} (${r.lines} lines)`, `${r.saved} を保存しました（${r.lines} 行）`));
    // The listing shows a size and a date; both just changed.
    await reread();
    return true;
}

/// What is on disk now, beside what is in the editor.
///
/// The reader's own two-pane comparison, on a copy of the file as it stands —
/// so the question "what did they change" is answered by the thing that
/// already answers it, rather than by a second diff written for this dialog.
async function showDiskDiff() {
    const fresh = await ask('viewpath', { path: viewer.path });
    if (!fresh) return;
    show(tr('What is on disk now', 'いまディスクにあるもの'),
        tr('yours is still in the editor — nothing has been written', 'あなたの編集はエディタに残っています。まだ何も書いていません'),
        (fresh.lines || []).map((t, i) => ({ n: String(i + 1), label: t })),
        { foot: tr('Esc back to your copy', 'Esc 自分の編集に戻る') });
}

/// Write somewhere else instead, leaving theirs alone.
async function saveAsPrompt() {
    const here = (viewer.path || '').replace(/[\\/][^\\/]*$/, '');
    const name = await askFor(tr('save as', '別名で保存'), `${viewer.name}`, {
        wide: true,
        hint: tr('a name beside the original', '元と同じフォルダに、別の名前で'),
    });
    if (!name) { say(tr('stopped', 'やめました')); return false; }
    const sep = here.includes('\\') ? '\\' : '/';
    const r = await ask('saveas', { path: `${here}${sep}${name}`, lines: viewer.ed.getValue().split(/\r?\n/) });
    if (!r) return false;
    viewer.base = viewer.ed.getModel().getAlternativeVersionId();
    viewer.dirty = false;
    clearDiskDiff();
    drawViewFoot();
    say(tr(`saved as ${r.saved}`, `${r.saved} として保存しました`));
    await reread();
    return true;
}

/// Leaving. An unsaved file asks first — the only door out of an editor that
/// can lose work.
async function closeView(ask_first = true) {
    if (ask_first && viewer.dirty) {
        if (!await confirm(tr(`${viewer.name} has unsaved edits`, `${viewer.name} は未保存です`), tr('closing loses them', '閉じると編集は失われます'))) return;
    }
    setViewerOn(false);
    viewer.dirty = false;
    renameList.on = false;
    stopHex();
    reading = false;
    el.vRead.hidden = true;
    el.vRead.replaceChildren();
    member.on = false;
    remoteMember.on = false;
    scratch.on = false;
    if (pair.ed) { pair.ed.dispose(); pair.ed = null; }
    pair.on = false;
    // Only when the door is being used, not when stepping between files.
    if (ask_first) openFiles.list = [];
    if (viewer.vim) { viewer.vim.dispose(); viewer.vim = null; }
    el.vPic.replaceChildren();
    pic.node = null;
    el.vPic.hidden = true;
    el.vBody.hidden = false;
    el.view.hidden = true;
    el.status.focus?.();
}

/// Editing a binary, one byte at a time.
///
/// **Overwrite only.** Offsets never shift and the file cannot change size,
/// which is the difference between editing a binary and corrupting one: an
/// inserted byte moves everything after it, and in a binary those offsets are
/// usually written down inside the file itself.
///
/// Two hex digits make a byte, so the first one is remembered and shown as
/// pending rather than applied — half a byte written is a byte nobody meant.
const hex = { editing: false, at: 0, half: null };

function startHex() {
    if (!viewer.readOnly) return;
    hex.editing = true;
    hex.at = 0;
    hex.half = null;
    viewer.ed.updateOptions({ readOnly: true });
    markHexByte();
    drawViewFoot();
    say(tr('hex edit — 0-9 a-f overwrite, Ctrl+S saves', '16進編集 — 0-9 a-f で上書き、Ctrl+S で保存'));
}

function stopHex() {
    hex.editing = false;
    hex.half = null;
    if (viewer.ed) viewer.ed.deltaDecorations(hexMark, []);
    hexMark = [];
    drawViewFoot();
}

let hexMark = [];

/// Show which byte the next digit lands on. A hex editor with no cursor is a
/// hex editor you overwrite the wrong byte with.
function markHexByte() {
    if (!viewer.ed) return;
    const line = Math.floor(hex.at / 16) + 1;
    // The dump is `oooooooo  xx xx …` — two hex digits per byte, a space
    // between, and an extra space after the eighth.
    const col = 11 + (hex.at % 16) * 3 + (hex.at % 16 >= 8 ? 1 : 0);
    hexMark = viewer.ed.deltaDecorations(hexMark, [{
        range: new (window.monaco.Range)(line, col, line, col + 2),
        options: { inlineClassName: 'hexcur' },
    }]);
    viewer.ed.revealLineInCenterIfOutsideViewport(line);
}

async function hexDigit(ch) {
    const v = parseInt(ch, 16);
    if (Number.isNaN(v)) return;
    if (hex.half === null) {
        hex.half = v;
        drawViewFoot();
        return;
    }
    const byte = (hex.half << 4) | v;
    hex.half = null;
    const r = await ask('hexset', { at: hex.at, byte });
    if (!r) return;
    // One line back, not the file: a dump of a large binary is a lot of text
    // to resend because two digits changed.
    // Through the model, not the editor: `executeEdits` is a no-op while the
    // editor is read-only, and the editor has to stay read-only or the digits
    // would be typed into the dump as text. The first version saved the right
    // bytes and showed the old ones.
    const model = viewer.ed.getModel();
    const line = r.line + 1;
    model.applyEdits([{
        range: new (window.monaco.Range)(line, 1, line, model.getLineMaxColumn(line)),
        text: r.text,
    }]);
    viewer.dirty = true;
    hex.at += 1;
    markHexByte();
    drawViewFoot();
}

async function saveHex() {
    const r = await ask('hexsave', {});
    if (!r) return;
    viewer.dirty = false;
    await reread();
    say(tr(`saved ${r.saved} (the original is in ${r.backup})`, `${r.saved} を保存しました（元は ${r.backup} に残しました）`));
}

/// Three of the same key in a row is the way out.
///
/// The terminal build's rule, taken rather than invented — and the reason it
/// exists is worth keeping with it. One press must not close a file with
/// unsaved work in it, and Esc is pressed by reflex, so a single Esc closing
/// the editor would make it the most dangerous key on the keyboard in the one
/// grammar where it is hit without thinking. Three in a row is not a stray
/// keystroke.
///
/// It counts silently. A tally along the bottom, raised by a key pressed in
/// error, is noise exactly when it is least wanted; `?` says how to leave.
const wayOut = { key: null, times: 0 };

/// Whether vim is taking text right now.
///
/// Read off the mode line, because that is where vim itself says so — and
/// because this listener runs in the capture phase, before monaco-vim has
/// seen the key, the line still reads INSERT on the press that leaves insert
/// mode. Which is what is wanted: that press is leaving insert, not asking to
/// leave the file.
function vimTyping() {
    return !!viewer.vim && /INSERT|REPLACE/.test(el.vFoot.textContent || '');
}

document.addEventListener('keydown', (e) => {
    if (!viewer.on) return;
    // The picture keys. A terminal draws images as half-blocks, where zooming
    // has nothing to show; this window has the pixels.
    if (pic.node && !e.ctrlKey && !e.metaKey && !e.altKey) {
        if (e.key === '+' || e.key === '=') {
            e.stopPropagation(); e.preventDefault(); zoomPicture(1.25); return;
        }
        if (e.key === '-') { e.stopPropagation(); e.preventDefault(); zoomPicture(1 / 1.25); return; }
        if (e.key === '0') {
            e.stopPropagation(); e.preventDefault();
            pic.fit = false; pic.at = 1; pic.ox = 0; pic.oy = 0; paintPicture(); return;
        }
        if (e.key === 'f') {
            e.stopPropagation(); e.preventDefault();
            pic.fit = true; pic.ox = 0; pic.oy = 0; paintPicture(); return;
        }
    }
    // `ZZ` saves and closes, `ZQ` closes without saving.
    //
    // The two the fingers reach for first when leaving vim, and monaco-vim
    // has neither — `mapCommand('ZZ', …)` does not take, so this counts the
    // `Z` itself. `Z` alone is a prefix in vim and commands nothing, so
    // holding it costs nothing; anything else clears it.
    if (viewer.vim && !vimTyping() && !hex.editing
        && !e.ctrlKey && !e.metaKey && !e.altKey) {
        // 物理位置で見る ── `?` と同じ理由（配列と IME で `key` はぶれる）。
        if (e.code === 'KeyZ' && e.shiftKey) {
            e.stopPropagation();
            e.preventDefault();
            if (viewer.zPending) {
                viewer.zPending = false;
                saveFile().then((ok) => { if (ok) closeView(false); });
            } else {
                viewer.zPending = true;
            }
            return;
        }
        if (viewer.zPending && e.code === 'KeyQ' && e.shiftKey) {
            viewer.zPending = false;
            e.stopPropagation();
            e.preventDefault();
            // `ZQ` is "leave, and I mean it" — `closeView(false)` is the door
            // that does not ask, which is the whole of what ZQ means.
            closeView(false);
            return;
        }
        viewer.zPending = false;
    }
    // `?` — what can be done *in here*, which is not the same question as
    // what cian can do.
    //
    // **The hint bar has advertised `? キー一覧` since the bar was written and
    // nothing was bound to it**, so the key went to vim, which reads `?` as
    // "search backwards" and put up a box saying `? (javaScript regexp)`.
    // cian-tui binds it (viewer.rs:1065) to the viewer's own manual; this is
    // the same idea over the section the window's help already has.
    //
    // Only in vim style, and only when the keys are commands: in notepad
    // style `?` is a character, and so it is in the middle of a word.
    // `e.key` で当てるだけでは足りない ── 日本語キーボードでは同じ刻印が
    // 違う `key` で届きうるし、IME が拾っている間は `Process` になる。
    // `?` は JIS でも US でも `Slash` の位置なので、物理キーでも受ける。
    if ((e.key === '?' || (e.code === 'Slash' && e.shiftKey))
        && viewer.vim && !vimTyping() && !hex.editing
        && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.stopPropagation();
        e.preventDefault();
        viewerHelp();
        return;
    }
    // Shift+Enter opens the file's own menu, the key cian-tui puts it on in
    // the viewer as well as in a listing. Before the editor sees it: in
    // notepad style a plain Enter is a newline, and this one is not.
    if (e.key === 'Enter' && e.shiftKey && !hex.editing) {
        e.stopPropagation();
        e.preventDefault();
        openMenu(VIEWER_MENU);
        return;
    }
    // The hex editor owns its keys while it is on.
    if (hex.editing) {
        if (e.key === 'Escape') { e.stopPropagation(); e.preventDefault(); stopHex(); return; }
        if (e.key === 's' && mod(e)) {
            e.stopPropagation();
            e.preventDefault();
            saveHex();
            return;
        }
        if (/^[0-9a-fA-F]$/.test(e.key) && !e.ctrlKey && !e.metaKey) {
            e.stopPropagation();
            e.preventDefault();
            hexDigit(e.key);
            return;
        }
        // Moving between bytes, so a mistake is walked back to rather than
        // restarted from the top.
        const step = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: 16, ArrowUp: -16 }[e.key];
        if (step) {
            e.stopPropagation();
            e.preventDefault();
            hex.at = Math.max(0, hex.at + step);
            hex.half = null;
            markHexByte();
            drawViewFoot();
            return;
        }
    }
    if (viewer.readOnly && !hex.editing && e.key === 'i') {
        e.stopPropagation();
        e.preventDefault();
        startHex();
        return;
    }
    // Not while the question is up. Esc answers it — and counting those
    // presses toward another way out would mean declining to close three
    // times and being asked a fourth.
    if (!el.ask.hidden) { wayOut.key = null; wayOut.times = 0; return; }

    // F3 is nobody's editing key, so it is the one door that opens on a single
    // press. Esc and Backspace are both, which is why they take three.
    // Between the differences, when two files are side by side.
    if (e.key === 'F7' && pair.ed) {
        e.stopPropagation();
        e.preventDefault();
        hopDiff(e.shiftKey ? -1 : 1);
        return;
    }
    // `L` — the same comparison as a list. The editor is the better way to
    // *read* a difference; the list is the better way to take one out of the
    // window (`c`, `w`) or ask about it (`x`), and it folds the identical runs
    // away. Both, one key apart, rather than one of them behind the other.
    if (e.key === 'L' && pair.on && !e.ctrlKey && !e.metaKey) {
        e.stopPropagation();
        e.preventDefault();
        compareAsList = true;
        closeView(false).then(() => cmdCompare());
        return;
    }
    // Reading the rendered document: Esc goes back to the source rather than
    // out of the file, because "back one step" is what Esc means everywhere
    // else in here. (Ctrl+E does both directions, and is taken in the capture
    // listener further down — Monaco claims that chord.)
    if (reading && e.key === 'Escape') {
        e.stopPropagation();
        e.preventDefault();
        togglePreview2();
        return;
    }
    if (e.key === 'F3') {
        e.stopPropagation();
        e.preventDefault();
        wayOut.key = null;
        wayOut.times = 0;
        closeView();
        return;
    }
    // Between the open files, when there is more than one.
    // The grep's hits, from inside the file one of them opened.
    if (e.key === 'n' && mod(e)) {
        e.stopPropagation();
        e.preventDefault();
        hopHit(e.shiftKey ? -1 : 1);
        return;
    }
    if ((e.key === 'F2') && openFiles.list.length > 1) {
        e.stopPropagation();
        e.preventDefault();
        openNth(openFiles.at + (e.shiftKey ? -1 : 1));
        return;
    }

    // A comparison closes on one Esc.
    //
    // The three-press rule exists for a file you are *editing*: Esc is vim's
    // way out of half a command, and a single one must not throw the file
    // away. A side-by-side comparison is something you opened to look at, and
    // three presses to put it down is three presses. `closeView` still asks
    // if either side has unsaved edits, which is the thing the rule was
    // protecting.
    if (e.key === 'Escape' && pair.on && !vimTyping()) {
        e.stopPropagation();
        e.preventDefault();
        wayOut.key = null;
        wayOut.times = 0;
        closeView();
        return;
    }
    // Backspace deletes in notepad style, so it is not offered as a way out
    // there. In vim style it is, but not while insert mode has the keyboard.
    const doors = viewer.vim
        ? (vimTyping() ? [] : ['Escape', 'Backspace'])
        : ['Escape'];
    if (!doors.includes(e.key)) {
        wayOut.key = null;
        wayOut.times = 0;
        return;
    }
    // The same key three times. Esc, Backspace, Esc is three presses and no
    // intent — it is a hand looking for something.
    if (wayOut.key !== e.key) {
        wayOut.key = e.key;
        wayOut.times = 0;
    }
    wayOut.times += 1;
    if (wayOut.times < 3) return;
    wayOut.key = null;
    wayOut.times = 0;
    // Not stopped on the way through: the editor still gets its Esc, which is
    // how vim leaves whatever it was in the middle of. The third press asks
    // only when there is something to lose.
    closeView();
}, true);

/// Ctrl+Enter: a folder to the other pane, a file to your own application.
///
/// One key with two answers, because that is the terminal build's — and the
/// two share a question ("open this somewhere other than here") rather than
/// being two features that happen to sit on one chord.
async function openOut() {
    const r = await ask('open', { pane: state.focus });
    if (!r) return;
    if (r.view) {
        state[r.pane] = r.view;
        draw(r.pane);
        say(tr(`opened ${r.name} on the ${r.pane === 'left' ? 'left' : 'right'}`, `${r.name} を${r.pane === 'left' ? '左' : '右'}で開きました`));
    } else {
        say(tr(`opened ${r.opened} in its default app`, `${r.opened} を既定のアプリで開きました`));
    }
}

// ─────────────────────────────────────────────────────────────────────────
// The commands, and the two ways to reach them.
//
// `:` for the name you already know, `C` for fuzzy-finding the one you don't.
// Both run the same table, which is the point: a command added here gets a
// prompt, a palette entry and a help line without any of them being written.
// The terminal build reached the same arrangement, and for the same reason —
// it has far more commands than it has keys.
// ─────────────────────────────────────────────────────────────────────────
/// The dictionary, built for the language that is on.
///
/// Same reason as `helpRows`: a list of descriptions frozen at startup is a
/// list that stops agreeing with the rest of the window the moment somebody
/// switches. Cached per language, because `findCommand` runs on every `:`
/// keystroke and this is a hundred and thirty-odd objects.
let commandsFor = { lang: null, list: null };

function commands() {
  if (commandsFor.lang === lang) return commandsFor.list;
  commandsFor = { lang, list: buildCommands() };
  return commandsFor.list;
}

function buildCommands() {
  return [
    { name: 'count', about: tr("count files, lines and steps", 'ファイル・ステップ数を数える'), run: cmdCount },
    { name: 'du', alias: ['diskusage'], about: tr("disk usage \u2014 what's biggest here", '容量分析 — 何が大きいか'), run: cmdDu },
    { name: 'attr', about: tr("permissions & owner", '属性を見る'), run: cmdAttr },
    { name: 'chmod', about: tr("change the mode (e.g. :chmod 644)", 'モードを変える（例 :chmod 644）'), arg: tr('Mode', 'モード'), run: cmdChmod },
    { name: 'readonly', about: tr("set / clear read-only (on by default)", '読み取り専用にする / 解除（既定 on）'), run: cmdReadonly },
    // `:md5` and `:sha256` are verbs of their own in cian-tui (commands.rs:402).
    { name: 'hash', alias: ['md5', 'sha256'], about: tr("checksum (sha256 by default; :hash md5 too)", 'チェックサム（既定 sha256、:hash md5 も）'), run: (a, as_) => cmdHash(as_ === 'md5' ? 'md5' : a) },
    { name: 'find', about: tr("find by name, through the whole tree", '名前で探す（この下すべて）'), arg: tr('name', '名前'), run: (a) => cmdSearch('name', a) },
    { name: 'grep', about: tr("search inside files, through the whole tree", 'ファイルの中を探す（この下すべて）'), arg: tr('a string, or /a regex/', '文字列か /正規表現/'), run: (a) => cmdSearch('content', a) },
    { name: 'branch', about: tr("flatten everything below here, one file per line", 'この配下を1ファイル1行に平坦化'), run: cmdBranch },
    { name: 'diff', alias: ['compare'], about: tr("compare the two panes (also =)", '左右を比較（= でも）'), run: cmdCompare },
    { name: 'diffedit', about: tr("open the two files side by side, both editable", '左右のファイルを並べて、どちらも編集できる形で開く'), run: cmdDiffEdit },
    { name: 'renamepattern', about: tr("bulk rename by a pattern: {name}_{n3}.{ext}", '一括リネーム: {name}_{n3}.{ext}'), arg: tr('pattern', 'パターン'), run: cmdRenamePattern },
    { name: 'zip', about: tr("zip the marks (:zip -e for a password)", 'マークを zip に（:zip -e でパスワード付き）'), arg: '-e', optional: true, run: (a) => cmdCompress('zip', /-e/.test(a || '')) },
    { name: 'tar', about: tr("tar the marks", 'マークを tar にまとめる'), run: () => cmdCompress('tar') },
    { name: 'targz', about: tr("tar.gz the marks", 'マークを tar.gz にまとめる'), run: () => cmdCompress('targz') },
    { name: 'unzip', alias: ['extract'], about: tr("extract the archive under the cursor, here", 'カーソルのアーカイブをここに展開'), run: cmdExtract },
    { name: 'lsar', about: tr("list an archive\u2019s contents", 'アーカイブの中身を見る'), run: cmdArchiveList },
    { name: 'log', about: tr("the commit log (git / svn)", 'コミットログ（git / svn）'), run: () => cmdLog(false) },
    { name: 'filelog', about: tr("this file's history", 'このファイルの履歴'), run: () => cmdLog(true) },
    { name: 'gitdiff', alias: ['gdiff', 'svndiff'], about: tr("the selected file's diff (git / svn)", '選択ファイルの差分（git / svn）'), run: () => cmdVcsDiff(null) },
    { name: 'stage', alias: ['add', 'svnadd'], about: tr("git add", 'git add'), run: () => cmdVcs('stage') },
    { name: 'unstage', alias: ['reset'], about: tr("git reset", 'git reset'), run: () => cmdVcs('unstage') },
    { name: 'discard', alias: ['revert', 'svnrevert'], about: tr("discard worktree changes", '作業ツリーの変更を破棄'), run: () => cmdVcs('discard') },
    { name: 'dup', alias: ['duplicate', 'dedup'], about: tr("find files with identical contents", '中身が同じファイルを探す'), run: cmdDedup },
    { name: 'redo', about: tr("redo what u undid", 'u で取り消した操作をやり直す'), run: redo },
    { name: 'image', about: tr("how images are drawn (a window always draws them)", '画像の表示方式（窓では常に描画されます）'), run: () => say(tr('a window always draws images — just press F3', '窓では画像は常に表示されます — F3 でどうぞ')) },
    // `finder` is NOT an alias here: it is `:files`'s, and a spelling that
    // lives on two commands reaches only the first — the fuzzy finder its own
    // about-text promised could never open. `:view finder` still works as an
    // argument (cmdView maps it to details).
    { name: 'view', alias: ['classic', 'details'], about: tr("which mode the window is in \u2014 :view classic | details", 'モード — :view classic | details'), arg: 'classic / details', optional: true, run: cmdView },
    { name: 'shell', about: tr("open the shell panel (also Shift+J)", 'シェルパネルを開く（Shift+J でも）'), run: openShell },
    { name: 'remote', alias: ['sftp'], about: tr("open a server in this pane (SFTP)", 'このペインでサーバを開く（SFTP）'), run: cmdSftpPicker },

    { name: 'ssh', about: tr("ssh to a host, in the shell panel (also Shift+S)", 'ホストへ ssh（シェルパネルで。Shift+S でも）'), run: cmdSshPicker },
    { name: 'paste', about: tr("paste the held files here (also Ctrl+V / y)", '保持したファイルをここへ貼り付け（Ctrl+V / y でも）'), run: paste },
    { name: 'local', about: tr("close the server and come back to this disk", 'サーバを閉じてローカルへ戻る'), run: cmdDisconnect },
    { name: 'aicmd', about: tr("AI: a shell command from a description", 'AI: 説明からシェルコマンドを作る'), arg: tr('what you want', 'やりたいこと'), run: cmdAiCmd },
    { name: 'ailog', alias: ['logtriage', 'triage'], about: tr("AI: triage the selected log", 'AI: 選択したログを診断する'), run: cmdAiLog },
    // The argument is optional now that this opens a conversation rather than
    // asking one question: `:ai` on its own is cian-tui's `new_ai_chat`, an
    // empty window with the caret in it. It was required, so `:ai` stopped to
    // demand a question before it would show you the place to type one.
    { name: 'ai', alias: ['chat'], about: tr("AI - simple: chat with the local model", 'AI - simple: ローカルモデルとチャット'), arg: tr('what to ask', '訊きたいこと'), optional: true, run: cmdAiAsk },
    // Not `explain`: that word is cian-tui's `:aierror` (commands.rs:327), and
    // having it mean "explain the diff" here made one name do two jobs.
    { name: 'aidiff', alias: ['explaindiff'], about: tr("AI: explain the diff on screen", 'AI: 表示中の差分を説明する'), run: cmdAiDiff },
    { name: 'office', about: tr("open the cloud copy of an Office document", 'Office 文書のクラウド側を開く'), run: () => cmdOffice('office') },
    { name: 'officelink', about: tr("write a .url to the cloud copy (this is the one to paste in a mail)", 'クラウド側への .url を作る（メールに貼るのはこれ）'), run: () => cmdOffice('officelink') },
    { name: 'reload', about: tr("reload init.lua", 'init.lua を読み直す'), run: cmdReload },
    { name: 'key', about: tr("report each key as received (again stops)", '受け取ったキーをそのまま表示（もう一度で止める）'), run: toggleKeyEcho },
    { name: 'bookmark', about: tr("bookmark where you are", 'いまの場所を登録する'), arg: tr('name', '名前'), optional: true, run: cmdBookmark },
    { name: 'macro', about: tr("run a macro (also @)", 'マクロを実行（@ でも）'), run: cmdMacros },
    { name: 'sync', alias: ['broadcast'], about: tr("shell: type into every pane at once (also Ctrl+S)", 'シェル: 全ペインに同時入力（Ctrl+S でも）'), run: cmdSync },
    { name: 'snip', alias: ['snippet'], about: tr("a saved command, to the shell (also Ctrl+Shift+Enter)", '保存したコマンドをシェルへ（Ctrl+Shift+Enter でも）'), run: cmdSnippets },
    { name: 'sessionlog', alias: ['log2'], about: tr("record the shell to a file, or stop", 'シェルの写しをファイルに取る／止める'), run: cmdShellLog },
    { name: 'shellname', alias: ['tabname'], about: tr("name this shell tab (double-clicking the tab does it too)", 'このシェルタブに名前を付ける（タブを二度押しでも）'), arg: tr('name', '名前'), optional: true, run: cmdShellName },
    { name: 'zoom', about: tr("zoom whichever surface has the keys, and back (also F12)", 'いま操作している面を広げる／戻す（F12 でも）'), run: zoomFocused },
    { name: 'df', about: tr("free space on the disk", 'ディスクの空き容量'), run: cmdDf },
    { name: 'wc', about: tr("lines / words / bytes", '行／単語／バイト数'), run: cmdWc },
    { name: 'head', about: tr("the first lines (:head -n 20)", '先頭だけ見る（:head -n 20）'), arg: tr('-n count', '-n 数'), optional: true, run: (a) => cmdPeek(a, false) },
    { name: 'tail', about: tr("the last lines (:tail -n 20)", '末尾だけ見る（:tail -n 20）'), arg: tr('-n count', '-n 数'), optional: true, run: (a) => cmdPeek(a, true) },
    { name: 'recent', alias: ['oldfiles'], about: tr("recently-opened files", '最近開いたファイル'), run: cmdRecent },
    { name: 'version', alias: ['about'], about: tr("the version, and where it lives", '版と居場所'), run: cmdVersion },
    { name: 'man', about: tr("the key manual (same as :help)", 'キー一覧（:help と同じ）'), run: openHelp },
    { name: 'jump', about: tr("jump to a bookmark or somewhere you have been (also Z)", '登録した場所と履歴へ飛ぶ（Z でも）'), run: cmdJump },
    { name: 'palette', about: tr("every command (also C)", 'コマンド一覧（C でも）'), run: openPalette },
    { name: 'selectall', alias: ['markall'], about: tr("mark everything (also Ctrl+A)", '全部マーク（Ctrl+A でも）'), run: () => mark(true) },
    { name: 'ren', alias: ['rename'], about: tr("rename (also r)", 'リネーム（r でも）'), run: rename },
    { name: 'untar', about: tr("extract here (same as :unzip)", 'ここに展開（:unzip と同じ）'), run: cmdExtract },
    { name: 'step', about: tr("files and steps (same as :count)", 'ファイル数とステップ数（:count と同じ）'), run: cmdCount },
    { name: 'files', alias: ['finder'], about: tr("fuzzy-find a file below here (also //)", 'この下のファイルをあいまい検索（// でも）'), run: openFinder },
    { name: 'where', alias: ['config'], about: tr("where cian reads and writes its config", 'cian が読み書きする設定ファイルの場所'), run: cmdWhere },
    { name: 'mark', about: tr("mark by wildcard (:mark *.rs)", 'ワイルドカードでマーク（:mark *.rs）'), arg: tr('pattern', 'パターン'), run: (a) => cmdMarkGlob(a, true) },
    { name: 'unmark', alias: ['deselect'], about: tr("unmark by wildcard", 'ワイルドカードでマークを外す'), arg: tr('pattern', 'パターン'), run: (a) => cmdMarkGlob(a, false) },
    { name: 'copyto', about: tr("copy to a named place", '指定した場所へコピー'), arg: tr('where to', '行き先'), run: (a) => cmdTo('copyto', a) },
    { name: 'moveto', about: tr("move to a named place", '指定した場所へ移動'), arg: tr('where to', '行き先'), run: (a) => cmdTo('moveto', a) },
    { name: 'revealos', alias: ['showinfinder'], about: tr("reveal in Finder / Explorer", 'Finder / エクスプローラで表示'), run: cmdRevealOs },
    { name: 'edit', alias: ['e'], about: tr("open in the external editor ($EDITOR)", '外部エディタで開く（$EDITOR）'), run: cmdEditExternal },
    { name: 'vi', alias: ['vim', 'nvim'], about: tr("open the file in that editor, in a shell tab of its own", 'そのエディタを新しいシェルタブで開く'), run: cmdEditorTab },
    { name: 'editstyle', alias: ['notepad', 'vimkey'], about: tr("editor keys \u2014 :editstyle vim / :notepad", 'エディタのキー操作 — :editstyle vim / :notepad'), arg: 'vim / notepad', optional: true, run: cmdEditStyle },
    { name: 'scratch', alias: ['new'], about: tr("a scratch buffer (:w gives it a name)", '下書きを開く（:w で名前を付けて保存）'), run: cmdScratch },
    { name: 'limit', alias: ['speed', 'ratelimit'], about: tr("cap the transfer rate \u2014 :limit 2m / 500k / off", '転送の速さの上限 — :limit 2m / 500k / off'), arg: '2m / 500k / off', optional: true, run: cmdLimit },
    { name: 'summary', alias: ['summarize', 'summarise'], about: tr("AI: summarise the open file", 'AI: 開いているファイルを要約'), run: cmdSummary },
    { name: 'aicommit', alias: ['commitmsg'], about: tr("AI: a commit message from the staged diff", 'AI: ステージ済みの差分からコミットメッセージを作る'), run: cmdAiCommit },
    { name: 'aijunk', alias: ['junk'], about: tr("AI: detect junk files \u2014 no contents are sent", 'AI: ゴミファイル検出 — 中身は送りません'), run: () => cmdAiScan('aijunk') },
    { name: 'aistructure', alias: ['organize', 'aiorganize'], about: tr("AI: suggest a folder structure \u2014 the whole plan first", 'AI: ディレクトリ構成を提案 — 実行前に全部見せます'), run: () => cmdAiScan('aistructure') },
    { name: 'airename', about: tr("AI: rename by instruction (:airename to snake_case)", 'AI: 指示でリネーム（:airename snake_case に）'), arg: tr('how to change them', 'どう変えるか'), run: cmdAiRename },
    { name: 'aisearch', alias: ['ask', 'semsearch'], about: tr("AI: semantic search (:aisearch last month's invoices)", 'AI: セマンティック検索（:aisearch 先月の請求書）'), arg: tr('what to find', '探しもの'), run: cmdAiSearch },
    { name: 'aierror', alias: ['explain'], about: tr("AI: explain the shell's last error", 'AI: シェルの直近のエラーを説明する'), run: cmdAiError },
    { name: 'ime', alias: ['inputmethod'], about: tr("input method \u2014 off in vim's normal mode (cian.ime)", 'IME 連携 — vim のノーマルモードで自動オフ（cian.ime）'), run: cmdIme },
    { name: 'stat', about: tr("attributes (same as :attr)", '属性（:attr と同じ）'), run: cmdAttr },
    { name: 'blame', about: tr("who last changed each line of the open file", '各行を最後に変えた人（開いているファイル）'), run: cmdBlame },
    { name: 'enc', about: tr("re-read the open file under another encoding", '開いているファイルの文字コードを変えて読み直す'), arg: 'utf8 / sjis / utf16le / utf16be', optional: true, run: cmdEncoding },
    { name: 'ws', about: tr("show or hide tabs, trailing spaces and the rest", 'タブ・行末の空白などを見せる／隠す'), run: toggleWs },
    { name: 'ruler', about: tr("show or hide the column ruler", '桁の目盛りを出す／消す'), run: toggleRuler },
    { name: 's', about: tr("replace in the open file, s/old/new/g", '開いているファイルを置換 s/古い/新しい/g'), arg: 's/…/…/', run: cmdSubstitute },
    { name: 'g', about: tr("delete the matching lines (:g/re/d)", '一致した行を削除（:g/re/d）'), arg: tr('regex', '正規表現'), run: (a) => cmdLineFilter(a, false) },
    { name: 'v', about: tr("keep only the matching lines (:v/re/d)", '一致した行だけ残す（:v/re/d）'), arg: tr('regex', '正規表現'), run: (a) => cmdLineFilter(a, true) },
    { name: 'combine', about: tr("join the next line (:combine 3 for three; :combine! without a space)", '次の行を連結（:combine 3 で3行、:combine! は空白なし）'), arg: tr('how many lines', '行数'), optional: true, run: cmdCombine },
    { name: 'theme', alias: ['colorscheme', 'colourscheme'], about: tr("twenty-one palettes \u2014 choosing one dresses the window (also in T\u2019s menu)", '配色 21 種 — 選ぶだけで着せ替わります（T のメニューにも）'), arg: tr('name', '名前'), optional: true, run: cmdTheme },
    { name: 'redraw', alias: ['refresh!'], about: tr("redraw the screen", '画面を描き直す'), run: () => { draw('left'); draw('right'); say(tr('redrawn', '描き直しました')); } },
    { name: 'preview', about: tr("follow the cursor and show what it is on (again stops)", 'カーソルのファイルを追って表示（もう一度で止める）'), run: togglePreview },
    // cian-tui's `:mermaid` opens the file's diagrams in a browser. The window
    // draws them in the preview, but the browser is still the place you go to
    // make one big enough to read — so the verb exists here too, and does the
    // same thing.
    { name: 'mermaid', about: tr("draw the mermaid diagrams (:mermaid! for a browser)", 'mermaid 図を描く（:mermaid! でブラウザ）'), run: (a, as_) => (as_ === 'mermaid!' ? cmdMermaidOut() : cmdMermaid()), alias: ['mermaid!'] },
    { name: 'render', alias: ['source'], about: tr("set the Markdown (also Ctrl+E)", 'Markdown を組んで表示（Ctrl+E でも）'), run: togglePreview2 },
    { name: 'queue', about: tr("what is running, and how to stop it", '実行中の操作を見る・止める'), run: cmdQueue },
    { name: 'tab', about: tr("a new tab (also t / F9)", '新しいタブ（t / F9 でも）'), run: () => tabNew() },
    { name: 'tabclose', about: tr("close the tab (also w / F10)", 'タブを閉じる（w / F10 でも）'), run: () => tabClose() },
    // The short ones the terminal build has, spelled the same way. A person who
    // knows `:mkdir -p` should not have to find out that this one is different.
    { name: 'mkdir', alias: ['md'], about: tr("make a folder (:mkdir -p a/b/c)", 'ディレクトリを作る（:mkdir -p a/b/c）'), arg: tr('name', '名前'), run: cmdMkdir },
    { name: 'touch', about: tr("create a file, or touch its time", 'ファイルを作る／時刻を更新'), arg: tr('name', '名前'), run: cmdTouch },
    { name: 'cp', alias: ['copy'], about: tr("copy \u2014 to the other pane with no argument, or :cp <where>", 'コピー — 引数なしで反対ペインへ、:cp <行き先> でそこへ'), arg: tr('where to', '行き先'), optional: true, run: (a) => a ? cmdTo('copyto', a) : operate('copy') },
    { name: 'mv', alias: ['move'], about: tr("move \u2014 to the other pane with no argument, or :mv <where>", '移動 — 引数なしで反対ペインへ、:mv <行き先> でそこへ'), arg: tr('where to', '行き先'), optional: true, run: (a) => a ? cmdTo('moveto', a) : operate('move') },
    { name: 'rm', alias: ['del', 'delete'], about: tr("delete (to the trash)", '削除（ゴミ箱へ）'), run: () => operate('delete') },
    { name: 'pwd', about: tr("show this folder and put it on the clipboard", 'いまの場所を表示してクリップボードへ'), run: cmdPwd },
    { name: 'ls', alias: ['dir'], about: tr("reload (:ls -a toggles the dotfiles)", '読み直す（:ls -a で隠しファイル切替）'), run: cmdLs },
    { name: 'q', alias: ['quit'], about: tr("quit (it asks)", '閉じる（確認します）'), run: cmdQuit },
    { name: 'each', about: tr("a command per marked file \u2014 {} is the path", 'マーク各ファイルにコマンド — {} がパス'), arg: tr('command', 'コマンド'), run: cmdEach },
    { name: 'nobom', alias: ['stripbom'], about: tr("strip UTF-8 BOMs (UTF-16 is left alone)", 'UTF-8 BOM を除去（UTF-16 は触らない）'), run: cmdNoBom },
    { name: 'renamelist', about: tr("rename by editing the list of names", '名前の一覧を編集してリネーム'), run: cmdRenameList },
    { name: 'outline', about: tr("the headings of the open file", '開いているファイルの見出し一覧'), run: cmdOutline },
    { name: 'sort', about: tr("sort the lines of the open file", '開いているファイルの行をソート'), run: () => textOp('sort') },
    { name: 'rsort', about: tr("sort the lines in reverse", '行を逆順ソート'), run: () => textOp('rsort') },
    { name: 'uniq', about: tr("drop duplicate lines", '重複行を落とす'), run: () => textOp('uniq') },
    { name: 'han', about: tr("full-width ASCII \u2192 half-width", '全角ASCII → 半角'), run: () => textOp('han') },
    { name: 'zen', about: tr("half-width kana \u2192 full-width", '半角カナ → 全角'), run: () => textOp('zen') },
    { name: 'expand', about: tr("leading tabs \u2192 spaces", '行頭のタブ → スペース'), run: () => textOp('expand') },
    { name: 'unexpand', about: tr("leading spaces \u2192 tabs", '行頭のスペース → タブ'), run: () => textOp('unexpand') },
    { name: 'reindent', about: tr("re-indent to a consistent step", 'インデントを揃える'), run: () => textOp('reindent') },
    { name: 'lf', about: tr("line endings to LF", '改行を LF にする'), run: () => setEol('lf') },
    { name: 'crlf', about: tr("line endings to CRLF", '改行を CRLF にする'), run: () => setEol('crlf') },
    { name: 'svnupdate', about: tr("svn update", 'svn update'), run: () => cmdSvn('update') },
    { name: 'svncommit', about: tr("svn commit (it asks for a message)", 'svn commit（メッセージを訊きます）'), run: () => cmdSvn('commit') },
    { name: 'svnresolve', alias: ['resolve'], about: tr("svn resolve --accept working", 'svn resolve --accept working'), run: () => cmdSvn('resolve') },
    { name: 'visual', alias: ['select'], about: tr("visual selection (also v)", 'ビジュアル選択（v でも）'), run: startVisual },
    // `:back` is cian-tui's name for the history popup (commands.rs:248), not
    // for stepping one directory back — that is Alt+← and `:cd -`, as it is
    // there. The two builds had the same word doing two different things.
    { name: 'back', alias: ['history'], about: tr("this pane's directory history", 'このペインの移動履歴'), run: cmdHistory },
    { name: 'forward', about: tr("forward, one directory", 'ひとつ先のディレクトリへ'), run: () => step('forward') },

    { name: 'cd', alias: ['goto'], about: tr(":cd <path> / :cd .. / :cd - / :cd ~", ':cd <パス> / :cd .. / :cd - / :cd ~'), arg: tr('path', 'パス'), run: cmdCd },
    { name: 'hidden', about: tr("show / hide dotfiles", '隠しファイルの表示切替'), run: toggleHidden },
    { name: 'refresh', alias: ['rescan'], about: tr("reload", '読み直す'), run: reread },
    { name: 'undo', about: tr("undo the last operation", '直前の操作を取り消す'), run: undo },
    // Two commands, not one alias for both: cian-tui's `:menu` is the
    // right-click menu and `:toggle` is the switches (commands.rs:164/219).
    // Here they were aliases of each other, both landing on the switches — so
    // `:menu` opened something else entirely.
    { name: 'menu', about: tr("the right-click menu", '右クリックメニュー'), run: () => openMenu(CONTEXT) },
    { name: 'toggle', about: tr("the switches menu (T)", 'UIトグルメニュー（T）'), run: () => openMenu(TOGGLES) },
    { name: 'help', alias: ['h'], about: tr("the key manual", 'キー一覧'), run: openHelp },
  ];
}

/// `:q` — with the question, as the terminal build asks it. A window's ✕
/// button exists, so anyone typing :q is a person whose hands close things
/// by keyboard — and a typo away from :w.
/// `:cd`, the four ways the terminal build spells it. `-` is the previous
/// directory — the pane's own history already remembers it, so it is `back`
/// by another name. `~` and relatives resolve in the engine, against the
/// pane rather than against wherever the engine process was started.
async function cmdCd(dest) {
    if (dest.trim() === '-') { await step('back'); return; }
    await goToPath(dest.trim());
}

/// `q` / `:q` — 終わる。
///
/// **`q` が何にも割り当てられていなかった。** `:q` はあったが、端末版は
/// 一覧の `q` で終了の確認を出す（`start_quit_confirm`、keys.rs:2350）。
/// 押して何も起きないキーは、無いキーより悪い。
///
/// 訊くのは端末版と同じ。シェルで何か動いているかもしれない、というのが
/// 訊く理由なので、それを言う。
async function cmdQuit() {
    if (!await confirm(tr('Quit cian', 'cian を終了します'),
        term.on ? tr('anything running in the shell ends', 'シェルで動いているものは終わります') : '')) {
        say(tr('stopped', 'やめました'));
        return;
    }
    window.close();
}

async function cmdMkdir(spec) {
    // `-p a/b/c` makes the whole chain; without it, one directory here.
    const deep = /^-p\s+/.test(spec);
    const name = spec.replace(/^-p\s+/, '').trim();
    if (!name) { say(tr('no name given', '名前がありません'), true); return; }
    const r = await ask('create', { pane: state.focus, name, dir: true, deep });
    if (!r) return;
    state[state.focus] = r.pane ?? r;
    draw(state.focus);
    say(tr(`created ${name}`, `${name} を作りました`));
}

async function cmdTouch(name) {
    if (!name) { say(tr('no name given', '名前がありません'), true); return; }
    const r = await ask('create', { pane: state.focus, name, dir: false, touch: true });
    if (!r) return;
    state[state.focus] = r.pane ?? r;
    draw(state.focus);
    say(tr(`created ${name}`, `${name} を作りました`));
}

async function cmdPwd() {
    const cwd = state[state.focus].cwd;
    await navigator.clipboard.writeText(cwd);
    say(tr(`${cwd} — on the clipboard`, `${cwd} — クリップボードへ`));
}

async function cmdLs(arg) {
    if (/-a/.test(arg || '')) { await toggleHidden(); return; }
    await reread();
}

/// Where the grep results are, so the viewer can walk them.
///
/// Kept after the report closes: opening a hit and then stepping to the next
/// is the whole point of a grep, and it cannot be done from a screen that had
/// to be closed to open the file.
const hits = { list: [], at: -1, needle: '' };

/// `Ctrl+N` / `Ctrl+Shift+N` — the next or previous grep hit, opened and
/// scrolled to its line.
async function hopHit(step) {
    if (!hits.list.length) { say(tr('no grep results', 'grep の結果がありません'), true); return; }
    hits.at = (hits.at + step + hits.list.length) % hits.list.length;
    const h = hits.list[hits.at];
    if (!await landOn(h.path)) return;
    if (viewer.on) await closeView(false);
    await lookInside();
    if (viewer.ed && h.line) {
        viewer.ed.setPosition({ lineNumber: h.line, column: 1 });
        viewer.ed.revealLineInCenter(h.line);
    }
    say(`${hits.needle}   ${hits.at + 1} / ${hits.list.length}   ${h.path.split(/[\\/]/).pop()}`);
}

/// `:queue` — what is running, and a way to stop one without stopping the
/// rest. A file manager copying ten thousand files should be able to say which
/// ten thousand.
async function cmdQueue() {
    const r = await ask('queue', {});
    if (!r) return;
    if (!r.jobs.length) { say(tr('nothing is running', '動いている操作はありません')); return; }
    const verb = { copy: tr('copy', 'コピー'), move: tr('move', '移動'), delete: tr('delete', '削除') };
    const waiting = r.jobs.filter((j) => j.state === 'waiting').length;
    show(tr('Operation queue', '操作キュー'), waiting ? tr(`1 running, ${waiting} waiting`, `実行中 1 件、待ち ${waiting} 件`) : tr('1 running', '実行中 1 件'),
        r.jobs.map((j) => ({
            // The runner is marked, because "what is happening now" and "what
            // is about to" are different questions and the list answers both.
            n: j.state === 'running' ? '▶' : `#${j.op}`,
            label: tr(`${verb[j.kind] || j.kind}  ${j.total}`, `${verb[j.kind] || j.kind}  ${j.total} 件`),
            sub: j.stopping ? tr('stopping…', '止めています…')
                : j.state === 'waiting' ? tr(`waiting   ${j.dest || ''}`, `待機中   ${j.dest || ''}`) : (j.dest || ''),
            op: j.op,
        })), {
            foot: tr('x stop (or cancel, if it is waiting)   b leave it running   Esc close', 'x 中止（待機中なら取り消し）   b 動かしたまま閉じる   Esc 閉じる'),
            act: {
                x: async () => {
                    const row = report.rows[report.at];
                    if (!row) return;
                    await ask('cancel', { op: row.op });
                    say(tr(`stopping #${row.op}`, `#${row.op} を中止しています`));
                    closeReport();
                },
                // `b` puts it out of the way and leaves it running — the
                // terminal build's word for it. Nothing is cancelled; the
                // screen is. The bar goes with it, for the same reason.
                b: () => {
                    prog.hidden = true;
                    drawProg();
                    closeReport();
                    say(tr('it is still running (:queue comes back to it)', '操作は動いたままです（:queue で戻れます）'));
                },
            },
        });
}

/// `@` — the macros in `macro.lua`.
///
/// **A layout macro builds a grid of shell panes in the terminal build, and
/// there are no splits here yet — so each pane becomes a tab.** The shells,
/// their commands and their scripted steps all run; only the arrangement is
/// lost. Said out loud in the list rather than discovered.
async function cmdMacros() {
    const r = await ask('macros', {});
    if (!r) return;
    if (!r.macros.length) {
        say(tr(`no macros (${r.where || 'macro.lua'})`, `マクロがありません（${r.where || 'macro.lua'}）`));
        return;
    }
    show(tr('Macros', 'マクロ'), r.where || '', r.macros.map((m) => ({
        n: m.script ? tr('script', 'スクリプト') : tr(`${m.panes} panes`, `${m.panes}枚`),
        label: m.name,
        sub: m.script
            ? tr('Lua — it moves files and says what it did', 'Lua ── ファイルを操作して、結果を言います')
            : tr('opened as tabs', 'タブとして開きます'),
        name: m.name,
        script: m.script,
    })), {
        foot: tr('Enter run   Esc close', 'Enter 実行   Esc 閉じる'),
        pick: async (row) => {
            closeReport();
            if (!term.on) await openShell();
            const done = await ask('macrorun', {
                pane: state.focus, name: row.name, ...shellSize(),
            });
            if (!done) return;
            // Two kinds behind one name. A layout macro hands back shell
            // panels; a script macro has already run and hands back what it
            // did — and it used to be refused outright, so half of anybody's
            // `macro.lua` worked and half said "not yet".
            if (done.script) {
                state.left = done.left;
                state.right = done.right;
                draw('left');
                draw('right');
                const lines = done.messages || [];
                if (done.error) {
                    // The error *and* whatever it managed to say first: a
                    // macro that stopped halfway has still done what it did.
                    show(tr(`${row.name} — stopped`, `${row.name} ── 途中で止まりました`),
                        done.error, lines.map((t) => ({ label: t })),
                        { foot: tr('Esc close', 'Esc 閉じる') });
                } else if (lines.length > 1) {
                    show(row.name, tr('what it did', '結果'),
                        lines.map((t) => ({ label: t })),
                        { foot: tr('Esc close', 'Esc 閉じる') });
                } else {
                    say(lines[0] || tr(`macro done: ${row.name}`, `マクロ完了: ${row.name}`));
                }
                return;
            }
            takeShell(done);
            setShellFocus(true);
            say(tr(`${done.name} — ${done.opened} panes opened as tabs`, `${done.name} — ${done.opened} 枚をタブで開きました`));
        },
    });
}

// ---- Tabs ----
//
// One list per side, and the active tab *is* that pane. A tab opens where you
// are standing, which is what makes it useful: the reason to open one is
// nearly always "keep this, and go somewhere else for a moment".

async function tabNew() {
    const which = state.focus;
    // Asked, as cian-tui asks (`ask_new_tab`, actions.rs:2793): a tab opens
    // silently, looks like the pane it came from, and nobody notices one has
    // appeared until there are several. One keystroke, and the tab is
    // something that was decided rather than something that happened.
    if (!await confirm(tr('New tab', '新しいタブ'), tr('Open another tab in this pane?', 'このペインにタブをもう一つ開きますか？'))) return;
    const pane = await ask('tabnew', { pane: which });
    if (!pane) return;
    state[which] = pane;
    draw(which);
    say(tr(`tab ${pane.tab + 1} / ${pane.tabs.length}`, `タブ ${pane.tab + 1} / ${pane.tabs.length}`));
}

async function tabClose(ask_first = false) {
    const which = state.focus;
    if (ask_first && !await confirm(tr('Close this tab', 'このタブを閉じます'), state[which].cwd)) {
        say(tr('stopped', 'やめました'));
        return;
    }
    const pane = await ask('tabclose', { pane: which });
    if (!pane) return;
    state[which] = pane;
    draw(which);
    say(pane.cwd);
}

async function goTab(which, how) {
    const pane = await ask('tabgo', { pane: which, ...how });
    if (!pane) return;
    state[which] = pane;
    state.focus = which;
    draw('left');
    draw('right');
    say(pane.cwd);
}

/// `s` — the folders worth going back to.
///
/// The terminal build's own `shortcuts.lua`, read and written through the same
/// renderer. A second bookmark list would be the worst kind of two-programs
/// problem: which folders you had saved would depend on which one you saved
/// them from.
/// `s` — the bookmarks, and the keys that change them.
///
/// cian-tui edits them from this very popup (`a` add, `A` add a folder, `d`
/// delete, `r` rename, `p` copy the target) and the window could only read
/// the list and jump. Same letters, same file — `shortcuts.lua`, which both
/// builds read, so a bookmark made in one is there in the other.
/// Which folder of the bookmarks is open, as a list of row names.
///
/// cian-tui shows one level at a time and walks into a folder on Enter
/// (keys.rs, over `sc_level(entries, path)`). The window laid the whole tree
/// out flat and put two spaces of indent in front of each name to suggest the
/// nesting — so a folder was permanently open, and the path in the second
/// column inherited the indent and sat at a margin of its own.
let scPath = [];

/// `s` — the bookmarks.
///
/// `keepLevel` is for the calls this function makes to itself: walking into a
/// group, and re-reading the list after an edit, both want the level they are
/// already standing in. **Opening it from the key does not.** `scPath` is
/// module state, so making a group, walking into it and closing the popup
/// left the path set — and the next `s` opened *inside that group*, with a
/// 戻る row as the only clue that there was anything above it. A bookmark list
/// that does not open at the top is a bookmark list you have to navigate out
/// of before you can use it.
async function cmdShortcuts(keepLevel = false) {
    if (!keepLevel) scPath = [];
    const r = await ask('shortcuts', {});
    if (!r) return;
    // The engine hands back a depth-first walk with a depth on each row.
    // Rebuild the shape it walked, so a level can be asked for.
    const roots = [];
    const stack = [{ kids: roots, depth: -1 }];
    for (const x of r.rows) {
        while (stack.length > 1 && stack[stack.length - 1].depth >= x.depth) stack.pop();
        const node = { ...x, kids: x.group ? [] : null };
        stack[stack.length - 1].kids.push(node);
        if (x.group) stack.push({ kids: node.kids, depth: x.depth });
    }
    // Walk down to whatever was open. A folder that has since been renamed or
    // deleted drops the path rather than showing an empty level with no way
    // back — the list is re-read after every edit, so this happens.
    let here = roots;
    const open = [];
    for (const name of scPath) {
        const found = here.find((n) => n.group && n.name === name);
        if (!found) break;
        open.push(name);
        here = found.kids;
    }
    scPath = open;

    const rows = here.map((x) => ({
        n: x.group ? '▸' : '',
        label: x.name,
        sub: x.group ? tr(`${x.kids.length} items`, `${x.kids.length} 件`) : (x.target || ''),
        target: x.target,
        at: x.at,
        group: x.group,
        plain: x.name,
    }));
    if (scPath.length) {
        rows.unshift({ n: '◂', label: tr('back', '戻る'), sub: scPath.join(' / '), up: true, plain: '' });
    }
    // Even with nothing in it: `a` has to have somewhere to be pressed.
    const edit = async (params, done) => {
        const res = await ask('shortcutedit', params);
        if (!res) return;
        say(res.said);
        if (done) done();
        closeReport();
        cmdShortcuts(true);
    };
    show(tr('Shortcuts', 'ショートカット') + (scPath.length ? ` / ${scPath.join(' / ')}` : ''),
        rows.length ? (r.where || '') : tr('nothing bookmarked yet', '登録がありません'), rows, {
        // The paths line up under each other; a column you read down has to
        // start in the same place on every row.
        align: true,
        foot: tr('Enter open / go   a add   A group   r rename   d delete   p path   Esc close', 'Enter 開く／そこへ   a 追加   A まとめる   r 名前   d 削除   p パス   Esc 閉じる'),
        // Enter on a folder walks into it, as the terminal build's does; on a
        // bookmark it goes there. Nothing else opens a level, so a folder that
        // is not entered stays shut.
        pick: (row) => {
            if (row.up) { scPath.pop(); closeReport(); cmdShortcuts(true); return; }
            if (row.group) { scPath.push(row.plain); closeReport(); cmdShortcuts(true); return; }
            if (row.target) { closeReport(); revealPath(row.target, true); }
        },
        act: {
            a: async () => {
                const name = await askFor(tr('a name for this place', 'ここを登録する名前'), state[state.focus].cwd.split(/[\\/]/).pop() || '');
                if (name === null) return;
                const made = await ask('bookmark', { pane: state.focus, name: name.trim() });
                if (!made) return;
                say(tr(`bookmarked ${made.name}`, `${made.name} を登録しました`));
                closeReport();
                cmdShortcuts(true);
            },
            A: async () => {
                const name = await askFor(tr('a name for the group', 'まとめの名前'), '');
                if (name === null || !name.trim()) return;
                await edit({ do: 'group', value: name.trim() });
            },
            r: async () => {
                const row = report.rows[report.at];
                if (!row) return;
                const name = await askFor(tr('a new name', '新しい名前'), row.plain);
                if (name === null || !name.trim()) return;
                await edit({ do: 'rename', at: row.at, value: name.trim() });
            },
            d: async () => {
                const row = report.rows[report.at];
                if (!row) return;
                // Asked, like everything else that loses something. A folder
                // takes its contents with it, and that is worth saying.
                const ok = await confirm(
                    row.group ? tr(`delete ${row.plain} and everything in it`, `${row.plain} を中身ごと削除`) : tr(`delete ${row.plain}`, `${row.plain} を削除`),
                    row.target || '',
                );
                if (!ok) return;
                await edit({ do: 'delete', at: row.at });
            },
            p: async () => {
                const row = report.rows[report.at];
                if (!row || !row.target) return;
                await navigator.clipboard.writeText(row.target);
                say(tr(`${row.target} copied`, `${row.target} をコピー`));
            },
        },
    });
}

async function cmdBookmark(name) {
    const r = await ask('bookmark', { pane: state.focus, name });
    if (!r) return;
    say(tr(`bookmarked ${r.name}`, `${r.name} を登録しました`));
}

// ---- The AI, where a site has configured one ----
//
// The prompts live in the engine, word for word the terminal build's. Two
// front ends asking the same model differently would give two different
// answers to the same question, which is the kind of difference nobody can
// debug.

/// `:aicmd` — a description in, a command out, into the shell's prompt but
/// **not run**. A model that guesses wrong is a model that guesses wrong; the
/// person presses Enter, not the program.
/// What to do with the answer when it arrives. The engine runs the model on a
/// worker — it waits on a python process talking to somebody else's network —
/// so this is a question asked and an answer heard, not a call.
let aiWaiting = null;

async function cmdAiCmd(want) {
    const r = await ask('ai', { pane: state.focus, what: 'cmd', text: want });
    if (!r) return;
    say(tr('thinking…', '考えています…'));
    aiWaiting = async (answer) => {
        // Shown before it is put anywhere. It was typed straight into the
        // shell prompt — never run, which is the important half — but
        // cian-tui asks first (AiShellConfirm), and a command you did not ask
        // to see sitting on your prompt is a command you might Enter on
        // reflex.
        const line = answer.trim().split('\n')[0].replace(/^[$#>]\s*/, '');
        if (!line) { say(tr('the answer came back empty', '返事が空でした'), true); return; }
        const ok = await confirm(tr('Put this command at the prompt?', 'このコマンドをプロンプトに置きますか'),
            tr(`${line}\n\nit is only placed; nothing is run`, `${line}\n\n置くだけで実行はしません`));
        if (!ok) { say(tr('stopped', 'やめました')); return; }
        if (!term.on) await openShell();
        await ask('shellinput', { text: line });
        setShellFocus(true);
        say(tr('Enter runs it, Ctrl+C throws it away — nothing has run', 'Enter で実行、Ctrl+C で捨てる — 実行はしていません'));
    };
}

async function cmdAiLog() {
    const pane = state[state.focus];
    const name = pane.entries[pane.cursor]?.name || '';
    const r = await ask('ai', { pane: state.focus, what: 'log' });
    if (!r) return;
    openChat(tr('Triage this log', 'このログを診断'), tr(`Triage the log: ${name}`, `ログを診断: ${name}`));
    answerIntoChat();
}

async function cmdAiAsk(question) {
    openChat(tr('Chat', 'チャット'));
    if (question && question.trim()) await askChat(question.trim());
}

/// The three things a file open in front of you is usually wanted for —
/// cian-tui's `AiOverText` (ai.rs:476), prompt for prompt.
///
/// The window had one row here that pre-filled the command line with
/// `ai この選択を直して` and two that were a plain chat wearing a different
/// name. Same words on the menu, three different actions behind them.
/// A function, not a constant: the second half of each pair is a *title*, and
/// a title is words on a screen. See `sorts()` for what a `const` does to one.
function aiOverText() {
    return {
    writing: [
        'You are an editor. Improve the passage below: fix grammar and '
        + "typos, tighten wording, and keep the author's voice and "
        + 'language (answer in the language it is written in). Give the '
        + 'rewritten text first, then a short list of what you changed '
        + 'and why. Do not invent facts.',
        tr('Improve this writing', 'この文章を推敲'),
    ],
    command: [
        'You help an operator on RHEL/AIX. If the text below is a shell '
        + 'command, explain what it does, flag anything destructive, and '
        + 'suggest a safer or shorter form. If it is a description of a '
        + 'task, write the command that does it and explain each part. '
        + 'Plain text, no markdown headings.',
        tr('Explain / write this command', 'コマンドを説明・作成'),
    ],
    code: [
        'You review code. For the excerpt below: point out bugs, '
        + 'error handling that is missing, and anything that will not do '
        + 'what it looks like it does — most important first. Then give '
        + 'the corrected code. Say so plainly if you find nothing wrong.',
        tr('Review and fix this code', 'このコードを点検・修正'),
    ],
    };
}

/// `:mermaid` — the open file's diagrams, drawn here.
///
/// **cian-tui opens a browser because a terminal cannot draw a diagram.** That
/// is a workaround for a limitation this build does not have, and porting it
/// unchanged was porting the limitation: the window has been rendering these
/// inline in the Markdown preview all along. So `:mermaid` now shows the
/// diagrams — all of them, full width, nothing else — in the reading panel.
///
/// The browser is still worth having and is still one row away
/// (`:mermaid!` / `ブラウザで開く`): it is how a diagram gets printed, zoomed
/// past the window's width, or handed to somebody who does not have cian.
async function cmdMermaid() {
    if (!viewer.on || !viewer.ed) {
        say(tr('open a file first (F3)', '先にファイルを開いてください（F3）'), true);
        return;
    }
    // The engine's extractor, not a second one written here.
    const r = await ask('mermaid', { text: viewer.ed.getModel().getValue(), open: false });
    if (!r) return;
    const frag = document.createElement('div');
    for (const src of r.blocks) {
        const pre = document.createElement('pre');
        const code = document.createElement('code');
        code.className = 'language-mermaid';
        code.textContent = src;
        pre.append(code);
        frag.append(pre);
    }
    el.vRead.replaceChildren(frag);
    el.vRead.hidden = false;
    el.vBody.hidden = true;
    reading = true;
    await drawDiagrams();
    say(tr(`${r.blocks.length} mermaid — Ctrl+E or Esc for the source, :mermaid! for a browser`, `mermaid ${r.blocks.length} 件 — Ctrl+E か Esc でソースへ、:mermaid! でブラウザ`));
}

/// `:mermaid!` — the same diagrams, in a browser.
///
/// For printing, for a diagram wider than the window, and for handing to
/// somebody without cian. Offline when `mermaid.min.js` sits beside the
/// config, exactly as cian-tui does it.
async function cmdMermaidOut() {
    if (!viewer.on || !viewer.ed) {
        say(tr('open a file first (F3)', '先にファイルを開いてください（F3）'), true);
        return;
    }
    const r = await ask('mermaid', { text: viewer.ed.getModel().getValue() });
    if (!r) return;
    say(tr(`opened ${r.blocks} mermaid block(s) in the browser (${r.offline ? 'offline' : 'via CDN'})`, `mermaid を ${r.blocks} 件ブラウザで開きました（${r.offline ? 'offline' : 'via CDN'}）`));
}

/// `:summary` — what this file is, for someone about to work on it.
///
/// cian-tui's `summarize_viewer` (ai.rs:303), prompt and bound alike. Unlike
/// the scans, this sends the file's **text** to the model, which is why it is
/// an explicit row rather than something the viewer does on opening.
async function cmdSummary() {
    if (!viewer.on || !viewer.ed) {
        say(tr('open a file first (F3)', '先にファイルを開いてください（F3）'), true);
        return;
    }
    const body = viewer.ed.getModel().getValue().slice(0, 24000);
    if (!body.trim()) { say(tr('nothing to summarise', '要約する対象がありません'), true); return; }
    const r = await ask('ai', {
        pane: state.focus, what: 'text',
        system: 'You summarise a file\'s contents for a developer. Give a '
            + 'short, plain-text summary: what it is, its purpose, and the key '
            + 'points or structure. Be concise; no preamble, no markdown headings.',
        text: body,
    });
    if (!r) return;
    openChat(
        tr('Summarise this file', 'このファイルを要約'),
        tr(`Summarise ${viewer.name}`, `${viewer.name} を要約`),
        body,
    );
    answerIntoChat();
}

async function cmdAiOverText(kind) {
    if (!viewer.on || !viewer.ed) {
        say(tr('open a file first (F3)', '先にファイルを開いてください（F3）'), true);
        return;
    }
    const [system, title] = aiOverText()[kind];
    // The selection when there is one, the file when there is not — which is
    // what cian-tui sends (`selected_text().unwrap_or(whole)`). A model has a
    // limit and a file may be larger than it; the head is the part that says
    // what the thing is.
    const model = viewer.ed.getModel();
    const sel = viewer.ed.getSelection();
    const text = (sel && !sel.isEmpty() ? model.getValueInRange(sel) : model.getValue()).slice(0, 16000);
    if (!text.trim()) {
        say(tr('there is nothing in it', '中身がありません'), true);
        return;
    }
    const r = await ask('ai', { pane: state.focus, what: 'text', system, text });
    if (!r) return;
    openChat(title, `${title} — ${viewer.name}`, text);
    answerIntoChat();
}

/// `:aidiff` — explain what is on the comparison screen.
///
/// Only from a comparison, because "explain the diff" with no diff up is a
/// question about nothing.
async function cmdAiDiff() {
    if (!report.on || !report.rows.length) {
        say(tr('compare something first, with =', '先に = で比較してください'), true);
        return;
    }
    const text = report.rows.map((x) => `${x.n || ''} ${x.label} ${x.sub || ''}`).join('\n');
    const r = await ask('ai', {
        pane: state.focus, what: 'text',
        system: 'You explain a diff to the person who is about to act on it. '
            + 'Say what changed and, where it is clear, why it matters. '
            + 'Be concise; plain text, no markdown headings.',
        text: text.slice(0, 16000),
    });
    if (!r) return;
    openChat(
        tr('The diff, explained', '差分の説明'),
        tr('Explain this diff', 'この差分を説明'),
        text.slice(0, 16000),
    );
    answerIntoChat();
}

async function cmdOffice(what) {
    const r = await ask(what, { pane: state.focus });
    if (!r) return;
    if (r.made) {
        state[state.focus] = r;
        draw(state.focus);
        say(tr(`created ${r.made}`, `${r.made} を作りました`));
        return;
    }
    say(tr(`opened ${r.opened} in the cloud`, `${r.opened} をクラウドで開きました`));
}

async function cmdReload() {
    const r = await ask('reload', {});
    if (!r) return;
    // Said plainly rather than "reloaded": some of init.lua is read once, at
    // startup, and claiming otherwise sends people looking for a bug.
    say(tr(`init.lua re-read (AI ${r.ai ? 'yes' : 'no'}, `, `init.lua を読み直しました（AI ${r.ai ? 'あり' : 'なし'}、`)
        + tr(`${r.sync_maps} sync maps, ${r.ssh_hosts} hosts) — borders and the like need a restart`, `同期 ${r.sync_maps} 件、SSH ${r.ssh_hosts} 件）— 枠線などは再起動が要ります`));
}

/// `:key` — show what the window actually received.
///
/// The first thing to ask when a key "does nothing": did it arrive, and as
/// what? On this build it also answers whether a menu accelerator ate it,
/// which is a Windows-only failure invisible from a Mac.
const keyEcho = { on: false };

function toggleKeyEcho() {
    keyEcho.on = !keyEcho.on;
    say(keyEcho.on
        ? tr('key echo: each key is shown and nothing is run (Esc stops)', 'キー表示: 押したキーを出すだけで、何も実行しません（Esc で止める）')
        : tr('key echo stopped', 'キー表示を止めました'));
}

// ---- A server, in this pane ----
//
// Not a transfer dialog. The rows are rows, `Enter` walks into a directory,
// `..` climbs, and `c` across to the other pane is an upload or a download
// depending on which side you are standing on. That is the terminal build's
// arrangement, and the reason it is worth having at all: nothing new to learn.

/// `Shift+S` — the hosts init.lua declares, picked rather than typed.
///
/// Whether a password is stored comes over as a yes or a no; the password
/// itself never leaves the engine, which resolves it (or runs password_cmd)
/// at connect time.
/// The hosts from `init.lua`'s `cian.ssh`, as picker rows. Two callers wanted
/// the same list in the same shape and had a copy each.
function sshRows(hosts) {
    const rows = [];
    for (const h of hosts) {
        for (const u of h.users) {
            rows.push({
                n: u.stored ? tr('key', '鍵あり') : '',
                label: `${u.name}@${h.name}`,
                sub: `${h.host}:${h.port}`,
                host: h.at,
                user: u.at,
                stored: u.stored,
                who: `${u.name}@${h.host}`,
            });
        }
    }
    return rows;
}

/// `:ssh` / Shift+S — **a shell on the far machine**, in the shell panel.
///
/// This used to open an SFTP listing in a pane, which is what `:sftp` is for.
/// The terminal build has always run `ssh user@host` in its shell here
/// (`App::ssh_connect`), so the same word did two different things depending
/// on which build you were in — and the window's answer was the one nobody
/// asked for: you press SSH接続 to get a prompt, not a file list.
async function cmdSshPicker() {
    const r = await ask('sshhosts', {});
    if (!r) return;
    if (!r.hosts.length) {
        say(tr('no hosts in init.lua’s cian.ssh', 'init.lua の cian.ssh にサーバがありません'), true);
        return;
    }
    const rows = sshRows(r.hosts);
    show('SSH', tr(`${rows.length} hosts (init.lua’s cian.ssh) — opens a shell`, `${rows.length} 件（init.lua の cian.ssh）── シェルで開きます`), rows, {
        filter: true,
        hint: tr('type to narrow (host or user)', '打って絞り込み（ホスト名・ユーザー）'),
        foot: tr('type to narrow   Enter connect   Esc close', '打って絞る   Enter 接続   Esc 閉じる'),
        pick: async (row) => {
            closeReport();
            if (!term.on) await openShell();
            setShellFocus(true);
            const c = await ask('sshshell', { pane: state.focus, host: row.host, user: row.user, ...shellSize() });
            if (!c) return;
            takeShell(c);
            // Said, because a password is about to be typed by the program
            // rather than by the person, and that should never be a surprise.
            say(c.keyed
                ? tr(`→ ${c.who}`, `→ ${c.who}`)
                : tr(`→ ${c.who} (the password goes in when it is asked for)`, `→ ${c.who}（訊かれたらパスワードを送ります）`));
        },
    });
}

/// `:sftp` / `:remote` — **a server's files, in this pane.**
///
/// Offers what `init.lua` holds first. It went straight to a `user@host` box,
/// so a machine with five hosts configured made you type one of them out —
/// "init.lua でセットしているサーバの設定が有効にならなかった", which was
/// exactly right: the configuration was never consulted. F2 still takes one
/// by hand, for a host that is not in the file.
async function cmdSftpPicker() {
    const r = await ask('sshhosts', {});
    if (!r) return;
    if (!r.hosts.length) { await cmdConnect(); return; }
    const rows = sshRows(r.hosts);
    show('SFTP', tr(`${rows.length} hosts (init.lua’s cian.ssh)`, `${rows.length} 件（init.lua の cian.ssh）`), rows, {
        // A host list is long the moment there is more than a handful, and
        // cian-tui narrows it as you type.
        filter: true,
        hint: tr('type to narrow (host or user)', '打って絞り込み（ホスト名・ユーザー）'),
        foot: tr('type to narrow   Enter connect   F2 type one   Esc close', '打って絞る   Enter 接続   F2 手で入力   Esc 閉じる'),
        act: { F2: () => { closeReport(); cmdConnect(); } },
        pick: async (row) => {
            closeReport();
            let password;
            if (!row.stored) {
                password = await askFor(tr(`password for ${row.who}`, `${row.who} のパスワード`), '', { secret: true });
                if (password === null) return;
            }
            say(tr(`connecting to ${row.who}…`, `${row.who} に繋いでいます…`));
            const c = await ask('connect', {
                pane: state.focus, preset_host: row.host, preset_user: row.user, password,
            });
            if (!c) return;
            state[state.focus] = c.pane;
            draw(state.focus);
            say(`${c.host}  ${c.path}`);
        },
    });
}

/// `転送 ▸` — send the marked files to a configured server, or fetch from one.
///
/// cian-tui's `SendMenu` picks a host and then opens a *modal* remote browser
/// to choose the far end. The window already browses a server in a pane, which
/// is the same journey in this build's own idiom and better: the far side is a
/// listing you can navigate, mark in and sort, not a dialog. So this connects
/// the **opposite** pane to the host and hands you back to `c`, which is the
/// key that already means "across" in both builds.
async function cmdSend(dir) {
    const here = state[state.focus];
    if (dir === 'up') {
        const rows = (here.entries || []).filter((r) => r.marked && !r.parent);
        const one = here.entries[here.cursor];
        if (!rows.length && (!one || one.parent)) {
            say(tr('choose a file to upload', 'アップロードするファイルを選んでください'), true);
            return;
        }
    }
    const other = state.focus === 'left' ? 'right' : 'left';
    const r = await ask('sshhosts', {});
    if (!r) return;
    if (!r.hosts.length) {
        say(tr('no hosts in init.lua’s cian.ssh — :sftp takes one by hand', 'init.lua の cian.ssh にサーバがありません — :sftp で手入力できます'), true);
        return;
    }
    const rows = sshRows(r.hosts);
    show(dir === 'up' ? tr('Upload → server', 'アップロード → サーバ') : tr('Download ← server', 'ダウンロード ← サーバ'),
        tr(`${rows.length} hosts (init.lua’s cian.ssh) — opening in the ${other === 'left' ? 'left' : 'right'} pane`, `${rows.length} 件（init.lua の cian.ssh）— ${other === 'left' ? '左' : '右'}のペインに開きます`),
        rows, {
            filter: true,
            hint: tr('type to narrow (host or user)', '打って絞り込み（ホスト名・ユーザー）'),
            foot: tr('type to narrow   Enter connect   Esc close', '打って絞る   Enter 接続   Esc 閉じる'),
            pick: async (row) => {
                closeReport();
                let password;
                if (!row.stored) {
                    password = await askFor(tr(`password for ${row.who}`, `${row.who} のパスワード`), '', { secret: true });
                    if (password === null) return;
                }
                say(tr(`connecting to ${row.who}…`, `${row.who} に繋いでいます…`));
                const c = await ask('connect', {
                    pane: other, preset_host: row.host, preset_user: row.user, password,
                });
                if (!c) return;
                state[other] = c.pane;
                draw(other);
                // Back to the key that already means "across". The far side is
                // a listing now, so the folder is chosen by walking to it.
                say(dir === 'up'
                    ? tr(`${c.host}  ${c.path} — walk to where it goes, then c`, `${c.host}  ${c.path} — 送り先まで移動して c`)
                    : tr(`${c.host}  ${c.path} — mark what to fetch and press c there`, `${c.host}  ${c.path} — 取ってくるものをマークして、そちらで c`));
            },
        });
}

async function cmdConnect() {
    const spec = await askFor('user@host[:port][:/path]', '');
    if (spec === null || !spec.trim()) return;
    const m = spec.trim().match(/^([^@]+)@([^:/]+)(?::(\d+))?(?::?(\/.*))?$/);
    if (!m) { say(tr('write it as user@host', 'user@host の形で書いてください'), true); return; }
    const [, user, host, port, path] = m;
    // Asked for, never stored. cian has nowhere to keep a password that would
    // be better than not keeping one.
    const password = await askFor(tr(`password for ${user}@${host}`, `${user}@${host} のパスワード`), '', { secret: true });
    if (password === null) return;
    say(tr(`connecting to ${user}@${host}…`, `${user}@${host} に繋いでいます…`));
    const r = await ask('connect', {
        pane: state.focus, user, host,
        port: port ? Number(port) : 22,
        path: path || '.',
        password,
    });
    if (!r) return;
    state[state.focus] = r.pane;
    draw(state.focus);
    say(`${r.host}  ${r.path}`);
}

async function cmdDisconnect() {
    const pane = await ask('disconnect', { pane: state.focus });
    if (!pane) return;
    state[state.focus] = pane;
    draw(state.focus);
    say(pane.cwd);
}

/// The same keys, over the network.
///
/// `a`, `A`, `r`, `d` behave as they do locally — the rows look the same, so
/// they should act the same. The one difference is said out loud rather than
/// discovered: a remote delete is a delete, because SFTP has no trash.
async function remoteOp(what) {
    const which = state.focus;
    const pane = state[which];
    let name;
    if (what === 'mkdir' || what === 'touch') {
        name = await askFor(what === 'mkdir' ? tr('name for the new folder', '新しいディレクトリの名前') : tr('name for the new file', '新しいファイルの名前'), '');
        if (name === null || !name) return;
    } else if (what === 'rename') {
        const row = pane.entries[pane.cursor];
        if (!row || row.parent) return;
        name = await askFor(tr(`a new name for ${row.name}`, `${row.name} の新しい名前`), row.name);
        if (name === null || !name) return;
    } else if (what === 'delete') {
        const marked = pane.entries.filter((x) => x.marked);
        const rows = marked.length ? marked : [pane.entries[pane.cursor]].filter((x) => x && !x.parent);
        if (!rows.length) { say(tr('nothing to work on', '対象がありません'), true); return; }
        if (!await confirm(
            tr(`Delete ${rows.length} from the server`, `${rows.length} 件をサーバから削除します`),
            tr('there is no trash there — this cannot be undone\n\n', 'ゴミ箱はありません — 元に戻せません\n\n') + rows.map((x) => x.name).join('\n'),
        )) { say(tr('stopped', 'やめました')); return; }
    }
    const r = await ask('remoteop', { pane: which, what, name });
    if (!r) return;
    state[which] = r;
    draw(which);
    say(r.said);
}

async function remoteStep(opts) {
    const which = state.focus;
    const r = await ask('remotelist', { pane: which, ...opts });
    if (!r) return;
    state[which] = r.pane;
    draw(which);
    say(r.path);
}

async function uploadHeld() {
    const which = state.focus;
    say(tr('uploading…', 'アップロード中…'));
    const r = await ask('uploadclip', { pane: which });
    if (!r) return;
    state[which] = r;
    draw(which);
    if (r.errors.length) say(r.errors.join('  /  '), true);
    else say(tr(`uploaded ${r.ok}`, `${r.ok} 件をアップロードしました`));
}

async function transfer() {
    const which = state.focus;
    const other = which === 'left' ? 'right' : 'left';
    const pane = state[which];
    const rows = pane.entries.filter((x) => x.marked);
    const what = rows.length ? rows : [pane.entries[pane.cursor]].filter((x) => x && !x.parent);
    if (!needTargets(what)) return;
    const up = !!state[other].remote;
    const relay = up && !!state[which].remote;
    const head = relay
        ? tr(`Send ${what.length} to the other server`, `${what.length} 件を反対のサーバへ`)
        : tr(`${up ? 'Upload' : 'Download'} ${what.length}`, `${what.length} 件を ${up ? 'アップロード' : 'ダウンロード'}`);
    // Named for what it does: there is no server-to-server SFTP, so the file
    // comes here and goes out again, and a person watching the bytes twice
    // should know why.
    const body = what.map((x) => x.name).join('\n')
        + (relay ? tr('\n\nthey pass through this machine on the way', '\n\nこの機械を経由します') : '');
    if (!await confirm(head, body)) { say(tr('stopped', 'やめました')); return; }
    say(tr(`${head}…`, `${head}中…`));
    // The bar, before the transfer rather than after it. SFTP does not go
    // through the job queue, so there is no op id to key on yet — the engine's
    // first progress event carries it and the bar adopts it. Until this, a
    // transfer of a 400MB file said "uploading…" once and then nothing, while
    // cian-scp had been counting the bytes all along with nobody listening.
    running = {
        op: null, kind: 'transfer', verb: head,
        total: what.length, done: 0, bytes: 0, bytesTotal: 0, ms: 0, path: '',
    };
    prog.hidden = false;
    prog.stalledAt = performance.now();
    drawProg();
    const r = await ask('transfer', { pane: which });
    running = null;
    prog.hidden = true;
    if (!r) return;
    state.left = r.left;
    state.right = r.right;
    draw('left');
    draw('right');
    if (r.errors.length) say(r.errors.join('  /  '), true);
    else if (r.direction === 'relay') {
        say(tr(`sent ${r.ok} to the other server`, `${r.ok} 件を反対のサーバへ送りました`));
    } else {
        say(tr(`${r.direction === 'up' ? 'uploaded' : 'downloaded'} ${r.ok}`, `${r.ok} 件を${r.direction === 'up' ? 'アップロード' : 'ダウンロード'}しました`));
    }
}

/// `:!cmd` — run it in the shell, in this pane's directory. `%` is the
/// selection, `%f` the file, `%d` the directory; the engine substitutes them,
/// quoted, because a path with a space in it is the common case.
async function cmdBang(line) {
    if (!term.on) await openShell();
    const r = await ask('run', { pane: state.focus, line });
    if (!r) return;
    say(r.sent);
}

/// `:renamelist` — edit the names as a list, apply the list.
///
/// The other bulk rename. `:renamepattern` is for a rule; this is for the
/// hundred names that follow no rule, which is most of them. Editing them as
/// text is the only way that is not a hundred prompts, and it is what every
/// filer that has this feature does.
async function cmdRenameList() {
    const pane = state[state.focus];
    const rows = pane.entries.filter((x) => x.marked);
    const what = rows.length ? rows : pane.entries.filter((x) => !x.parent);
    if (!needTargets(what)) return;

    let monaco;
    try {
        monaco = await loadMonaco();
    } catch (e) { say(e.message, true); return; }

    // The editor, on a list rather than a file. Nothing is written until it is
    // closed, and the line count has to still match — a list one line short is
    // a rename that would pair the wrong names together, silently.
    renameList.on = true;
    renameList.paths = what.map((x) => x.path);
    setViewerOn(true);
    viewer.name = tr('the list of names', '名前の一覧');
    el.view.hidden = false;
    el.vBody.hidden = false;
    el.vPic.hidden = true;
    makeEditor(monaco, what.map((x) => x.name).join('\n'), 'plaintext');
    viewer.base = viewer.ed.getModel().getAlternativeVersionId();
    viewer.dirty = false;
    setStyle(style);
    el.vName.textContent = tr('Edit the list of names', '名前の一覧を編集');
    el.vAbout.textContent = tr(`${what.length} names   one per line, and keep the order`, `${what.length} 件   1行に1つ、順番は変えないこと`);
    el.vFoot.textContent = tr('Ctrl+S applies   Esc ×3 cancels', 'Ctrl+S 適用   Esc ×3 取消');
    viewer.ed.focus();
}

const renameList = { on: false, paths: [] };

async function applyRenameList() {
    const names = viewer.ed.getValue().split(/\r?\n/).map((s) => s.trim()).filter(Boolean);
    if (names.length !== renameList.paths.length) {
        say(tr(`the count does not match (${names.length} lines / ${renameList.paths.length} files)`, `行数が合いません（${names.length} 行 / ${renameList.paths.length} 件）`), true);
        return false;
    }
    const rows = renameList.paths
        .map((path, i) => ({ path, to: names[i] }))
        .filter((x, i) => x.to !== state[state.focus].entries.find((e) => e.path === x.path)?.name
            || names[i] !== names[i]);
    const changing = rows.filter((x) => x.to !== x.path.split(/[\\/]/).pop());
    if (!changing.length) { say(tr('no name would change', '変わる名前がありません')); return true; }
    if (!await confirm(tr(`Rename ${changing.length}`, `${changing.length} 件の名前を変えます`),
        changing.map((x) => `${x.path.split(/[\\/]/).pop()}  →  ${x.to}`).join('\n'))) {
        say(tr('stopped', 'やめました'));
        return false;
    }
    const done = await ask('renameapply', { rows: changing });
    if (!done) return false;
    await reread();
    if (done.errors.length) say(done.errors.join('  /  '), true);
    else say(tr(`renamed ${done.renamed}`, `${done.renamed} 件の名前を変えました`));
    return true;
}

/// The open file's headings, and a way to land on one.
///
/// Closing the list rather than keeping it beside the text: a file manager's
/// editor is a place you go to change one thing, and a permanent outline
/// column would be a third of the width spent on navigation you use twice.
async function cmdOutline() {
    if (!viewer.on) { say(tr('open a file first', '先にファイルを開いてください'), true); return; }
    const r = await ask('outline', {});
    if (!r) return;
    if (!r.items.length) { say(tr('no headings found', '見出しが見つかりません')); return; }
    show(tr(`headings in ${viewer.name}`, `${viewer.name} の見出し`), tr(`${r.items.length} items`, `${r.items.length} 件`),
        r.items.map((i) => ({
            n: String(i.line + 1),
            label: '  '.repeat(i.level) + i.text,
            line: i.line,
        })),
        {
            foot: tr('Enter go there   Esc close', 'Enter そこへ   Esc 閉じる'),
            pick: (row) => {
                closeReport();
                viewer.ed.revealLineInCenter(row.line + 1);
                viewer.ed.setPosition({ lineNumber: row.line + 1, column: 1 });
                viewer.ed.focus();
            },
        });
}

/// A line operation on whatever the editor is holding.
///
/// The lines go down to cian-core and come back changed. `:han` and `:zen`
/// alone are a table of Japanese width mappings, and nobody should own two
/// copies of that.
/// Hand the whole buffer to the engine and take back what it returns.
///
/// The line work — sort, uniq, the substitutions — belongs on the engine's
/// side, where cian-core already holds it and the terminal build already
/// calls it. What is left here is the same six lines every time, and putting
/// the answer back **through the editor's own edit stack rather than
/// setValue** is the part that matters: it has to be undoable with the key
/// that undoes everything else in here.
async function rewriteBuffer(method, params, said) {
    if (!needViewer()) return null;
    const lines = viewer.ed.getValue().split(/\r?\n/);
    const r = await ask(method, { ...params, lines });
    if (!r) return null;
    const model = viewer.ed.getModel();
    viewer.ed.executeEdits('cian', [{
        range: model.getFullModelRange(),
        text: r.lines.join('\n'),
    }]);
    viewer.ed.pushUndoStop();
    say(said(r, lines));
    return r;
}

async function textOp(op) {
    await rewriteBuffer('textop', { op },
        (r, lines) => tr(`:${op}   ${lines.length} → ${r.lines.length} lines`, `:${op}   ${lines.length} 行 → ${r.lines.length} 行`));
}

async function setEol(kind) {
    const r = await ask('eol', { kind });
    if (!r) return;
    say(tr(`line endings set to ${r.eol.toUpperCase()} (written on save)`, `改行を ${r.eol.toUpperCase()} にしました（保存時に反映）`));
}

async function cmdSvn(what) {
    let message;
    if (what === 'commit') {
        message = await askFor(tr('Commit message', 'コミットメッセージ'), '');
        if (message === null || !message.trim()) return;
    }
    const r = await ask('svn', { pane: state.focus, what, message });
    if (!r) return;
    state[state.focus] = r.pane;
    draw(state.focus);
    say(r.said);
}

async function cmdNoBom() {
    const pane = state[state.focus];
    const rows = pane.entries.filter((x) => x.marked);
    const what = rows.length ? rows : [pane.entries[pane.cursor]].filter((x) => x && !x.parent);
    if (!needTargets(what)) return;
    if (!await confirm(tr(`Strip UTF-8 BOMs from ${what.length}`, `${what.length} 件から UTF-8 BOM を除去します`),
        what.map((x) => x.name).join('\n'))) { say(tr('stopped', 'やめました')); return; }
    const r = await ask('nobom', { pane: state.focus });
    if (!r) return;
    state[state.focus] = r.pane;
    draw(state.focus);
    const parts = [tr(`${r.stripped} stripped`, `BOM除去 ${r.stripped} 件`)];
    if (r.none) parts.push(tr(`${r.none} had none`, `もともと無し ${r.none} 件`));
    if (r.utf16) parts.push(tr(`${r.utf16} UTF-16 left alone`, `UTF-16 は据置 ${r.utf16} 件`));
    if (r.failed) parts.push(tr(`${r.failed} failed`, `失敗 ${r.failed} 件`));
    say(parts.join('   '), r.failed > 0);
}

async function cmdEach(line) {
    if (!term.on) await openShell();
    const r = await ask('each', { pane: state.focus, line });
    if (!r) return;
    say(tr(`ran on ${r.ran}`, `${r.ran} 件に実行しました`));
}

function findCommand(name) {
    // Aliases carry the terminal build's other spellings (`:duplicate`,
    // `:dup`) without a second palette entry per spelling.
    return commands().find((c) => c.name === name || (c.alias || []).includes(name));
}

/// `:` — the name, then whatever it takes.
function commandLine(initial = '') {
    // On the prompt row at the foot, where cian-tui puts its command line —
    // not in a sheet in the middle of the window. Purple, because `/` above
    // it is green and the two take the same letters.
    openPrompt('cmd', initial);
}

/// Run whatever was typed on the command line.
async function runTypedCommand(line) {
    const text = line.trim();
    if (!text) return;
    // `!` is a prefix, not a name: everything after it is the command line
    // itself, spaces and all.
    if (text.startsWith('!')) {
        await cmdBang(text.slice(1).trim());
        return;
    }
    const at = text.indexOf(' ');
    const name = at < 0 ? text : text.slice(0, at);
    const arg = at < 0 ? '' : text.slice(at + 1).trim();
    const cmd = findCommand(name);
    if (cmd && cmd.name !== name) {
        // Called by an alias: the spelling used is information (`:icons` is
        // `:view icons`, `:nvim` names its editor), so it rides along.
        await runCommand(cmd, arg, name);
        return;
    }
    if (!cmd) {
        // Named, not "unknown command": the name typed is the one thing the
        // person can compare against the list.
        say(tr(`:${name}? — C lists every command`, `:${name} は知りません — C でコマンド一覧`), true);
        return;
    }
    await runCommand(cmd, arg);
}

async function runCommand(cmd, arg, invokedAs) {
    let a = arg;
    // Only where there is no sensible default, and only where there is no
    // sensible *nothing*: `:theme` with no name shows the list, which is a
    // better answer than a prompt. `:hash` means sha256 and `:readonly` means
    // on; stopping to ask would be a prompt with one likely answer, which is
    // the kind of question that trains people to hit Enter.
    if (cmd.arg && !a && !cmd.optional) {
        // What it does, not what it is called. `Shift+F` opened a box headed
        // `:find` and `Ctrl+F` one headed `:grep`, which are the two names
        // hardest to tell apart from the outside — and neither says whether it
        // is about to look at names or inside files. Every command already
        // carries the sentence (`about`) and the word for its field (`arg`);
        // they were being thrown away right here.
        a = await askFor(cmd.about, '', {
            hint: cmd.arg,
            note: `:${cmd.name}`,
            wide: true,
        });
        if (a === null) return;
    }
    try {
        await cmd.run(a, invokedAs);
    } catch (e) {
        say(String(e.message || e), true);
    }
}

/// `C` — every command, fuzzy.
function openPalette() {
    const rows = commands().map((c) => ({ label: `:${c.name}`, sub: c.about, cmd: c }));
    show(tr('command', 'コマンド'), tr(`${rows.length}`, `${rows.length} 個`), rows, {
        // The one the help has always called あいまい検索 and which walked
        // a hundred and thirty rows with j and k until now.
        filter: true,
        hint: tr('type to narrow (:name or the description)', '打って絞り込み（:name か説明）'),
        foot: tr('type to narrow   ↑↓ choose   Enter run   Esc close', '打って絞る   ↑↓ 選ぶ   Enter 実行   Esc 閉じる'),
        pick: (row) => { closeReport(); runCommand(row.cmd, ''); },
    });
}

// ---- The commands themselves ----

async function cmdCount() {
    const r = await ask('count', { pane: state.focus });
    if (!r) return;
    const most = r.by_ext.reduce((n, e) => Math.max(n, e.steps), 0) || 1;
    const rows = r.by_ext.map((e) => ({
        n: e.steps.toLocaleString(),
        bar: e.steps / most,
        label: e.ext,
        sub: tr(`${e.files} files`, `${e.files} ファイル`),
    }));
    show(tr('Files and steps', 'ファイル数とステップ数'),
        tr(`${r.files.toLocaleString()} files   ${r.steps.toLocaleString()} steps`, `${r.files.toLocaleString()} ファイル   ${r.steps.toLocaleString()} ステップ`)
        + tr(`   (code ${r.steps.toLocaleString()} / blank ${r.blank.toLocaleString()} / comment ${r.comments.toLocaleString()})`, `   （実行 ${r.steps.toLocaleString()} / 空白 ${r.blank.toLocaleString()} / コメント ${r.comments.toLocaleString()}）`)
        + (r.truncated ? tr('   ※ stopped at the cap', '   ※上限で打ち切り') : ''),
        rows, { foot: tr('Esc close', 'Esc 閉じる') });
}

async function cmdDu(path) {
    const r = await ask('du', { pane: state.focus, ...(path ? { path } : {}) });
    if (!r) return;
    const big = r.rows.reduce((n, x) => Math.max(n, x.size), 0) || 1;
    const total = r.rows.reduce((n, x) => n + x.size, 0);
    const rows = r.rows.map((x) => ({
        n: human(x.size),
        // Two proportions in one row: the bar is this entry against the
        // biggest one (which is what the eye compares), the percentage is
        // against the whole folder (which is what you say out loud).
        bar: x.size / big,
        label: x.is_dir ? `${x.name}/` : x.name,
        sub: total ? `${(x.size * 100 / total).toFixed(1)}%` : '',
        path: x.path,
        is_dir: x.is_dir,
    }));
    const up = r.cwd.replace(/[\\/][^\\/]+$/, '') || r.cwd;
    show(tr('Disk usage', '容量分析'), tr(`${r.cwd}   ${human(total)} in all`, `${r.cwd}   合計 ${human(total)}`), rows, {
        foot: tr('Enter in   ← / Bksp up   Esc close', 'Enter 入る   ← / Bksp 親へ   Esc 閉じる'),
        pick: (row) => { if (row.is_dir) cmdDu(row.path); },
        act: {
            // cian-tui walks back out of a du tree with `-`, Backspace or ←.
            // Going in with no way out is a one-way screen.
            '-': () => { if (up !== r.cwd) cmdDu(up); },
            Backspace: () => { if (up !== r.cwd) cmdDu(up); },
            ArrowLeft: () => { if (up !== r.cwd) cmdDu(up); },
        },
    });
}

async function cmdAttr() {
    const r = await ask('attr', { pane: state.focus });
    if (!r) return;
    const rows = [
        { label: tr('Kind', '種類'), sub: r.is_dir ? tr('Folder', 'ディレクトリ') : tr('File', 'ファイル') },
        { label: tr('Mode', 'モード'), sub: r.mode || tr('(none)', '(なし)') },
        { label: tr('Read-only', '読み取り専用'), sub: r.readonly ? tr('yes', 'はい') : tr('no', 'いいえ') },
        { label: tr('Owner', '所有者'), sub: r.owner || tr('(none)', '(なし)') },
        { label: tr('Size', '大きさ'), sub: r.size === null ? '—' : tr(`${human(r.size)} (${r.size.toLocaleString()} bytes)`, `${human(r.size)}（${r.size.toLocaleString()} バイト）`) },
        { label: tr('Where', '場所'), sub: r.path },
    ];
    show(tr('Attributes', '属性'), r.name, rows, { foot: tr('Esc close', 'Esc 閉じる') });
}

async function cmdChmod(spec) {
    const r = await ask('chmod', { pane: state.focus, spec });
    if (!r) return;
    await reread();
    say(tr(`set ${r.changed} to ${r.spec}`, `${r.changed} 件を ${r.spec} にしました`));
}

async function cmdReadonly(onOff) {
    const on = !/^(off|no|false|0|解除)$/i.test((onOff || '').trim());
    const r = await ask('readonly', { pane: state.focus, on });
    if (!r) return;
    await reread();
    say(tr(`made ${r.changed} ${on ? 'read-only' : 'writable'}`, `${r.changed} 件を${on ? '読み取り専用に' : '書き込み可に'}しました`));
}

async function cmdHash(kind) {
    const k = /md5/i.test(kind || '') ? 'md5' : 'sha256';
    say(tr(`computing ${k}…`, `${k} を計算中…`));
    const r = await ask('hash', { pane: state.focus, kind: k });
    if (!r) return;
    show(tr(`Checksum (${r.kind})`, `チェックサム（${r.kind}）`), tr(`${r.rows.length} items`, `${r.rows.length} 件`),
        r.rows.map((x) => ({ label: x.name, sub: x.sum })),
        { foot: tr('Esc close', 'Esc 閉じる') });
}

async function cmdSearch(mode, needle) {
    if (!needle) return;
    say(tr(`searching ${mode === 'content' ? 'inside' : 'by name'}…`, `${mode === 'content' ? '中を' : '名前を'}探しています…`));
    const r = await ask('search', { pane: state.focus, needle, mode });
    if (!r) return;
    const rows = r.hits.map((h) => ({
        n: h.line ? String(h.line.n) : null,
        label: h.rel + (h.is_dir ? '/' : ''),
        sub: h.line ? h.line.text.trim() : '',
        path: h.path,
        is_dir: h.is_dir,
    }));
    // Remembered, so Ctrl+N can walk them after this screen is gone.
    hits.list = r.hits.map((h) => ({ path: h.path, line: h.line ? h.line.n : 0 }));
    hits.at = -1;
    hits.needle = needle;
    show(mode === 'content' ? `grep ${needle}` : `find ${needle}`,
        tr(`${r.root}   ${rows.length}${r.truncated ? ' (stopped at the cap)' : ''}`, `${r.root}   ${rows.length} 件${r.truncated ? '（打ち切り）' : ''}`),
        rows, {
            // Still searching: the sheet takes the search colour, as the pane
            // does while the prompt is up.
            mode: 'search',
            foot: tr('Enter go there   p into a pane   r replace across   Esc close', 'Enter そこへ   p 一覧に読み込む   r 一括置換   Esc 閉じる'),
            pick: (row) => {
                closeReport();
                hits.at = rows.indexOf(row) - 1;
                if (row.n) hopHit(1);
                else revealPath(row.path, row.is_dir);
            },
            act: {
                r: async () => {
                    // Replace across every file the grep matched. The plan
                    // first, every line of it: this writes to files that are
                    // not open and `u` cannot take it back.
                    const spec = await askFor(tr('replace s/old/new/g', '置換 s/古い/新しい/g'), `s/${needle}//g`);
                    if (spec === null || !spec.trim()) return;
                    const paths = [...new Set(rows.map((x) => x.path))];
                    const plan = await ask('replaceplan', { paths, spec });
                    if (!plan) return;
                    if (!plan.changes.length) { say(tr('no line would change', '変わる行がありません')); return; }
                    closeReport();
                    showReplacePlan(spec, plan);
                },
                p: async () => {
                    const paths = rows.map((x) => x.path);
                    const which = state.focus;
                    const pane = await ask('panelize', {
                        pane: which, paths, label: `${mode === 'content' ? 'grep' : 'find'} ${needle}`,
                    });
                    if (!pane) return;
                    closeReport();
                    state[which] = pane;
                    draw(which);
                    say(tr(`${paths.length} loaded into the pane (Esc leaves)`, `${paths.length} 件を一覧に読み込みました（Esc で戻る）`));
                },
            },
        });
}

/// Put the cursor on a path, entering its directory if need be.
/// Put the cursor on this path, reading its directory only if we are not
/// standing in it already.
///
/// Three things wanted this — jumping to a search hit, landing on a row
/// picked out of a report, stepping through the files opened at once — and
/// each had written its own. Staying put when the directory is already the
/// right one is not only faster: a re-read would drop the marks and the
/// filter, which is a visible loss for a gesture that means "look over
/// there", not "start again".
async function landOn(path, isDir = false) {
    const which = state.focus;
    const dir = isDir ? path : path.replace(/[\\/][^\\/]*$/, '');
    if (state[which].cwd !== dir) {
        const pane = await ask('list', { pane: which, path: dir });
        if (!pane) return false;
        state[which] = pane;
    }
    if (!isDir) {
        const at = state[which].entries.findIndex((x) => x.path === path);
        if (at >= 0) state[which].cursor = at;
    }
    draw(which);
    return true;
}

async function revealPath(path, isDir) {
    if (await landOn(path, isDir)) say(state[state.focus].cwd);
}

async function cmdBranch() {
    const which = state.focus;
    if (state[which].flat) { await leaveFlat(); return; }
    say(tr('gathering everything below here…', 'この配下を集めています…'));
    const r = await ask('branch', { pane: which });
    if (!r) return;
    state[which] = r.pane;
    draw(which);
    say(tr(`${r.found} found (b or Esc leaves)`, `${r.found} 件（b か Esc で戻る）`));
}

async function leaveFlat() {
    const which = state.focus;
    const pane = await ask('leaveflat', { pane: which });
    if (!pane) return;
    state[which] = pane;
    draw(which);
    say(pane.cwd);
}

async function step(dir) {
    const which = state.focus;
    const pane = await ask(dir, { pane: which });
    if (!pane) return;
    state[which] = pane;
    draw(which);
    say(pane.cwd);
}

async function cmdHistory() {
    const r = await ask('history', { pane: state.focus });
    if (!r) return;
    const rows = [
        ...r.back.map((p) => ({ n: '←', label: p })),
        // 「いまここ」は口語すぎる、と言われた。一覧の中で自分の位置を示す
        // 語は「現在地」── 地図でも案内でも使われている、説明の要らない語。
        { n: '', label: r.cwd, sub: tr('current', '現在地') },
        ...r.forward.map((p) => ({ n: '→', label: p })),
    ];
    show(tr('History', '履歴'), r.cwd, rows, {
        foot: tr('Enter go there   a bookmark here   Esc close', 'Enter そこへ   a ここを登録   Esc 閉じる'),
        pick: (row) => { closeReport(); revealPath(row.label, true); },
        act: {
            // Bookmark the row you are looking at. The history is where you
            // notice that a place is worth keeping — cian-tui puts `a` here
            // for that reason.
            a: async () => {
                const row = report.rows[report.at];
                if (!row) return;
                const name = await askFor(tr('a name for this place', 'この場所を登録する名前'),
                    row.label.split(/[\\/]/).pop() || row.label);
                if (name === null) return;
                const made = await ask('bookmark', { path: row.label, name: name.trim() });
                if (made) say(tr(`bookmarked ${made.name}`, `${made.name} を登録しました`));
            },
        },
    });
}

let lastGG = 0;

/// `v` — mark a run without pressing Space down it.
///
/// The anchor is where it started; every move re-marks from there, so
/// overshooting is corrected by moving back rather than by starting again.
/// `Enter` or a second `v` keeps it, `Esc` puts the marks back as they were.
const visual = { on: false, from: 0, was: null };

async function startVisual() {
    const pane = state[state.focus];
    if (!pane) return;
    if (visual.on) { await endVisual(true); return; }
    visual.on = true;
    visual.from = pane.cursor;
    visual.was = pane.entries.filter((x) => x.marked).map((x) => x.path);
    await paintVisual();
    say(tr('visual selection — Enter confirms, Esc cancels', 'ビジュアル選択 — Enter で確定、Esc で取消'));
}

async function paintVisual() {
    const which = state.focus;
    const pane = state[which];
    const lo = Math.min(visual.from, pane.cursor);
    const hi = Math.max(visual.from, pane.cursor);
    const want = new Set(visual.was);
    pane.entries.forEach((x, i) => { if (i >= lo && i <= hi && !x.parent) want.add(x.path); });
    const next = await ask('setmarks', { pane: which, paths: [...want] });
    if (!next) return;
    next.cursor = pane.cursor;
    state[which] = next;
    draw(which);
    say(tr(`visual: ${next.marked}`, `ビジュアル: ${next.marked} 件`));
}

async function endVisual(keep) {
    if (!visual.on) return;
    visual.on = false;
    if (!keep) {
        const which = state.focus;
        const next = await ask('setmarks', { pane: which, paths: visual.was });
        if (next) { state[which] = next; draw(which); }
        say(tr('cancelled', '取り消しました'));
    } else {
        say(tr(`${state[state.focus].marked} marked`, `${state[state.focus].marked} 件をマーク`));
    }
    visual.was = null;
}

/// `f` looks in *this* listing, and `n`/`N` walk the matches.
///
/// Not the same as `/`, which narrows the listing to what matches, and not the
/// same as `Shift+F`, which walks the whole tree below here. The terminal
/// build keeps all three, and they answer three different questions: where is
/// it, show me only those, and is it anywhere under here.
let here = { needle: '', at: -1 };

async function searchHere() {
    const needle = await askFor(tr('search this listing', 'この一覧を検索'), here.needle, {
        // The box stays open and ↑↓ walk the matches — cian-tui's own
        // behaviour on this popup, and the difference between finding *a*
        // `main.rs` and finding the one you meant.
        onStep: (value, step) => {
            if (!value.trim()) return;
            here.needle = value;
            hopHere(step);
        },
    });
    if (needle === null || !needle) return;
    here.needle = needle;
    here.at = -1;
    hopHere(1);
}

function hopHere(step) {
    const pane = state[state.focus];
    if (!pane || !here.needle) return;
    const q = here.needle.toLowerCase();
    const hits = [];
    pane.entries.forEach((x, i) => {
        if (!x.parent && x.name.toLowerCase().includes(q)) hits.push(i);
    });
    if (!hits.length) { say(tr(`${here.needle} — not found`, `${here.needle} — 見つかりません`), true); return; }
    if (here.at < 0) {
        // The first hop starts from where the eye is, not from the top.
        const ahead = hits.findIndex((n) => n > pane.cursor);
        here.at = step > 0 ? (ahead < 0 ? 0 : ahead) : (ahead <= 0 ? hits.length - 1 : ahead - 1);
    } else {
        here.at = (here.at + step + hits.length) % hits.length;
    }
    pane.cursor = hits[here.at];
    draw(state.focus);
    say(`${here.needle}   ${here.at + 1} / ${hits.length}`);
}

// ---- Left against right, bulk rename, archives ----

/// `=` — one key, and what the two cursors point at decides the answer.
/// Which encoding the comparison is being read under. cian-tui's `e` on this
/// screen: two Shift_JIS files read as UTF-8 differ on every line that holds a
/// Japanese character, which is a comparison that says nothing about the files.
const DIFF_ENCS = [null, 'sjis', 'utf8', 'utf16le', 'utf16be'];
const ENC_NAME = { sjis: 'Shift_JIS', utf8: 'UTF-8', utf16le: 'UTF-16LE', utf16be: 'UTF-16BE' };
let diffEnc = null;

async function cmdCompare() {
    say(tr('comparing…', '比べています…'));
    const r = await ask('compare', { folded: diffFolded, enc: diffEnc || undefined });
    if (!r) return;
    if (r.kind === 'dirs') {
        // Same as the file case, and for the same reason (actions.rs:1276):
        // two identical folders used to open a list with nothing in it.
        if (!r.rows.length) {
            show(tr('The two folders are identical', '2つのディレクトリは同一です'),
                `${r.left}  ↔  ${r.right}`, [], { foot: tr('Esc close', 'Esc 閉じる') });
            return;
        }
        const mark = { left: tr('◀ left only', '◀ 左だけ'), right: tr('right only ▶', '右だけ ▶'), differ: tr('≠ differ', '≠ 違う') };
        const roots = { left: r.left, right: r.right };
        show(tr('Folder comparison', 'ディレクトリ比較'), tr(`${r.left}   ↔   ${r.right}   ${r.rows.length}${r.truncated ? ' (stopped at the cap)' : ''}`, `${r.left}   ↔   ${r.right}   ${r.rows.length} 件${r.truncated ? '（打ち切り）' : ''}`),
            r.rows.map((x) => ({
                n: mark[x.status],
                label: x.rel + (x.is_dir ? '/' : ''),
                rel: x.rel,
                status: x.status,
            })),
            {
                foot: tr('Enter takes both panes there   > to the right   < to the left   ] match the right   [ match the left   c copy   w save   Esc', 'Enter 両ペインをそこへ   > 右へ   < 左へ   ] 右を揃える   [ 左を揃える   c コピー   w 保存   Esc'),
                // Enter takes both panes to the entry, which is what you want
                // after finding the difference: cian-tui does it, and without
                // it you memorise a path and type it twice.
                pick: async (row) => {
                    closeReport();
                    const dir = (root) => `${root}/${row.rel}`.replace(/[\\/][^\\/]+$/, '');
                    await landOn(dir(r.left), true);
                    state.focus = state.focus === 'left' ? 'right' : 'left';
                    await landOn(dir(r.right), true);
                    state.focus = state.focus === 'left' ? 'right' : 'left';
                    draw('left'); draw('right');
                    say(row.rel);
                },
                act: {
                    '>': () => copyAcross(roots, 'left', 'right'),
                    '<': () => copyAcross(roots, 'right', 'left'),
                    ']': () => syncTree(roots, 'left', 'right'),
                    '[': () => syncTree(roots, 'right', 'left'),
                    c: () => copyReport(tr('Folder comparison', 'ディレクトリ比較')),
                    w: () => saveReport(`${r.left} ↔ ${r.right}`),
                },
            });
        return;
    }
    if (r.kind === 'files') {
        // Said in a sheet, not whispered in the status line.
        //
        // "a screen saying identical over an empty list is a screen that
        // wasted a keystroke" was the argument for a one-liner, and the
        // terminal build had already been through it and come back the other
        // way: "the compare felt unresponsive when identical folders only
        // whispered a message" (actions.rs:1276). It is exactly what
        // happened here — `=` reported as doing nothing at all. The keystroke
        // is not wasted; "they are the same" is the answer.
        if (!r.added && !r.removed && !r.changed) {
            show(tr('The two files are identical', '2つのファイルは同一です'),
                `${r.left}  ↔  ${r.right}`, [], { foot: tr('Esc close', 'Esc 閉じる') });
            return;
        }
        // Two files go to the **diff editor**, not to a list of lines.
        //
        // cian-tui shows a list because a terminal has no diff editor; the
        // window has had one all along, one keystroke behind the list. Side by
        // side, coloured within the line, foldable, and both halves editable —
        // that is what a comparison is *for*, and it was the second thing you
        // reached rather than the first. `L` still gets the list, which keeps
        // what a list is better at: `f` to unfold, `c` and `w` to take it out
        // of the window, `x` to ask about it.
        if (!compareAsList) { closeReport(); await cmdDiffEdit(); return; }
        compareAsList = false;
    }
    // A difference is read by its differences, so the identical runs between
    // them are folded away — the engine did that; here they are one row
    // saying how many went past.
    const glyph = { same: ' ', changed: '~', removed: '-', added: '+', skipped: '⋯' };
    const rows = r.rows.map((x) => {
        if (x.kind === 'skipped') return { n: '⋯', label: tr(`── ${x.lines} identical lines ──`, `── 同じ ${x.lines} 行 ──`), sub: '' };
        return {
            n: `${glyph[x.kind]} ${x.ln ?? ''}`.trim(),
            label: x.left ?? '',
            sub: x.right ?? '',
        };
    });
    const encNote = diffEnc ? `   [${ENC_NAME[diffEnc]}]` : '';
    show(tr('File comparison', 'ファイル比較'), `${r.left}   ↔   ${r.right}   ${r.summary}${encNote}`, rows, {
        foot: tr('Enter side by side   / search   n/N next, prev   f unfold   e encoding   > to the right   < to the left   x AI   c copy   w save   Esc', 'Enter 並べて編集   / 検索   n/N 次・前   f 畳みを解く   e 文字コード   > 右へ   < 左へ   x AI   c コピー   w 保存   Esc'),
        pick: () => { closeReport(); cmdDiffEdit(); },
        act: {
            c: () => copyReport(`${r.left} ↔ ${r.right}`),
            w: () => saveReport(`${r.left} ↔ ${r.right}`),
            // The same three cian-tui puts on this screen. A long diff is a
            // list you search, and the folded runs are folded until you want
            // one of them.
            '/': () => diffFind(),
            n: () => diffHop(1),
            N: () => diffHop(-1),
            // Re-running from the list stays in the list. Without this,
            // changing the folding or the encoding threw you into the diff
            // editor — the answer to a question you asked *of the list*.
            f: () => { diffFolded = !diffFolded; compareAsList = true; closeReport(); cmdCompare(); },
            x: () => { closeReport(); runCommand(findCommand('aidiff'), ''); },
            // cian-tui's `e` on this screen, cycling the same list its viewer
            // offers. Both sides are decoded again and compared afresh.
            e: () => {
                diffEnc = DIFF_ENCS[(DIFF_ENCS.indexOf(diffEnc) + 1) % DIFF_ENCS.length];
                compareAsList = true;
                closeReport();
                cmdCompare();
            },
            // And `>` / `<`: put one side's file over the other. The whole
            // point of standing two files next to each other is often to make
            // one of them the other, and the window could only look.
            '>': () => copyOneOver(r, 'right'),
            '<': () => copyOneOver(r, 'left'),
        },
    });
}

/// Ask for the list rather than the diff editor, once. Set by `L` from the
/// editor and cleared the moment the list is drawn — a preference nobody set
/// is not a preference.
let compareAsList = false;

/// Whether the identical runs in a file comparison are folded. cian-tui's
/// `f`, and off by default for the same reason: a diff is read by its
/// differences, and the sameness between them is noise until it is not.
let diffFolded = true;

/// `]` / `[` — make one side match the other, whole.
///
/// The per-entry `>` and `<` are for picking; this is for "these two should
/// be the same". Named and counted before anything happens, because it is
/// the one action on this screen that can overwrite work in bulk.
async function syncTree(roots, from, to) {
    const rows = report.rows.filter((x) => x.status !== 'same');
    // Only what is missing on `to` or differs — an entry that exists only on
    // the destination side is not made by copying anything.
    const going = rows.filter((x) => x.status === 'differ'
        || x.status === (from === 'left' ? 'left' : 'right'));
    if (!going.length) { say(tr('nothing goes that way', 'その向きに送るものはありません')); return; }
    closeReport();
    const ok = await confirm(
        tr(`Match ${going.length} ${from === 'left' ? 'left → right' : 'right → left'}`, `${going.length} 件を ${from === 'left' ? '左 → 右' : '右 → 左'} に揃えます`),
        `${going.map((x) => x.rel).slice(0, 20).join('\n')}`
        + (going.length > 20 ? tr(`\n… and ${going.length - 20} more`, `\n… 他 ${going.length - 20} 件`) : '')
        + tr('\n\nsame names are overwritten', '\n\n同じ名前は上書きされます'),
    );
    if (!ok) { say(tr('stopped', 'やめました')); return; }
    let done = 0;
    for (const row of going) {
        const src = `${roots[from]}/${row.rel}`;
        const dest = `${roots[to]}/${row.rel}`.replace(/[\\/][^\\/]+$/, '');
        const r = await ask('copyone', { src, dest });
        if (r) done += 1;
    }
    await reread();
    say(tr(`matched ${done}`, `${done} 件を揃えました`));
}

/// `/` and `n`/`N` on a file comparison — the diff is a list, and a long one.
let diffNeedle = '';

async function diffFind() {
    const q = await askFor(tr('search the comparison', '比較結果を検索'), diffNeedle);
    if (q === null || !q) return;
    diffNeedle = q;
    diffHop(1);
}

function diffHop(step) {
    if (!report.on || !diffNeedle) return;
    const q = diffNeedle.toLowerCase();
    const n = report.rows.length;
    for (let i = 1; i <= n; i++) {
        const at = ((report.at + step * i) % n + n) % n;
        const row = report.rows[at];
        if (`${row.label} ${row.sub || ''}`.toLowerCase().includes(q)) {
            report.at = at;
            drawReport();
            return;
        }
    }
    say(tr(`${diffNeedle} — not found`, `${diffNeedle} — 見つかりません`), true);
}

/// `>` / `<` in a directory comparison — put this entry on the other side.
///
/// The row knows where it is missing from, so the direction is checked rather
/// than assumed: copying a file that only exists on the right *to* the right
/// is a no-op that would still ask for a confirmation, and copying the wrong
/// way over a newer file is the mistake this screen exists to prevent.
/// Overwrite one side of a file comparison with the other.
///
/// cian-tui's `>` / `<` on the diff screen — with its confirmation, because
/// this is the one key here that destroys something. The name says which file
/// is about to stop existing as it is.
async function copyOneOver(r, to) {
    const from = to === 'right' ? 'left' : 'right';
    const src = r[`${from}_path`];
    const dst = r[`${to}_path`];
    if (!src || !dst) { say(tr('the paths are not known', 'パスが分かりません'), true); return; }
    if (!await confirm(tr(`Overwrite ${r[to]} with ${r[from]}`, `${r[to]} を ${r[from]} で上書きします`),
        tr(`${src}\n  →  ${dst}\n\nwhat ${r[to]} holds now is lost`, `${src}\n  →  ${dst}\n\n${r[to]} のいまの中身は失われます`))) {
        say(tr('stopped', 'やめました'));
        return;
    }
    const done = await ask('copyone', { src, dest: dst.replace(/[\\/][^\\/]*$/, '') });
    if (!done) return;
    closeReport();
    await reread();
    say(tr(`${r[from]} written over ${r[to]}`, `${r[from]} を ${r[to]} へ上書きしました`));
}

async function copyAcross(roots, from, to) {
    const row = report.rows[report.at];
    if (!row) return;
    if (row.status === (from === 'left' ? 'right' : 'left')) {
        say(tr(`${row.rel} is not on the ${from === 'left' ? 'left' : 'right'}`, `${row.rel} は${from === 'left' ? '左' : '右'}にありません`), true);
        return;
    }
    const src = `${roots[from]}/${row.rel}`;
    const destDir = `${roots[to]}/${row.rel}`.replace(/[\\/][^\\/]*$/, '');
    if (!await confirm(tr(`Copy ${row.rel} to the ${to === 'right' ? 'right' : 'left'}`, `${row.rel} を${to === 'right' ? '右' : '左'}へコピー`), `${src}\n  →  ${destDir}`)) {
        say(tr('stopped', 'やめました'));
        return;
    }
    const r = await ask('copyone', { src, dest: destDir });
    if (!r) return;
    say(tr(`copied ${row.rel}`, `${row.rel} をコピーしました`));
}

/// `c` / `w` on any report — the list as text, to the clipboard or to a file.
///
/// A comparison is something people paste into a ticket. Reading it off the
/// screen and retyping it is the alternative, and that is where the typos in
/// change requests come from.
async function copyReport(title) {
    const text = report.rows
        .map((x) => [x.n, x.label, x.sub].filter(Boolean).join('\t'))
        .join('\n');
    await navigator.clipboard.writeText(`${title}\n${text}`);
    say(tr(`${report.rows.length} lines on the clipboard`, `${report.rows.length} 行をクリップボードへ`));
}

async function saveReport(title) {
    const name = await askFor(tr('a name to save it as', '保存する名前'), 'compare.txt');
    if (name === null || !name) return;
    const text = report.rows
        .map((x) => [x.n, x.label, x.sub].filter(Boolean).join('\t'))
        .join('\n');
    const r = await ask('writefile', { pane: state.focus, name, text: `${title}\n${text}\n` });
    if (!r) return;
    await reread();
    say(tr(`saved to ${r.wrote}`, `${r.wrote} に保存しました`));
}

/// `=` in the comparison, or `:diffedit` — the two files side by side, both
/// editable.
///
/// The report screen answers "what differs"; this answers "let me fix it".
/// Same two files, a different question — and fixing a difference by reading
/// it in one window and typing in another is how the wrong half gets edited.
const pair = { on: false, ed: null };

async function cmdDiffEdit() {
    const r = await ask('twofiles', {});
    if (!r) return;
    let monaco;
    try {
        monaco = await loadMonaco();
    } catch (e) { say(e.message, true); return; }

    if (viewer.on) await closeView(false);
    if (report.on) closeReport();
    setViewerOn(true);
    pair.on = true;
    viewer.name = `${r.left.name} ↔ ${r.right.name}`;
    el.view.hidden = false;
    el.vBody.hidden = false;
    el.vPic.hidden = true;
    el.vName.textContent = viewer.name;
    el.vAbout.textContent = tr('both sides are editable — Ctrl+S saves both', '左右とも編集できます — Ctrl+S でどちらも保存');
    el.vFoot.textContent = tr('F7 / Shift+F7 next / previous difference   ·   L as a list   ·   Ctrl+S saves   ·   Esc ×3 closes', 'F7 / Shift+F7 次 / 前の相違   ·   L 一覧で見る   ·   Ctrl+S 保存   ·   Esc ×3 閉じる');

    const lang = MONACO_LANG[r.lang] || 'plaintext';
    // A fresh diff editor each time: reusing one across different file pairs
    // means old models hanging on to files nobody has open.
    if (pair.ed) pair.ed.dispose();
    // **And the plain editor has to let go of this node first.**
    //
    // `replaceChildren()` empties the element but tells Monaco nothing, so
    // the ordinary editor was still holding it — and building the diff editor
    // on the same node printed `Element already has context attribute:
    // vbody` into the console every time. It surfaced at the tail of the
    // standard round, once the round grew far enough to compare two files
    // after opening one.
    if (viewer.ed) { viewer.ed.dispose(); viewer.ed = null; viewer.vim = null; }
    el.vBody.replaceChildren();
    pair.ed = monaco.editor.createDiffEditor(el.vBody, {
        theme: editorTheme(),
        automaticLayout: true,
        fontFamily: getComputedStyle(document.body).fontFamily,
        fontSize: FONT.at,
        originalEditable: true,
        renderSideBySide: true,
        minimap: { enabled: false },
    });
    pair.ed.setModel({
        original: monaco.editor.createModel(r.left.lines.join('\n'), lang),
        modified: monaco.editor.createModel(r.right.lines.join('\n'), lang),
    });
    say(`${r.left.name} ↔ ${r.right.name}`);
}

async function savePair() {
    if (!pair.ed) return;
    const m = pair.ed.getModel();
    const l = await ask('save', { lines: m.original.getValue().split(/\r?\n/) });
    const r = await ask('savepair', { lines: m.modified.getValue().split(/\r?\n/) });
    if (!l && !r) return;
    await reread();
    say(tr(`saved ${[l && l.saved, r && r.saved].filter(Boolean).join('  and  ')}`, `${[l && l.saved, r && r.saved].filter(Boolean).join('  と  ')} を保存しました`));
}

/// The plan first, always — the hundred new names before any of them exists.
async function cmdRenamePattern(pattern) {
    const r = await ask('renameplan', { pane: state.focus, pattern });
    if (!r) return;
    const changing = r.rows.filter((x) => !x.same);
    if (!changing.length) { say(tr('no name would change', '変わる名前がありません')); return; }
    const clashes = changing.filter((x) => x.clash);
    show(tr(`Bulk rename   ${r.pattern}`, `一括リネーム   ${r.pattern}`),
        tr(`${changing.length} would change`, `${changing.length} 件が変わります`) + (clashes.length ? tr(`   ★ ${clashes.length} already exist`, `   ★ ${clashes.length} 件は既にある名前`) : ''),
        r.rows.map((x) => ({
            n: x.clash ? '★' : (x.same ? '=' : '→'),
            label: x.from,
            sub: x.to,
        })),
        {
            foot: clashes.length
                ? tr('★ names already exist — Enter does the rest   Esc cancels', '★ の名前は既にあります — Enter で残りだけ実行   Esc 取消')
                : tr('Enter run   Esc cancel', 'Enter 実行   Esc 取消'),
            pick: async () => {
                closeReport();
                const rows = changing.filter((x) => !x.clash);
                if (!rows.length) { say(tr('no row can be run', '実行できる行がありません'), true); return; }
                if (!await confirm(tr(`Rename ${rows.length}`, `${rows.length} 件の名前を変えます`),
                    rows.map((x) => `${x.from}  →  ${x.to}`).join('\n'))) { say(tr('stopped', 'やめました')); return; }
                const done = await ask('renameapply', { rows });
                if (!done) return;
                await reread();
                if (done.errors.length) say(done.errors.join('  /  '), true);
                else say(tr(`renamed ${done.renamed}`, `${done.renamed} 件の名前を変えました`));
            },
        });
}

async function cmdCompress(kind, encrypted = false) {
    const pane = state[state.focus];
    const rows = pane.entries.filter((x) => x.marked);
    const what = rows.length ? rows : [pane.entries[pane.cursor]].filter((x) => x && !x.parent);
    if (!needTargets(what)) return;
    const name = await askFor(tr('a name for the archive (no extension)', 'アーカイブの名前（拡張子なし）'), what[0].name.replace(/\.[^.]*$/, ''));
    if (name === null || !name) return;
    let password;
    if (encrypted) {
        password = await askFor(tr('a password for the zip', 'zip のパスワード'), '', { secret: true });
        if (password === null || !password) return;
    }
    say(tr(`making the ${kind}…`, `${kind} を作っています…`));
    const r = await ask('compress', { pane: state.focus, kind, name, password });
    if (!r) return;
    state[state.focus] = r.pane;
    draw(state.focus);
    if (r.errors.length) say(r.errors.join('  /  '), true);
    else say(tr(`made ${r.made} (${r.ok} files)`, `${r.made} を作りました（${r.ok} 件）`));
}

async function cmdExtract() {
    const r = await ask('extract', { pane: state.focus });
    if (!r) return;
    state[state.focus] = r.pane;
    draw(state.focus);
    if (r.errors.length) say(r.errors.join('  /  '), true);
    else say(tr(`extracted ${r.from} (${r.ok} files)`, `${r.from} を展開しました（${r.ok} 件）`));
}

async function cmdArchiveList() {
    const r = await ask('archivelist', { pane: state.focus });
    if (!r) return;
    show(r.name, tr(`${r.members.length} members`, `${r.members.length} 件`),
        r.members.map((m) => ({
            n: m.is_dir ? '' : human(m.size),
            label: m.name,
            sub: m.is_dir ? '' : tr(`${human(m.compressed)} packed`, `圧縮後 ${human(m.compressed)}`),
            member: m.name,
            is_dir: m.is_dir,
        })),
        {
            // cian-tui's Archive popup extracts from here: Enter for the one
            // under the cursor, `a` for all of it. Reading the list and then
            // having to know `:unzip` is two screens for one intention.
            foot: tr('Enter extract this one   a extract all   Esc close', 'Enter この1件を展開   a 全部展開   Esc 閉じる'),
            pick: async (row) => {
                if (row.is_dir) return;
                closeReport();
                const done = await ask('extract', { pane: state.focus, member: row.member });
                if (!done) return;
                await reread();
                say(tr(`extracted ${row.member}`, `${row.member} を展開しました`));
            },
            act: {
                a: async () => {
                    closeReport();
                    const done = await ask('extract', { pane: state.focus });
                    if (!done) return;
                    await reread();
                    say(tr(`extracted ${r.name}`, `${r.name} を展開しました`));
                },
            },
        });
}

// ---- Version control, duplicates, redo ----

async function cmdLog(justThisFile) {
    const r = await ask('log', { pane: state.focus, file: justThisFile });
    if (!r) return;
    show(r.of ? tr(`history of ${r.of}`, `${r.of} の履歴`) : tr('Commit log', 'コミットログ'),
        tr(`${r.kind}   ${r.commits.length} commits`, `${r.kind}   ${r.commits.length} 件`),
        r.commits.map((c) => ({ n: c.date, label: c.subject, sub: `${c.author}  ${c.hash}`, hash: c.hash })),
        {
            foot: tr('Enter the diff of that commit   Esc close', 'Enter そのコミットの差分   Esc 閉じる'),
            pick: (row) => cmdVcsDiff(row.hash),
        });
}

/// A diff, shown the way a diff reads: the sign in its own column, so `+` and
/// `-` line up down the page instead of hiding at the start of the text.
async function cmdVcsDiff(hash) {
    const r = await ask('vcsdiff', { pane: state.focus, ...(hash ? { hash } : {}) });
    if (!r) return;
    show(hash ? tr(`Diff ${hash}`, `差分 ${hash}`) : tr('Diff', '差分'), tr(`${r.lines.length} lines`, `${r.lines.length} 行`),
        r.lines.map((t) => ({
            n: t.startsWith('+') ? '+' : t.startsWith('-') ? '-' : t.startsWith('@') ? '@' : '',
            label: t,
        })),
        { foot: tr('Esc close', 'Esc 閉じる') });
}

async function cmdVcs(what) {
    // Discarding is the one of the three that loses work, and it was the one
    // that did not ask. cian-tui raises ConfirmDiscard for it.
    if (what === 'discard') {
        const pane = state[state.focus];
        const rows = pane.entries.filter((x) => !x.parent && x.marked);
        const here = pane.entries[pane.cursor];
        const targets = rows.length ? rows : (here && !here.parent ? [here] : []);
        if (!targets.length) { say(tr('nothing to work on', '対象がありません')); return; }
        const ok = await confirm(
            tr(`Discard changes in ${targets.length}`, `${targets.length} 件の変更を破棄`),
            tr(`${targets.map((x) => x.name).join('\n')}\n\nthis cannot be undone`, `${targets.map((x) => x.name).join('\n')}\n\n元には戻せません`),
        );
        if (!ok) { say(tr('stopped', 'やめました')); return; }
    }
    const r = await ask(what, { pane: state.focus });
    if (!r) return;
    state[state.focus] = r.pane;
    draw(state.focus);
    const verb = { stage: 'git add', unstage: 'git reset', discard: tr('Discard', '破棄') }[what];
    say(tr(`${verb} ${r.count}`, `${r.count} 件を ${verb} しました`));
}

async function cmdDedup() {
    say(tr('comparing the contents…', '中身を突き合わせています…'));
    const r = await ask('dedup', { pane: state.focus });
    if (!r) return;
    if (!r.groups.length) { say(tr('no two files have the same contents', '同じ中身のファイルはありません')); return; }
    // The first of each group starts unticked: a duplicate set with every
    // copy ticked is a set with nothing left. cian-tui's DupeReview is for
    // choosing which copies to lose, and one of them has to stay.
    const rows = [];
    r.groups.forEach((g, i) => {
        g.forEach((p, j) => rows.push({
            n: j === 0 ? `${i + 1}` : '', label: p, path: p, on: j !== 0,
        }));
    });
    show(tr('Files with identical contents', '中身が同じファイル'), tr(`${r.groups.length} groups — the first of each is the one kept`, `${r.groups.length} 組 — 各組の1つ目は残す側`), rows, {
        checks: true,
        foot: tr('Space off/on   a all   n none   Enter delete the chosen   Esc cancel', 'Space 外す／戻す   a 全部   n 全部外す   Enter 選んだ分を削除   Esc 取消'),
        pick: async (chosen) => {
            if (!chosen.length) { say(tr('no row is chosen', '選ばれている行がありません'), true); return; }
            closeReport();
            const ok = await confirm(tr(`${chosen.length} to the trash`, `${chosen.length} 件をゴミ箱へ`),
                chosen.map((x) => x.path).join('\n'));
            if (!ok) { say(tr('stopped', 'やめました')); return; }
            const done = await ask('delete', {
                pane: state.focus, paths: chosen.map((x) => x.path), mode: 'trash',
            });
            if (!done) return;
            beginOp(done, 'delete', tr('delete', '削除'));
        },
    });
}

/// `:view`, and the terminal build's aliases for it — `:icons` on its own
/// means `:view icons`, which is how fingers actually type it.
async function cmdView(arg, invokedAs) {
    const mode = (arg || invokedAs || '').trim();
    // `grid` と `icons` はアイコンモードのこと。**引退させたのは彼の判断**
    // （2026-09-05）── クラシックが基本、Explorer/Finder ふうが要るなら詳細
    // 一覧で足りる。描く側も 2026-09-06 に消したので、もう戻せる分岐は無い。
    // それでも打った人を「そんなモードは無い」で突き放さず、詳細一覧へ案内する。
    const map = { grid: 'details', icons: 'details', finder: 'details' };
    if (!mode || mode === 'view') {
        setView(VIEWS[(VIEWS.indexOf(viewMode) + 1) % VIEWS.length]);
    } else {
        setView(map[mode] || mode);
    }
    say(tr(`mode: ${viewName(viewMode)}`, `モード: ${viewName(viewMode)}`));
}

async function redo() {
    const r = await ask('redo', {});
    if (!r) return;
    state.left = r.left;
    state.right = r.right;
    draw('left');
    draw('right');
    say(r.said);
}

/// `Z` — the places this session has been, newest first.
///
/// The terminal build's jump list is history plus bookmarks; bookmarks need
/// somewhere to live, which is the same open question as the look and the
/// editor style, so this is the half that needs nothing written down.
async function cmdJump() {
    // Recents and bookmarks together, which is what the terminal build's `Z`
    // is: "fuzzy-jump to a recent / bookmarked directory". Bookmarks first —
    // a place worth naming outranks a place merely visited.
    const rows = [];
    const seen = new Set();
    const marks = await ask('shortcuts', {});
    if (marks) {
        for (const x of marks.rows) {
            if (x.target && !seen.has(x.target)) {
                seen.add(x.target);
                rows.push({ n: '★', label: x.name, sub: x.target, target: x.target });
            }
        }
    }
    for (const which of ['left', 'right']) {
        const r = await ask('history', { pane: which });
        if (!r) continue;
        for (const p of [r.cwd, ...r.back, ...r.forward]) {
            if (!seen.has(p)) {
                seen.add(p);
                rows.push({ n: '', label: p, target: p });
            }
        }
    }
    if (!rows.length) { say(tr('you have not been anywhere yet', 'まだどこにも行っていません')); return; }
    show(tr('where to', '行き先'), tr(`${rows.length} (★ = bookmarked)`, `${rows.length} 件（★ = 登録済み）`), rows, {
        // The terminal build calls `Z` a *fuzzy* jump, and a list of paths is
        // exactly the list where typing three letters beats arrowing.
        filter: true,
        hint: tr('type to narrow (part of a path)', '打って絞り込み（パスの一部）'),
        foot: tr('type to narrow   Enter go there   Esc close', '打って絞る   Enter そこへ   Esc 閉じる'),
        pick: (row) => { closeReport(); revealPath(row.target, true); },
    });
}

// ─────────────────────────────────────────────────────────────────────────
// The shell.
//
// **The terminal is in the engine.** Electron's usual answer is node-pty — a
// native module wanting a C++ toolchain and a rebuild against Electron's ABI,
// which is the several gigabytes this project already refused once. cian-pty
// is portable-pty and vt100, both plain Rust, and it is the same emulator the
// terminal build reads its shell through. So the window here knows nothing
// about escape sequences: it is handed a grid and it draws it. Interpreting
// them is a job with twenty years of edge cases in it, and a second answer to
// any of them is how two front ends stop looking like one program.
// ─────────────────────────────────────────────────────────────────────────
const term = { on: false, focused: false, rows: 24, cols: 80, tabs: 1, tab: 0, showing: null, names: [] };

/// How many cells fit. Measured from a real character rather than assumed:
/// the font is whatever the machine had, and three of the four looks disagree
/// about the size.
/// How big one character cell is, measured in the box the shell is drawn in.
///
/// The probe used to be a bare span dropped into the panel, which inherited
/// the *listing's* line height (--cell-h, made for rows you click on) and was
/// then multiplied by a hopeful 1.25 — so the panel was told it had seven
/// rows where twelve fitted, and two thirds of the shell was empty. It wears
/// `.sgrid` now, so it is measured under the rules the real thing is drawn
/// under, and ten lines are measured rather than one so rounding cannot
/// accumulate.
function measureCell() {
    const probe = document.createElement('div');
    probe.className = 'sgrid';
    probe.textContent = Array.from({ length: 10 }, () => 'M'.repeat(100)).join('\n');
    probe.style.cssText = 'position:absolute;visibility:hidden;left:-9999px;top:0;'
        + 'width:auto;height:auto;padding:0';
    el.sPanes.append(probe);
    const box = probe.getBoundingClientRect();
    const w = box.width / 100;
    const h = box.height / 10;
    probe.remove();
    return { w: w || 8, h: h || 20 };
}

/// The whole panel in cells. The engine divides this by each pane's share, so
/// a pane knows the width it actually has rather than the panel's — a shell
/// that thinks it is full width wraps at the wrong column, which is the
/// classic broken-split look.
function shellSize() {
    const { w, h } = measureCell();
    const box = el.sPanes.getBoundingClientRect();
    return {
        cols: Math.max(20, Math.floor((box.width - 16) / w)),
        rows: Math.max(4, Math.floor((box.height - 8) / h)),
    };
}

async function openShell(opts = {}) {
    const takeKeys = opts.focus !== false;
    el.shell.hidden = false;
    term.on = true;
    setShellFocus(takeKeys);
    const size = shellSize();
    term.rows = size.rows;
    term.cols = size.cols;
    const r = await ask('shellopen', { pane: state.focus, ...size });
    if (!r) { closeShell(); return; }
    takeShell(r);
    if (takeKeys) say(tr('shell — Esc goes back to the files', 'シェル — Esc でファイルへ戻る'));
    else draw('left');
}

function closeShell() {
    term.on = false;
    setShellFocus(false);
    el.shell.hidden = true;
}

/// Focus without closing. The panel stays visible while the files have the
/// keys — which is the point of docking it rather than opening it instead.
function blurShell() {
    setShellFocus(false);
    say(tr('back to the files (Shift+J returns to the shell)', 'ファイルへ戻りました（Shift+J でシェルへ）'));
}

/// A reply that carries a screen and the strip that belongs beside it.
/// How many split panes the shell is showing. Counted from the boxes on the
/// screen, which is the same thing the engine's `active_pane_count()` counts —
/// the menu asks so it can leave "type into all of them" out when there is
/// only one of them.
function shellPaneCount() {
    return el.sPanes.querySelectorAll('.sgrid').length || 1;
}

function takeShell(r) {
    if (r.gone) { closeShell(); return; }
    term.tabs = r.tabs ?? 1;
    term.tab = r.tab ?? 0;
    term.showing = r.showing ?? null;
    term.sync = !!r.sync;
    if (r.names) term.names = r.names;
    el.shell.classList.toggle('sync', term.sync);
    if (r.panes) { layoutShell(r.panes); layoutShellDividers(r.dividers); }
}

/// Place the panes where the engine said, and draw each one's screen.
///
/// Absolute positions from fractions, because the layout is a tree the engine
/// has already turned into rectangles. Deriving it again here out of nested
/// boxes would be the same arithmetic written twice.
/// A draggable handle on each inner split boundary.
///
/// cian-tui reaches these through `DividerTarget::ShellSplit`; a window has a
/// pointer, and four shells side by side with a boundary that can only be
/// moved by Ctrl+Shift+arrow is four shells you leave at whatever width they
/// started. The boxes come from the engine, which is where the tree is.
function layoutShellDividers(dividers) {
    for (const n of [...el.sPanes.querySelectorAll('.scut')]) n.remove();
    for (const d of dividers || []) {
        const cut = document.createElement('div');
        cut.className = `scut ${d.down ? 'h' : 'v'}`;
        cut.style.left = `${d.x * 100}%`;
        cut.style.top = `${d.y * 100}%`;
        if (d.down) cut.style.width = `${d.w * 100}%`;
        else cut.style.height = `${d.h * 100}%`;
        cut.addEventListener('mousedown', (e) => {
            e.preventDefault();
            e.stopPropagation();
            // The split's own box, taken from the divider: the boundary sits
            // inside it, so its extent is what the ratio is a fraction of.
            const box = el.sPanes.getBoundingClientRect();
            const spanStart = d.down ? d.y - dividerSpan(d).before : d.x - dividerSpan(d).before;
            const span = dividerSpan(d).total;
            const move = (ev) => {
                const at = d.down
                    ? (ev.clientY - box.top) / box.height
                    : (ev.clientX - box.left) / box.width;
                const ratio = span > 0 ? (at - spanStart) / span : 0.5;
                ask('shellsetratio', { id: d.id, down: d.down, ratio })
                    .then((r) => r && takeShell(r));
            };
            const up = () => {
                window.removeEventListener('mousemove', move);
                window.removeEventListener('mouseup', up);
                el.work.classList.remove('dragging');
                if (term.on) ask('shellresize', shellSize());
            };
            el.work.classList.add('dragging');
            window.addEventListener('mousemove', move);
            window.addEventListener('mouseup', up);
        });
        el.sPanes.append(cut);
    }
}

/// How much of the panel the split holding this divider covers, and how far
/// its near edge is from the divider. Worked out from the panes either side —
/// the engine sends the boundary, and the two boxes it separates say how wide
/// the thing being divided is.
function dividerSpan(d) {
    let lo = d.down ? d.y : d.x;
    let hi = lo;
    for (const n of el.sPanes.querySelectorAll('.sgrid')) {
        const st = n.style;
        const a = parseFloat(d.down ? st.top : st.left) / 100;
        const b = a + parseFloat(d.down ? st.height : st.width) / 100;
        // Only the boxes this boundary actually touches.
        if (Math.abs(b - (d.down ? d.y : d.x)) < 0.002) lo = Math.min(lo, a);
        if (Math.abs(a - (d.down ? d.y : d.x)) < 0.002) hi = Math.max(hi, b);
    }
    return { before: (d.down ? d.y : d.x) - lo, total: Math.max(0.01, hi - lo) };
}

function layoutShell(panes) {
    // "Which pane has the keys" is a question only a split can ask. With one
    // pane the answer is the panel's own top edge, already accented, and
    // drawing the frame as well put a second accent line a few pixels under
    // the first — see the stylesheet.
    el.shell.classList.toggle('split', panes.length > 1);
    const have = new Map([...el.sPanes.children].map((n) => [Number(n.dataset.id), n]));
    const want = new Set(panes.map((p) => p.id));
    for (const [id, node] of have) if (!want.has(id)) node.remove();
    for (const p of panes) {
        let node = have.get(p.id);
        if (!node) {
            node = document.createElement('div');
            node.className = 'sgrid';
            node.dataset.id = p.id;
            node.addEventListener('mousedown', () => focusPaneOf(p.id));
            el.sPanes.append(node);
        }
        node.style.left = `${p.x * 100}%`;
        node.style.top = `${p.y * 100}%`;
        node.style.width = `${p.w * 100}%`;
        node.style.height = `${p.h * 100}%`;
        node.classList.toggle('on', p.focused && term.focused);
        if (p.screen) drawShell(p.screen, node);
    }
}

async function focusPaneOf(id) {
    // Step until it lands: the engine owns the order, and asking it to move
    // one at a time is cheaper than teaching the window the tree.
    for (let i = 0; i < 8 && term.showing !== id; i++) {
        const r = await ask('shellfocus', { step: 1 });
        if (!r) return;
        takeShell(r);
    }
    setShellFocus(true);
}

/// `:theme` — pick a look by name, rather than cycling to it.
///
/// The switches menu walks them, which is right when there are four. Naming
/// one is what you want when you know which.
/// Every palette, the window's own three and cian-tui's eighteen, in one
/// list — because from where a person stands there is one question here.
function themeRows() {
    const rows = LOOKS.map(([, label], i) => ({
        n: !palette && i === look ? '●' : '',
        label,
        look: i,
    }));
    for (const name of palettes.keys()) {
        // No light/dark column: the gallery is live, so which way round a
        // palette goes is on the screen behind the list. A word saying so
        // would be describing what you can see.
        rows.push({ n: palette === name ? '●' : '', label: name, palette: name });
    }
    return rows;
}

/// Wear this one. `keep` is false while the cursor is only passing over it —
/// trying on eighteen palettes should not write eighteen settings.
function pickTheme(row, keep = true) {
    if (row.palette) {
        setPalette(row.palette, keep);
        if (keep) say(tr(`theme: ${row.palette}`, `配色: ${row.palette}`));
    } else {
        palette = null;
        setLook(row.look, keep);
        if (keep) say(tr(`theme: ${LOOKS[row.look][1]}`, `配色: ${LOOKS[row.look][1]}`));
    }
    // Move the ● with the choice. It used to be drawn once, when the list
    // opened, and then sat on whatever had been chosen before — pointing at
    // the wrong row of a list whose whole point is which row is on.
    if (report.on) {
        for (const r of report.rows) {
            r.n = (r.palette ? palette === r.palette : !palette && r.look === look) ? '●' : '';
        }
        drawReport();
    }
}

async function cmdTheme(name) {
    if (name) {
        const want = name.toLowerCase();
        if (palettes.has(want)) { setPalette(want); say(tr(`theme: ${want}`, `配色: ${want}`)); return; }
        const at = LOOKS.findIndex(([v, label]) =>
            v === name || label === name || (v || 'hakuji').startsWith(want));
        if (at >= 0) { setLook(at); say(tr(`theme: ${LOOKS[at][1]}`, `配色: ${LOOKS[at][1]}`)); return; }
        // Named, not "no such theme": what was typed is the one thing the
        // person knows about, so the near misses are worth more than the
        // refusal.
        const near = [...palettes.keys()].filter((k) => k.includes(want)).slice(0, 4);
        say(near.length ? `${name}? — ${near.join('  ')}` : tr(`there is no theme called ${name}`, `${name} という配色はありません`), true);
        return;
    }
    // What was on before the gallery opened, so Esc can put it back. The
    // foot says "Esc 戻す" and a promise on the screen has to be kept.
    const was = palette ? { palette } : { look };
    const rows = themeRows();
    show(tr('Themes', '配色'), tr(`${rows.length} — the top ${LOOKS.length} are this window's, the rest are cian-tui's`, `${rows.length} 種 — 上の ${LOOKS.length} つは窓のもの、あとは cian-tui のもの`),
        rows, {
            filter: true,
            hint: tr('type to narrow (dracula, light, …)', '打って絞り込み（dracula, light, …）'),
            foot: tr('type to narrow   ↑↓ dresses the window as you pass   Enter keep   Esc put it back', '打って絞る   ↑↓ 選ぶだけで着せ替わります   Enter 決定   Esc 戻す'),
            // Live, as the terminal build's gallery is: a palette is a thing
            // you look at, and choosing one from a list of names without
            // seeing it is choosing by memory.
            move: (row) => pickTheme(row, false),
            pick: (row) => { closeReport(); pickTheme(row); },
            leave: () => pickTheme(was, false),
        });
}

/// `背景色` — one pane's ground, from the fourteen cian-core publishes.
///
/// A menu rather than the report, because the report is the whole window and a
/// colour you cannot see while choosing it is a colour chosen by name. The
/// sheet leaves the panes visible, so `↑↓` really does dress the pane.
function cmdPaneGround() {
    const which = state.focus;
    const was = paneSkin[which].ground;
    const wear = (row) => { paneSkin[which].ground = row.color; paintPane(which); };
    const rows = grounds.map((g) => ({
        label: g.name,
        value: (g.color || null) === was ? '●' : '',
        color: g.color || null,
        run: () => { closeMenuChosen(); say(tr(`ground: ${g.name}`, `背景色: ${g.name}`)); },
    }));
    const at = Math.max(0, rows.findIndex((r) => r.color === was));
    openMenu({
        key: '',
        foot: tr(`the ${which === 'left' ? 'left' : 'right'} pane   ↑↓ dresses it as you pass   Enter keep   Esc put it back`, `${which === 'left' ? '左' : '右'}のペイン   ↑↓ 選ぶだけで着きます   Enter 決定   Esc 戻す`),
        stay: false,
        rows: () => rows,
        at: () => at,
        move: wear,
        leave: () => { paneSkin[which].ground = was; paintPane(which); },
    });
    // The cursor starts on the current colour, so nothing changes until it moves.
}

/// `テーマ（このペイン）` — a palette for one listing only.
///
/// cian-tui's `ThemePickPane`, including the way out: the first row clears the
/// override rather than being another palette, so the way back is in the same
/// list as the way in.
function cmdPaneTheme() {
    const which = state.focus;
    const was = paneSkin[which].theme;
    const wear = (row) => { paneSkin[which].theme = row.palette || null; paintPane(which); };
    const rows = [{
        label: tr('clear (back to the window’s theme)', '解除（窓ぜんたいの配色に戻す）'),
        value: was ? '' : '●',
        palette: null,
        run: () => { closeMenuChosen(); say(tr('this pane’s theme cleared', 'このペインの配色を解除しました')); },
    }];
    for (const name of palettes.keys()) {
        rows.push({
            label: name,
            value: was === name ? '●' : '',
            palette: name,
            run: () => { closeMenuChosen(); say(tr(`this pane’s theme: ${name}`, `このペインの配色: ${name}`)); },
        });
    }
    const at = Math.max(0, rows.findIndex((r) => r.palette === (was || null)));
    openMenu({
        key: '',
        foot: tr(`the ${which === 'left' ? 'left' : 'right'} pane only   ↑↓ dresses it as you pass   Enter keep   Esc put it back`, `${which === 'left' ? '左' : '右'}のペインだけ   ↑↓ 選ぶだけで着せ替わります   Enter 決定   Esc 戻す`),
        stay: false,
        rows: () => rows,
        at: () => at,
        move: wear,
        leave: () => { paneSkin[which].theme = was; paintPane(which); },
    });
}

/// `:blame` — who last changed each line, in the gutter.
///
/// In the gutter rather than as a list, because the question is always about a
/// *particular* line: "when did this become like this". A separate window with

/// 表示の桁で切って詰める。**字数ではなく桁数。**
///
/// `padEnd` は UTF-16 の単位で数えるので、全角の名前は詰まりが足りない ──
/// `:blame` の欄は1行ごとに幅が決まるので、日本語のコミッタ名の行だけ
/// コードが右へずれて、欄がぎざぎざになる。全角の判定は Unicode の
/// East Asian Width（W と F）で、絵文字もそこに入る。
///
/// 端末版が `unicode-width` でやっていることの、この窓で要る分だけ。
function cellWidth(s) {
    let n = 0;
    for (const ch of s) {
        const c = ch.codePointAt(0);
        n += (c >= 0x1100 && (
            c <= 0x115f
            || c === 0x2329 || c === 0x232a
            || (c >= 0x2e80 && c <= 0xa4cf && c !== 0x303f)
            || (c >= 0xac00 && c <= 0xd7a3)
            || (c >= 0xf900 && c <= 0xfaff)
            || (c >= 0xfe30 && c <= 0xfe6f)
            || (c >= 0xff00 && c <= 0xff60)
            || (c >= 0xffe0 && c <= 0xffe6)
            || (c >= 0x1f300 && c <= 0x1f64f)
            || (c >= 0x1f900 && c <= 0x1f9ff)
            || (c >= 0x20000 && c <= 0x3fffd)
        )) ? 2 : 1;
    }
    return n;
}

/// `cellWidth` で `cells` 桁ちょうどにする。
///
/// **切るのも桁で。** 全角の途中で切ると1桁足りない箱になるので、入らない
/// ときは1桁ぶんの空白で埋めて幅を合わせる。
function padCells(s, cells) {
    let out = '';
    let w = 0;
    for (const ch of s) {
        const cw = cellWidth(ch);
        if (w + cw > cells) break;
        out += ch;
        w += cw;
    }
    return out + ' '.repeat(cells - w);
}

/// the same line numbers is the same information at arm's length.
let blameOn = false;
let blameMarks = [];

async function cmdBlame() {
    if (!needViewer()) return;
    if (blameOn) {
        blameMarks = viewer.ed.deltaDecorations(blameMarks, []);
        blameOn = false;
        say(tr('blame hidden', 'blame を消しました'));
        return;
    }
    const r = await ask('blame', { pane: state.focus });
    if (!r) return;
    // One decoration per line, each carrying its own text: Monaco draws the
    // gutter, so the width sorts itself out and the code does not move.
    blameMarks = viewer.ed.deltaDecorations(blameMarks, r.lines.map((b, i) => ({
        range: new (window.monaco.Range)(i + 1, 1, i + 1, 1),
        options: {
            isWholeLine: true,
            before: {
                content: padCells(`${b.date} ${b.author}`, 22),
                inlineClassName: 'blame',
            },
        },
    })));
    blameOn = true;
    say(tr(`blame for ${r.lines.length} lines (:blame again hides it)`, `${r.lines.length} 行の blame（もう一度 :blame で消えます）`));
}

/// `:enc` — read the open file again in another encoding.
///
/// The bytes are already in the engine, so this decodes rather than re-reads.
/// That matters for a log something is still writing to: re-reading would show
/// a different file than the one being looked at.
async function cmdEncoding(name) {
    if (!needViewer()) return;
    const r = await ask('encoding', { as: name || undefined });
    if (!r) return;
    const model = viewer.ed.getModel();
    model.applyEdits([{ range: model.getFullModelRange(), text: r.lines.join('\n') }]);
    viewer.base = model.getAlternativeVersionId();
    viewer.dirty = false;
    const pretty = { Utf8: 'UTF-8', ShiftJis: 'Shift_JIS', Utf16Le: 'UTF-16LE', Utf16Be: 'UTF-16BE' };
    el.vAbout.textContent = `${pretty[r.encoding] || r.encoding}  ·  ${r.eol.toUpperCase()}`
        + tr(`  ·  ${r.lines.length} lines`, `  ·  ${r.lines.length} 行`);
    say(tr(`encoding: ${pretty[r.encoding] || r.encoding}`, `文字コード: ${pretty[r.encoding] || r.encoding}`));
}

/// The Markdown preview — the rendered document instead of the source.
///
/// **The one thing this build can do that the terminal cannot.** A terminal
/// draws Markdown in one face at one size; a window can set it, and a document
/// that is set properly is read faster than the same words in a grid.
///
/// The HTML comes from the engine, from the same parse cian-tui draws — so the
/// two never disagree about the program's own README — and it arrives already
/// escaped. A preview that runs what it finds is a preview that runs whatever
/// was in the repository somebody cloned.
let reading = false;

/// mermaid, loaded the first time a diagram appears — most Markdown has none,
/// and 3.4 MB is not a toll every preview should pay.
let mermaidLoading = null;

function loadMermaid() {
    if (mermaidLoading) return mermaidLoading;
    mermaidLoading = new Promise((ok, no) => {
        // The same trap monaco-vim fell into: a UMD bundle sees Monaco's AMD
        // `define` and takes the AMD branch, which cannot work here. With
        // `define` out of sight for the length of the load it lands on the
        // plain global instead.
        const savedDefine = window.define;
        window.define = undefined;
        const sc = document.createElement('script');
        sc.src = 'vendor/mermaid.js';
        sc.onload = () => {
            window.define = savedDefine;
            // strict: a README is a file from somewhere, and a diagram that
            // can run script is not a diagram.
            window.mermaid.initialize({ startOnLoad: false, securityLevel: 'strict' });
            ok(window.mermaid);
        };
        sc.onerror = () => {
            window.define = savedDefine;
            no(new Error(tr('vendor/mermaid.js is missing — run node gui/vendor.js', 'vendor/mermaid.js がありません — node gui/vendor.js')));
        };
        document.head.append(sc);
    });
    return mermaidLoading;
}

/// Draw every ```mermaid fence in the preview.
///
/// The terminal build folds these into an arrow list, because a terminal
/// cannot draw; this is the drawing. A diagram that fails to parse keeps its
/// source on screen with the reason — a blank where a diagram should be says
/// nothing, and the source at least says what was meant.
let mermaidSeq = 0;

async function drawDiagrams() {
    const fences = [...el.vRead.querySelectorAll('code.language-mermaid')];
    if (!fences.length) return;
    let mermaid;
    try {
        mermaid = await loadMermaid();
    } catch (e) { say(e.message, true); return; }
    const dark = isDark();
    // mermaid ships trebuchet, which has no Japanese and falls back silently
    // to whatever the system picks. Hand it the page's own body face so a
    // diagram's labels are set in the same type as the prose around them.
    const body = getComputedStyle(el.vRead).fontFamily;
    mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'strict',
        theme: dark ? 'dark' : 'default',
        fontFamily: body,
        themeVariables: { fontFamily: body },
    });
    for (const code of fences) {
        const src = code.textContent;
        const pre = code.parentElement;
        try {
            mermaidSeq += 1;
            const { svg } = await mermaid.render(`cian-mermaid-${mermaidSeq}`, src);
            const box = document.createElement('div');
            box.className = 'diagram';
            box.innerHTML = svg;
            pre.replaceWith(box);
        } catch (e) {
            const why = document.createElement('div');
            why.className = 'diagram-error';
            why.textContent = tr(`could not read it as a diagram: ${String(e.message || e).split('\n')[0]}`, `図として読めませんでした: ${String(e.message || e).split('\n')[0]}`);
            pre.before(why);
        }
    }
}

/// The diagrams, drawn **inside the editor**, under the fence they come from.
///
/// Taketan asked for this and it is the thing a window can do that a terminal
/// cannot even approximate: a view zone is a strip of real DOM that Monaco
/// keeps parked at a line, so the source stays exactly where it was — still
/// editable, still searchable, still vim — with the picture sitting under the
/// ```mermaid fence that produced it.
///
/// The terminal build's answer to "show me the diagram" is to leave: a browser
/// (`:mermaid`) or a rendered document instead of the source. Both make you
/// choose between reading the diagram and editing the text that makes it.
const zones = { ids: [], nodes: [], on: false, seq: 0 };

/// Where each ```mermaid fence starts and ends, and what is between them.
///
/// The same fence rule `cian_core::mermaid::extract_blocks` uses — ``` or ~~~
/// with `mermaid` after it — but it has to be done here as well as there,
/// because the zones need the *line numbers* and the engine's extractor
/// returns only the text. Kept next to that fact so the two stay together.
function mermaidFences(lines) {
    const out = [];
    for (let i = 0; i < lines.length; i++) {
        const t = lines[i].trimStart();
        const open = (t.startsWith('```') || t.startsWith('~~~'))
            && t.replace(/^[`~]+/, '').trim().toLowerCase() === 'mermaid';
        if (!open) continue;
        let j = i + 1;
        const body = [];
        while (j < lines.length
               && !(lines[j].trimStart().startsWith('```') || lines[j].trimStart().startsWith('~~~'))) {
            body.push(lines[j]);
            j++;
        }
        if (body.join('\n').trim()) out.push({ end: Math.min(j + 1, lines.length), src: body.join('\n') });
        i = j;
    }
    return out;
}

function clearDiagramZones() {
    if (!viewer.ed || !zones.ids.length) return;
    viewer.ed.changeViewZones((acc) => {
        for (const id of zones.ids) acc.removeZone(id);
    });
    zones.ids = [];
    zones.nodes = [];
}

/// Draw (or redraw) every fence's diagram as a zone under it.
async function drawDiagramZones() {
    if (!viewer.ed) return;
    clearDiagramZones();
    const lines = viewer.ed.getModel().getValue().split(/\r?\n/);
    const fences = mermaidFences(lines);
    if (!fences.length) return 0;
    let mermaid;
    try {
        mermaid = await loadMermaid();
    } catch (e) { say(e.message, true); return 0; }
    const body = getComputedStyle(document.body).fontFamily;
    mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'strict',
        theme: isDark() ? 'dark' : 'default',
        fontFamily: body,
        themeVariables: { fontFamily: body },
    });
    const drawn = [];
    for (const f of fences) {
        const node = document.createElement('div');
        node.className = 'zone-diagram';
        try {
            zones.seq += 1;
            const { svg } = await mermaid.render(`cian-zone-${zones.seq}`, f.src);
            node.innerHTML = svg;
        } catch (e) {
            // The source is already on screen right above, so a failed
            // diagram says why rather than leaving a gap.
            node.textContent = tr(`could not read it as a diagram: ${String(e.message || e).split('\n')[0]}`, `図として読めませんでした: ${String(e.message || e).split('\n')[0]}`);
            node.classList.add('bad');
        }
        drawn.push({ line: f.end, node });
    }
    // A zone's height is given to Monaco in pixels, and a diagram's height is
    // whatever mermaid decided at the width it ends up with. Measuring a copy
    // off screen gets the wrong number — the copy lays out in a box with no
    // width — so the zone goes in at a guess and is measured **in place**,
    // then told its real height. The first version clipped every diagram.
    const specs = [];
    viewer.ed.changeViewZones((acc) => {
        for (const d of drawn) {
            const spec = { afterLineNumber: d.line, heightInPx: 200, domNode: d.node };
            specs.push(spec);
            zones.nodes.push(d.node);
            zones.ids.push(acc.addZone(spec));
        }
    });
    // One frame later the nodes have a width and the SVGs have scaled to it.
    await new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(ok)));
    let changed = false;
    for (let i = 0; i < specs.length; i++) {
        const inner = zones.nodes[i].firstElementChild;
        const h = Math.ceil((inner ? inner.getBoundingClientRect().height : 0) + 16);
        if (h > 20 && Math.abs(h - specs[i].heightInPx) > 1) {
            specs[i].heightInPx = h;
            changed = true;
        }
    }
    if (changed) {
        viewer.ed.changeViewZones((acc) => {
            for (const id of zones.ids) acc.layoutZone(id);
        });
    }
    return drawn.length;
}

/// `Ctrl+E` — the rendered document and the source, back and forth.
///
/// In the capture phase, and this is the whole reason it works: Monaco binds
/// Ctrl+E itself (it moves to the end of the line), and a listener on the
/// bubble never got a look. Pressed on a Markdown file it did nothing at all
/// except step the cursor six columns right, which is a hard thing to read as
/// "this key is taken".
document.addEventListener('keydown', (e) => {
    if (!viewer.on) return;
    if (e.key !== 'e' && e.key !== 'E') return;
    if (!mod(e) || e.altKey) return;
    e.stopPropagation();
    e.preventDefault();
    togglePreview2();
}, true);

/// Ctrl+C / Ctrl+X / Ctrl+V keep meaning the clipboard in vim style.
///
/// This is cian-tui's rule, stated in one line at the top of its grammar
/// (viewer.rs `viewer_vim_key`): if CONTROL or ALT is held, the vim grammar
/// declines the key and it falls through to the ordinary handlers. So in the
/// terminal build's vim style these three are still copy, cut and paste.
///
/// monaco-vim disagrees — it wants Ctrl+V for visual block, Ctrl+C for
/// "back to normal" and Ctrl+X for decrement — so in the window, vim style
/// silently had no clipboard while notepad style did. Reported from Windows,
/// 2026-08-31.
///
/// `stopPropagation` and *not* `preventDefault`: the point is to keep the
/// editor's own handler from seeing the chord while leaving the browser free
/// to perform the copy. Preventing the default would take the clipboard away
/// from both of them, which is the bug wearing a different hat.
document.addEventListener('keydown', (e) => {
    if (!viewer.on || !viewer.vim) return;
    if (!mod(e) || e.altKey || e.shiftKey) return;
    if (!/^[cxvCXV]$/.test(e.key)) return;
    e.stopPropagation();
}, true);

async function togglePreview2() {
    if (!needViewer()) return;
    if (reading) {
        reading = false;
        el.vRead.hidden = true;
        el.vBody.hidden = false;
        viewer.ed.focus();
        // Back to the source — with the diagrams still on screen, parked
        // under the fences that make them. Asked for by name: the point of a
        // window is not having to choose between reading the picture and
        // editing the text.
        const n = await drawDiagramZones();
        zones.on = !!n;
        say(n ? tr(`back to the source — the ${n} diagram(s) stay on screen`, `ソースに戻りました — 図 ${n} 件はそのまま出しています`) : tr('back to the source', 'ソースに戻りました'));
        return;
    }
    const r = await ask('markdown', { lines: viewer.ed.getValue().split(/\r?\n/) });
    if (!r) return;
    // `innerHTML` on purpose, and only here: the engine escaped every piece of
    // text on the way out, and the markup is its own — not the file's.
    el.vRead.innerHTML = r.html;
    // The checkboxes are pressable here, the same as on the phone. Reading a
    // list is not a separate activity from crossing things off it, and going
    // back to the source to type an `x` between two brackets is not that.
    //
    // The line number came from the engine (`data-line`), so nothing here has
    // to work out which box this is — and it stays right when the note has a
    // front matter above it, which counting boxes would not.
    for (const box of el.vRead.querySelectorAll('.box[data-line]')) {
        box.classList.add('press');
        box.addEventListener('click', async () => {
            const line = Number(box.dataset.line);
            if (!Number.isInteger(line)) return;
            const done = box.textContent !== '☑';
            const out = await ask('check', {
                lines: viewer.ed.getValue().split(/\r?\n/), line, done,
            });
            if (!out) return;
            // Through the editor, so ⌘Z takes it back and the save is the
            // ordinary one — with its conflict check.
            viewer.ed.setValue(out.lines.join('\n'));
            await saveFile();
            await togglePreview2();
            await togglePreview2();
        });
    }
    // Links go to the desktop's browser rather than replacing the preview.
    // A file manager that navigates away from itself is a file manager you
    // have to restart.
    for (const a2 of el.vRead.querySelectorAll('a[href]')) {
        a2.addEventListener('click', (e) => {
            e.preventDefault();
            followMarkdownLink(a2.getAttribute('href'));
        });
    }
    reading = true;
    // The zones belong to the editor, and the editor is about to be hidden.
    clearDiagramZones();
    zones.on = false;
    el.vBody.hidden = true;
    el.vRead.hidden = false;
    el.vRead.scrollTop = 0;
    drawDiagrams();
    say(tr('preview — Ctrl+E goes back to the source', 'プレビュー — Ctrl+E でソースに戻ります'));
}

/// `:ws` — the characters you cannot see but a compiler can.
///
/// A trailing space, a tab where spaces were meant, an ideographic space that
/// arrived from a Japanese editor and looks exactly like a normal one. All
/// three break things silently, which is why showing them is a mode rather
/// than a hunt.
let wsOn = false;
function toggleWs() {
    if (!viewer.ed) { say(tr('open a file first', '先にファイルを開いてください'), true); return; }
    wsOn = !wsOn;
    viewer.ed.updateOptions({
        renderWhitespace: wsOn ? 'all' : 'selection',
        renderControlCharacters: wsOn,
        // The ideographic space is not whitespace to Monaco, so it needs the
        // unicode highlighter to be pointed at it — and it is the one of the
        // three that a person cannot spot by eye at all.
        unicodeHighlight: { ambiguousCharacters: wsOn, invisibleCharacters: wsOn },
    });
    say(wsOn ? tr('invisible characters shown', '見えない文字を表示') : tr('invisible characters hidden', '見えない文字を隠しました'));
}

let rulerOn = false;
function toggleRuler() {
    if (!viewer.ed) { say(tr('open a file first', '先にファイルを開いてください'), true); return; }
    rulerOn = !rulerOn;
    viewer.ed.updateOptions({ rulers: rulerOn ? [80, 100, 120] : [] });
    say(rulerOn ? tr('column ruler: 80 / 100 / 120', '桁の目盛り: 80 / 100 / 120') : tr('ruler hidden', '目盛りを消しました'));
}

/// `:s/old/new/g` — the same substitution language as the grep-wide replace,
/// because it is the same question asked of one file instead of many.
async function cmdSubstitute(spec) {
    await rewriteBuffer('substitute', { spec }, (r) => tr(`replaced ${r.changed}`, `${r.changed} 箇所を置換しました`));
}

/// The replace plan, with each line kept or dropped one at a time.
///
/// **Everything starts checked.** The common case is "yes, all of them", and
/// unchecking the exceptions is less work than checking the rest — which is
/// the terminal build's reasoning and it is right. Space unchecks; the count
/// on the header says how many are still going.
function showReplacePlan(spec, plan) {
    // On `show`'s own tick boxes rather than a second set kept beside it —
    // this screen had the only hand-rolled ones, and four review screens
    // needed the same thing.
    show(tr(`Replace ${spec}`, `置換 ${spec}`),
        tr(`${new Set(plan.changes.map((c) => c.path)).size} files`, `${new Set(plan.changes.map((c) => c.path)).size} ファイル`)
        + (plan.skipped.length ? tr(`   ${plan.skipped.length} skipped`, `   飛ばした ${plan.skipped.length} 件`) : ''),
        plan.changes.map((c) => ({
            n: String(c.line + 1),
            label: c.path.split(/[\\/]/).pop() + '  ' + c.before,
            sub: c.after,
            change: c,
        })),
        {
            checks: true,
            foot: tr('Space off/on   a all   n none   f the rest of this file   Enter run   Esc cancel',
                'Space 外す／戻す   a 全部   n 全部外す   f このファイルの残り   Enter 実行   Esc 取消'),
                // `f` — off with the whole file under the cursor, or back on
                // if none of it is picked. cian-tui's key on this screen
                // (keys.rs:1295), and the usual shape of "not this one, it is
                // generated": a hundred hits across twelve files, and one of
                // the twelve is a lock file.
                act: {
                    f: () => {
                        const row = report.rows[report.at];
                        if (!row || !row.change) return;
                        const path = row.change.path;
                        const mine = report.all.filter((r) => r.change && r.change.path === path);
                        const anyOn = mine.some((r) => r.on);
                        for (const r of mine) r.on = !anyOn;
                        drawReport();
                        drawCheckCount();
                        say(tr(`${path.split(/[\\/]/).pop()}: ${anyOn ? 'off' : 'on'}`,
                            `${path.split(/[\\/]/).pop()}: ${anyOn ? '外しました' : '戻しました'}`));
                    },
                },
                pick: async (chosen) => {
                    const going = chosen.map((r) => r.change);
                    if (!going.length) { say(tr('no row is chosen', '選ばれている行がありません'), true); return; }
                    closeReport();
                    if (!await confirm(tr(`Replace ${going.length} lines`, `${going.length} 行を置換します`),
                        tr(`${new Set(going.map((c) => c.path)).size} files — u cannot undo this`, `${new Set(going.map((c) => c.path)).size} ファイル — u では戻せません`))) {
                        say(tr('stopped', 'やめました'));
                        return;
                    }
                    const done = await ask('replaceapply', { changes: going });
                    if (!done) return;
                    await reread();
                    const bits = [tr(`replaced ${done.lines} lines in ${done.files} files`, `${done.files} ファイル ${done.lines} 行を置換`)];
                    if (done.stale) bits.push(tr(`${done.stale} lines had changed and were left alone`, `${done.stale} 行は変わっていたので触らず`));
                    say(bits.join('   '), done.errors.length > 0);
                },
        });
}

/// `:g/re/d` and `:v/re/d` — drop or keep every matching line.
///
/// The one line operation that filters rather than transforms, and the one
/// people reach for on a log: "everything except the heartbeats", once.
async function cmdLineFilter(pattern, keep) {
    if (!needViewer()) return;
    const lines = viewer.ed.getValue().split(/\r?\n/);
    const r = await ask('grepdel', { lines, pattern, keep });
    if (!r) return;
    replaceAll(r.lines);
    say(keep ? tr(`kept only the matching lines; dropped ${r.removed}`, `${r.removed} 行を落として、一致した行だけ残しました`) : tr(`deleted ${r.removed} lines`, `${r.removed} 行を削除しました`));
}

/// `:combine` — join the next line up, with a space or without.
async function cmdCombine(spec) {
    if (!needViewer()) return;
    const bang = /!$/.test(spec || '');
    const count = Math.max(2, Number((spec || '').replace('!', '').trim()) || 2);
    const lines = viewer.ed.getValue().split(/\r?\n/);
    const at = viewer.ed.getPosition().lineNumber - 1;
    const r = await ask('combine', { lines, at, count, space: !bang });
    if (!r) return;
    replaceAll(r.lines);
    viewer.ed.setPosition({ lineNumber: at + 1, column: 1 });
    say(tr(`joined ${r.joined} lines`, `${r.joined} 行を連結しました`));
}

/// Put a whole new set of lines in, through the edit stack so `u` takes it
/// back. Every line operation ends here, which is why it is one function.
function replaceAll(lines) {
    const model = viewer.ed.getModel();
    viewer.ed.executeEdits('cian', [{
        range: model.getFullModelRange(),
        text: lines.join('\n'),
    }]);
    viewer.ed.pushUndoStop();
}

/// `Ctrl+Q` / `Alt+V` — a rectangle, and what can be done to one.
///
/// Monaco has rectangular *selection*; what it does not have is vim's verbs
/// for it — `I` and `A` put text down the left or right edge of every line at
/// once, which is the whole reason anybody selects a rectangle. Columns are
/// display columns, so a line with a tab in it lines up the way it looks.
async function blockEdit(what) {
    if (!needViewer()) return;
    const sels = viewer.ed.getSelections() || [];
    if (!sels.length) { say(tr('there is no rectangle selected', '矩形選択がありません'), true); return; }
    const top = Math.min(...sels.map((s) => s.startLineNumber)) - 1;
    const bottom = Math.max(...sels.map((s) => s.endLineNumber)) - 1;
    const left = Math.min(...sels.map((s) => Math.min(s.startColumn, s.endColumn))) - 1;
    const right = Math.max(...sels.map((s) => Math.max(s.startColumn, s.endColumn))) - 1;
    let text = '';
    if (what !== 'delete') {
        text = await askFor(
            { insert: tr('text for the left edge', '左端に入れる文字'), append: tr('text for the right edge', '右端に足す文字'), replace: tr('text to replace it with', '置き換える文字') }[what],
            '',
        );
        if (text === null) return;
    }
    const lines = viewer.ed.getValue().split(/\r?\n/);
    const r = await ask('block', { lines, what, top, bottom, left, right, text });
    if (!r) return;
    replaceAll(r.lines);
    say({ delete: tr('rectangle deleted', '矩形を削除'), insert: tr('inserted at the left edge', '左端に挿入'), append: tr('added at the right edge', '右端に追加'), replace: tr('rectangle replaced', '矩形を置換') }[what]);
}

async function cmdDf() {
    const r = await ask('df', { pane: state.focus });
    if (!r) return;
    const pct = r.total ? Math.round((r.used / r.total) * 100) : 0;
    show(tr('Disk space', 'ディスクの空き'), r.where, [
        { n: human(r.total), label: tr('Total', '全体') },
        { n: human(r.used), label: tr('Used', '使用中'), sub: `${pct}%` },
        { n: human(r.available), label: tr('Free', '空き') },
    ], { foot: tr('Esc close', 'Esc 閉じる') });
}

/// `:head` / `:tail` — the ends of the file, without opening it. What a
/// log asks for first: the tail says what is happening, the head says when
/// it started.
async function cmdPeek(args, tail) {
    const n = Number(((args || '').match(/-n\s*(\d+)/) || [, 10])[1]) || 10;
    const r = await ask('peek', { pane: state.focus, n, tail });
    if (!r) return;
    show(`${tail ? 'tail' : 'head'} -n ${n}  ${r.name}`, tr(`${r.rows.length} lines`, `${r.rows.length} 行`),
        r.rows.map((t, i2) => ({ n: String(tail ? '' : i2 + 1), label: t })),
        { foot: tr('Esc close', 'Esc 閉じる') });
}

/// `:recent` — the files this session has opened, newest first.
const recentFiles = [];

function noteRecent(path, name) {
    const at = recentFiles.findIndex((x) => x.path === path);
    if (at >= 0) recentFiles.splice(at, 1);
    recentFiles.unshift({ path, name });
    if (recentFiles.length > 40) recentFiles.pop();
}

async function cmdRecent() {
    if (!recentFiles.length) { say(tr('nothing has been opened yet', 'まだ何も開いていません')); return; }
    show(tr('Recently opened', '最近開いたファイル'), tr(`${recentFiles.length} files`, `${recentFiles.length} 件`),
        recentFiles.map((x) => ({ label: x.name, sub: x.path, path: x.path })),
        {
            foot: tr('Enter go there   Esc close', 'Enter そこへ   Esc 閉じる'),
            pick: (row) => { closeReport(); revealPath(row.path, false); },
        });
}

async function cmdVersion() {
    const w = await ask('where', {}) || {};
    // When this engine was built, and from what. The version number is the
    // same on every build this year by design, so on its own it cannot answer
    // the only question anybody opens this to ask: am I running the thing I
    // just downloaded? Local time, because that is the clock beside the person
    // reading it.
    const built = w.built_at
        ? new Date(w.built_at * 1000).toLocaleString('ja-JP',
            { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
        : tr('(unknown)', '(不明)');
    show('cian', tr(`${w.version || '1.1.10'} — a window on cian-core`, `${w.version || '1.1.10'} — cian-core の上の窓`), [
        { label: tr('Built', 'ビルド日時'), sub: built + (w.commit ? `   ${w.commit}` : '') },
        { label: tr('Typeface', '書体'), sub: `${resolvedFace()}   ${FONT.at}px` },
        { label: tr('Config', '設定'), sub: w.config || tr('(none)', '(なし)') },
        { label: tr('Written to', '書き込み先'), sub: w.writes || tr('(none)', '(なし)') },
        { label: tr('Engine', 'エンジン'), sub: 'cian-server（JSON lines / stdio）' },
    ], { foot: tr('Esc close', 'Esc 閉じる') });
}

async function cmdWc() {
    const r = await ask('wc', { pane: state.focus });
    if (!r) return;
    if (!r.rows.length) { say(tr('nothing to count', '数えられるファイルがありません')); return; }
    const sum = r.rows.reduce((a, x) => ({
        lines: a.lines + x.lines, words: a.words + x.words, bytes: a.bytes + x.bytes,
    }), { lines: 0, words: 0, bytes: 0 });
    show(tr('Lines, words, bytes', '行・単語・バイト'),
        tr(`${r.rows.length} files   ${sum.lines.toLocaleString()} lines   `, `${r.rows.length} ファイル   ${sum.lines.toLocaleString()} 行   `)
        + tr(`${sum.words.toLocaleString()} words   ${human(sum.bytes)}`, `${sum.words.toLocaleString()} 語   ${human(sum.bytes)}`),
        r.rows.map((x) => ({
            n: x.lines.toLocaleString(),
            label: x.name,
            sub: tr(`${x.words.toLocaleString()} words   ${human(x.bytes)}`, `${x.words.toLocaleString()} 語   ${human(x.bytes)}`),
        })),
        { foot: tr('Esc close', 'Esc 閉じる') });
}

/// `:where` — which of the config files cian actually found.
///
/// The question exists because a copy beside the executable wins over the one
/// in the home directory, and that is not where anybody looks first. Editing
/// the wrong file and wondering why nothing changed is the failure this
/// answers.
async function cmdWhere() {
    const r = await ask('where', {});
    if (!r) return;
    show(tr('Where the config lives', '設定の場所'), tr('written to: ', '書き込み先: ') + (r.writes || tr('(unknown)', '(不明)')), [
        { label: 'init.lua', sub: r.config || tr('(none)', '(なし)') },
        { label: 'state.toml', sub: r.state || tr('(none)', '(なし)') },
        { label: 'shortcuts.lua', sub: r.shortcuts || tr('(none)', '(なし)') },
        { label: 'macro.lua', sub: r.macros || tr('(none)', '(なし)') },
    ], { foot: tr('Esc close', 'Esc 閉じる') });
}

async function cmdMarkGlob(glob, on) {
    const r = await ask('markglob', { pane: state.focus, glob, on });
    if (!r) return;
    state[state.focus] = r;
    draw(state.focus);
    say(tr(`${glob}: ${r.matched} ${on ? 'marked' : 'unmarked'}`, `${glob}: ${r.matched} 件を${on ? 'マーク' : '解除'}`));
}

/// `:copyto` / `:moveto` — somewhere that is not the other pane.
async function cmdTo(what, dest) {
    const r = await ask(what, { pane: state.focus, dest });
    if (!r) return;
    beginOp(r, r.kind, r.kind === 'move' ? tr('move', '移動') : tr('copy', 'コピー'));
    if (!r.queued) say(tr(`${r.count} → ${r.dest}`, `${r.count} 件を ${r.dest} へ`));
}

/// `:vi` / `:vim` / `:nvim` — the file, in that editor, in a new shell tab.
/// The tab is the terminal build's arrangement: the editor gets a real
/// terminal, and closing it brings you back to the files rather than to a
/// desktop window somewhere.
async function cmdEditorTab(_arg, invokedAs) {
    const pane = state[state.focus];
    const row = pane.entries[pane.cursor];
    if (!row || row.parent || row.is_dir) { say(tr('choose a file first', 'ファイルを選んでください'), true); return; }
    const editor = invokedAs && invokedAs !== 'vi' ? invokedAs : 'vi';
    if (!term.on) await openShell();
    const t = await ask('shelltab', { pane: state.focus, ...shellSize() });
    if (!t) return;
    takeShell(t);
    setShellFocus(true);
    await ask('run', { pane: state.focus, line: `${editor} %f` });
    say(tr(`opened in ${editor} (F10 closes the tab)`, `${editor} で開きました（F10 でタブごと閉じる）`));
}

async function cmdEditStyle(arg, invokedAs) {
    const want = (arg || (invokedAs === 'notepad' ? 'notepad' : '')).trim();
    const at = STYLES.findIndex(([v]) => v === want);
    if (at < 0) { say(tr(':editstyle vim, or :editstyle notepad', ':editstyle vim か :editstyle notepad'), true); return; }
    setStyle(at);
    say(tr(`editor: ${styleName(at)}`, `エディタ: ${styleName(at)}`));
}

/// `:scratch` — an empty buffer to think in. `:w` (or Ctrl+S) asks for a
/// name and it becomes a real file where you stand; closing it unsaved
/// costs nothing, which is the point of a scratchpad.
async function cmdScratch() {
    let monaco;
    try {
        monaco = await loadMonaco();
    } catch (e) { say(e.message, true); return; }
    if (viewer.on) await closeView(false);
    setViewerOn(true);
    scratch.on = true;
    viewer.name = tr('Scratch', '下書き');
    el.view.hidden = false;
    el.vBody.hidden = false;
    el.vPic.hidden = true;
    makeEditor(monaco, '', 'plaintext');
    viewer.base = viewer.ed.getModel().getAlternativeVersionId();
    viewer.dirty = false;
    setStyle(style);
    el.vName.textContent = tr('Scratch', '下書き');
    el.vAbout.textContent = tr('it exists nowhere until it is saved', '保存されるまでどこにもありません');
    el.vFoot.textContent = tr('Ctrl+S saves it under a name   ·   Esc ×3 throws it away', 'Ctrl+S 名前を付けて保存   ·   Esc ×3 捨てる');
    viewer.ed.focus();
}

const scratch = { on: false };

async function saveScratch() {
    const name = await askFor(tr('a name to save it as', '保存する名前'), 'scratch.txt');
    if (name === null || !name) return false;
    const r = await ask('writefile', {
        pane: state.focus, name,
        text: viewer.ed.getValue() + '\n',
    });
    if (!r) return false;
    scratch.on = false;
    viewer.dirty = false;
    await reread();
    say(tr(`saved to ${r.wrote}`, `${r.wrote} に保存しました`));
    await closeView(false);
    return true;
}

async function cmdLimit(spec) {
    const r = await ask('limit', { spec });
    if (!r) return;
    say(r.bps ? tr(`transfer cap: ${human(r.bps)}/s`, `転送の上限: ${human(r.bps)}/s`) : tr('transfer cap: none', '転送の上限: なし'));
}

/// `:aicommit` — the staged diff in, a Conventional Commits message out,
/// **shown, not committed**. Enter commits with it; Esc walks away. The
/// model drafts; the person signs.
async function cmdAiCommit() {
    const r = await ask('aicommit', { pane: state.focus });
    if (!r) return;
    say(tr('drafting a commit message…', 'コミットメッセージを作っています…'));
    aiWaiting = (answer) => showCommitDraft(answer.trim());
}

/// The drafted message, with a way to change it before it is signed.
///
/// **`e` edits it.** cian-tui's `CommitMessage` popup has a preview mode and a
/// typing mode on that key (keys.rs:1813), and it matters more than it looks:
/// the model drafts, and the person is the one whose name goes on the commit.
/// A draft you can only accept or throw away is a draft you throw away and
/// retype.
function showCommitDraft(msg) {
    const commit = async (text) => {
        closeReport();
        if (!await confirm(tr('Commit with this message', 'この文でコミットします'), text)) {
            say(tr('stopped', 'やめました'));
            return;
        }
        const done = await ask('commit', { pane: state.focus, message: text });
        if (!done) return;
        state[state.focus] = done.pane;
        draw(state.focus);
        say(tr('committed', 'コミットしました'));
    };
    show(tr('Commit message (a draft)', 'コミットメッセージ（案）'),
        tr('Enter commits it as it stands   e edits it   Esc cancels', 'Enter でこのままコミット   e で直す   Esc 取消'),
        msg.split('\n').map((t) => ({ label: t })), {
            foot: tr('Enter commit   e edit   Esc cancel', 'Enter コミット   e 直す   Esc 取消'),
            act: {
                e: async () => {
                    closeReport();
                    // One line in the box, the rest kept: a commit subject is
                    // the line that gets read, and it is the line worth fixing.
                    const first = msg.split('\n')[0];
                    const rest = msg.split('\n').slice(1).join('\n');
                    const edited = await askFor(tr('the commit message', 'コミットメッセージ'), first);
                    if (edited === null) { say(tr('stopped', 'やめました')); return; }
                    showCommitDraft(rest.trim() ? `${edited}\n${rest}` : edited);
                },
            },
            pick: () => commit(msg),
        });
}

/// The AI extension family. Everything here is metadata in, a *plan* out,
/// and nothing happens until the person says so on a list they can read —
/// which is the terminal build's arrangement and the only sane one for a
/// model with opinions about other people's files.

/// What to say when the walk behind an AI answer did not reach everything.
///
/// **A cap nobody is told about is a lie.** "明らかな不要ファイルは見つかり
/// ませんでした" about a tree two thirds of which was never opened reads as a
/// fact about the tree. The engine sends the two counts rather than a
/// sentence, so this side can say it in the reader's language — and the model
/// is told the same thing in English, in the prompt.
function scanShortfall(partial) {
    if (!partial) return '';
    const bits = [];
    if (partial.stopped) {
        // What *is* complete, not how much is missing. "42160 件が入りません
        // でした" was true over a Rust checkout, useless, and alarming; the
        // walk had in fact listed the top two levels in full.
        bits.push(partial.whole_to
            ? tr(`it stops ${partial.whole_to} level(s) down`, `${partial.whole_to} 階層下までで打ち切りました`)
            : tr('it stops partway through this directory', 'このディレクトリの途中で打ち切りました'));
    }
    if (partial.unopened > 0) {
        bits.push(tr(`${partial.unopened} directories were too deep`, `${partial.unopened} 個のディレクトリは深すぎて開いていません`));
    }
    if (!bits.length) return '';
    return tr(` — but ${bits.join('; ')}`, ` ── ただし${bits.join('・')}`);
}

async function cmdAiScan(what) {
    const r = await ask(what, { pane: state.focus });
    if (!r) return;
    say(what === 'aijunk' ? tr('looking for what might be junk…', '不要そうなものを探しています…') : tr('working out how to tidy it…', '畳み方を考えています…'));
    aiWaiting = async (payload) => {
        const rows = payload.rows || [];
        const short = scanShortfall(payload.partial);
        if (!rows.length) {
            say((what === 'aijunk' ? tr('nothing here is obviously junk', '明らかな不要ファイルは見つかりませんでした') : tr('it says this is already tidy', 'もう整っています、と言っています')) + short);
            return;
        }
        if (what === 'aijunk') {
            show(tr('Possibly junk', '不要かもしれないもの'), tr('the AI’s guess — check before acting', 'AI の見立てです。確かめてから') + short,
                rows.map((x) => ({ label: x.name, sub: x.reason || '', path: x.path })),
                {
                    checks: true,
                    foot: tr('Space off/on   a all   n none   Enter mark the chosen   Esc cancel', 'Space 外す／戻す   a 全部   n 全部外す   Enter 選んだ分をマーク   Esc 取消'),
                    pick: async (chosen) => {
                        if (!chosen.length) { say(tr('no row is chosen', '選ばれている行がありません'), true); return; }
                        closeReport();
                        const p = await ask('setmarks', {
                            pane: state.focus, paths: chosen.map((x) => x.path),
                        });
                        if (!p) return;
                        state[state.focus] = p;
                        draw(state.focus);
                        say(tr(`${chosen.length} marked — d deletes them (to the trash)`, `${chosen.length} 件をマークしました — d で削除（ゴミ箱へ）`));
                    },
                });
            return;
        }
        show(tr('A suggested folder structure', 'ディレクトリ構成の提案'), tr('it only moves things; nothing is deleted or renamed', '移すだけ。消しも改名もしません') + short,
            rows.map((x) => ({ n: '→ ' + x.folder, label: x.name, sub: x.reason || '', path: x.path, folder: x.folder })),
            {
                checks: true,
                foot: tr('Space off/on   a all   n none   Enter do it (u undoes)   Esc cancel', 'Space 外す／戻す   a 全部   n 全部外す   Enter 実行（u で戻せます）   Esc 取消'),
                pick: async (chosen) => {
                    if (!chosen.length) { say(tr('no row is chosen', '選ばれている行がありません'), true); return; }
                    closeReport();
                    if (!await confirm(tr(`Move ${chosen.length} into the folders below`, `${chosen.length} 件を下のディレクトリへ移します`),
                        chosen.map((x) => `${x.name} → ${x.folder}/`).join('\n'))) { say(tr('stopped', 'やめました')); return; }
                    const done = await ask('organizeapply', {
                        pane: state.focus,
                        rows: chosen.map((x) => ({ path: x.path, folder: x.folder })),
                    });
                    if (!done) return;
                    state[state.focus] = done.pane;
                    draw(state.focus);
                    if (done.errors.length) say(done.errors.join('  /  '), true);
                    else say(tr(`moved ${done.moved} (u undoes it)`, `${done.moved} 件を移しました（u で戻せます）`));
                },
            });
    };
}

async function cmdAiRename(instruction) {
    const r = await ask('airename', { pane: state.focus, instruction });
    if (!r) return;
    say(tr('working out new names…', 'リネーム案を考えています…'));
    aiWaiting = (payload) => {
        const rows = (payload.rows || []).filter((x) => x.new_name && !/[\\/]/.test(x.new_name));
        if (!rows.length) { say(tr('it proposed no changes', '変える案がありませんでした')); return; }
        // Through the same plan screen every bulk rename uses: clashes marked,
        // nothing moves until Enter.
        showRenamePlanRows(rows.map((x) => ({
            from: x.name, to: x.new_name, path: x.path,
            same: x.name === x.new_name, clash: false,
        })), tr('AI rename', 'AIリネーム'));
    };
}

/// The bulk-rename confirmation, callable with rows from anywhere — the
/// pattern rename builds them from a pattern, the AI from an instruction.
function showRenamePlanRows(rows, title) {
    const changing = rows.filter((x) => !x.same);
    if (!changing.length) { say(tr('no name would change', '変わる名前がありません')); return; }
    show(title, tr('choose them one row at a time', '一行ずつ選べます'),
        // A row whose name does not change starts unticked and stays that
        // way — there is nothing in it to do.
        rows.map((x) => ({
            n: x.same ? '=' : '→', label: x.from, sub: x.to,
            from: x.from, to: x.to, same: x.same, on: !x.same,
        })),
        {
            checks: true,
            foot: tr('Space off/on   a all   n none   Enter run   Esc cancel', 'Space 外す／戻す   a 全部   n 全部外す   Enter 実行   Esc 取消'),
            pick: async (chosen) => {
                const going = chosen.filter((x) => !x.same);
                if (!going.length) { say(tr('no row is chosen', '選ばれている行がありません'), true); return; }
                closeReport();
                if (!await confirm(tr(`Rename ${going.length}`, `${going.length} 件の名前を変えます`),
                    going.map((x) => `${x.from}  →  ${x.to}`).join('\n'))) { say(tr('stopped', 'やめました')); return; }
                const done = await ask('renameapply', { rows: going.map((x) => ({ from: x.from, to: x.to })) });
                if (!done) return;
                await reread();
                if (done.errors.length) say(done.errors.join('  /  '), true);
                else say(tr(`renamed ${done.renamed}`, `${done.renamed} 件の名前を変えました`));
            },
        });
}

async function cmdAiSearch(query) {
    const r = await ask('aisearch', { pane: state.focus, query });
    if (!r) return;
    say(tr('searching by meaning…', '意味で探しています…'));
    aiWaiting = (payload) => {
        const rows = payload.rows || [];
        const short = scanShortfall(payload.partial);
        if (!rows.length) { say(tr('nothing looks like it', 'それらしいものは見つかりませんでした') + short); return; }
        show(tr(`Things like “${query}”`, `「${query}」らしいもの`), tr(`${rows.length} — the AI’s guess`, `${rows.length} 件 — AI の見立てです`) + short,
            rows.map((x) => ({ label: x.path, sub: x.reason || '', full: x.full })),
            {
                foot: tr('Enter go there   Esc close', 'Enter そこへ   Esc 閉じる'),
                pick: (row) => { closeReport(); revealPath(row.full, false); },
            });
    };
}

/// The three switches cian-tui keeps in `T` and this build did not have at
/// all. Runtime-only in both, because they answer about *this session*: a
/// verify that had silently stayed on since last month would be a surprise
/// on the first big upload.
///
/// Defaults are cian-tui's: notify on, verify off, cloud reads off.
const switches = { notify: true, verify: false, cloud: false };

/// Say that a long job finished, when the person may have walked away.
///
/// cian-tui writes OSC 9 and lets the terminal decide; a window has the
/// desktop's own notifications, which is the same idea with a better answer.
/// Silent under `notify_min_secs`, because a notification for a job that took
/// two seconds is a notification you turn off.
/// `cian.set_option("notify_min_secs", n)` moves this; cian-tui's default is
/// five seconds and so is this.
let notifyAfterMs = 5000;

function notifyDone(ms, summary) {
    if (!switches.notify || ms < notifyAfterMs) return;
    try {
        // Control characters dropped, as the terminal build drops them — the
        // text comes from paths and error messages.
        const clean = String(summary).replace(/[\u0000-\u001f\u007f]/g, ' ').trim();
        new Notification('cian', { body: clean, silent: false });
    } catch { /* a desktop with no notifications is not an error */ }
}

/// `:ime` — the input method, put where this moment wants it.
///
/// The one thing a keyboard program cannot survive is an IME that stays on
/// where the keys are commands: `j` becomes か and nothing moves. When
/// cian.ime{…} names a helper, cian switches to the no-IME source wherever it
/// is being driven and puts back whatever was on wherever it is being typed
/// into. `:ime` toggles the herding and says what is configured.
const ime = { on: false, want: null, broken: false };

async function cmdIme() {
    const r = await ask('ime', {});
    if (!r) return;
    ime.on = !ime.on;
    ime.want = null;
    ime.broken = false;
    say(ime.on
        ? tr(`input method: on (now ${r.current || '?'}) — it comes back only where text is typed`, `IME 連携: オン（いま ${r.current || '?'}）— 文字を打つ所だけ IME が戻ります`)
        : tr('input method: off', 'IME 連携: オフ'));
    if (ime.on) syncIme();
}

/// A physical key, when the input method has swallowed the character.
///
/// **This is the one thing the window can do that the terminal genuinely
/// cannot.** With a Japanese IME on, a terminal holds every letter in
/// composition until it is committed, so `j` never reaches cian at all — the
/// note in the terminal build says as much, and both builds answer it the
/// same way: drive an external helper (`macism`, `im-select`, the bundled
/// `cian-ime`) to switch the IME *off* whenever keys become commands.
///
/// A browser does not swallow the key. It reports the keydown with
/// `isComposing` set and `key` as `Process`, and it still says **which
/// physical key** was pressed. So the window can read the key and leave the
/// person's input method exactly where they left it: no helper to install, no
/// `im-select` on Windows, and nothing switching under them mid-sentence.
///
/// Letters and digits only. `code` is a *physical* position, so punctuation
/// differs between a JIS and an ANSI keyboard and guessing it would put the
/// wrong character under somebody's finger; cian's commands are letters and
/// digits, and the rest keeps the old road.
function keyFromCode(e) {
    const c = e.code || '';
    if (/^Key[A-Z]$/.test(c)) {
        const ch = c.slice(3);
        return e.shiftKey ? ch : ch.toLowerCase();
    }
    if (/^Digit[0-9]$/.test(c) && !e.shiftKey) return c.slice(5);
    return null;
}

/// Marks the keydown we made ourselves, so it is not re-read as a composing
/// one and turned into another.
const RESENT = Symbol('cian-resent');

window.addEventListener('keydown', (e) => {
    // Only while keys are commands. In a text field the composition is the
    // point, and a listing key fired from under it would be a `d` that
    // deletes a file while somebody types a filename.
    //
    // **And never in the editor.** This road works because the listing has no
    // text field: `preventDefault` on the keydown is the whole of it. Monaco
    // *does* have one, and a composition already under way does not go
    // through keydown at all — it arrives as `compositionupdate` and `input`
    // straight into the textarea. So in vim's normal mode with the IME on,
    // this fired the physical letters as commands (`a`, `i` — both of which
    // enter insert mode) **and the IME then committed the Japanese into the
    // buffer it had just opened**. Reported as あいう appearing on the line
    // and the line below it, which is exactly two of those letters landing.
    //
    // Without this, normal mode with the IME on does nothing at all, which is
    // what vim in a terminal does. Getting the physical key *and* leaving the
    // IME on is not something a page can do — it needs the input source
    // switched off, which is `:ime` (see `syncIme`, and cian-ime.swift).
    if (e[RESENT] || wantsTextInput() || viewer.on) return;
    // 229 is what a browser reports for "the IME has this one"; `isComposing`
    // is the modern spelling and not every platform sets it on the first key.
    if (!e.isComposing && e.keyCode !== 229 && e.key !== 'Process') return;
    const key = keyFromCode(e);
    if (!key) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    const out = new KeyboardEvent('keydown', {
        key,
        code: e.code,
        shiftKey: e.shiftKey,
        ctrlKey: e.ctrlKey,
        altKey: e.altKey,
        metaKey: e.metaKey,
        bubbles: true,
        cancelable: true,
    });
    out[RESENT] = true;
    document.dispatchEvent(out);
}, true);

/// Is cian taking text right now, rather than being driven by commands?
///
/// The terminal build's whole rule, in this window's terms (ime.rs
/// `wants_text_input`): everything that reads a typed string says yes; the
/// file panes and a viewer being *read* say no. It used to be wired to
/// monaco-vim's mode changes alone, so the `:` line, the filter, every
/// askFor prompt, the finder and the shell were never herded at all — and in
/// notepad style, which is the default, nothing was.
function wantsTextInput() {
    // The viewer first, because Monaco holds the focus in a hidden textarea
    // whether or not it is taking text — the generic test below would say
    // "typing" in vim's normal mode, which is precisely the case this exists
    // to switch the IME *off* for.
    if (viewer.on && viewer.ed) return STYLES[style][0] === 'vim' ? vimTyping() : true;
    if (term.on && term.focused) return true;
    const at = document.activeElement;
    return !!at && (at.tagName === 'INPUT' || at.tagName === 'TEXTAREA' || at.isContentEditable);
}

/// Put the input method where this moment wants it.
///
/// Cheap to call — it compares one boolean and does nothing until the answer
/// changes — so it runs after every keystroke and every focus change rather
/// than being remembered at each of the dozen places that open a prompt.
function syncIme() {
    if (!ime.on || ime.broken) return;
    const want = wantsTextInput();
    if (ime.want === want) return;
    ime.want = want;
    window.cian.call('ime', { do: want ? 'restore' : 'off' }).catch((e) => {
        // Said once, and then left alone. A helper that is not there fails
        // on every switch, and a message per keystroke would bury the one
        // that matters.
        ime.broken = true;
        say(tr(`input-method handling stopped — ${e.message}`, `IME 連携を止めました — ${e.message}`), true);
    });
}

// Once per turn round the event loop, which is where the terminal build calls
// it. A capture listener always fires (an overlay's stopPropagation cannot
// reach it), and the microtask runs after the handlers have moved the state
// the answer depends on.
document.addEventListener('keydown', () => queueMicrotask(syncIme), true);
document.addEventListener('focusin', syncIme);
document.addEventListener('focusout', () => queueMicrotask(syncIme));

async function cmdAiError() {
    const r = await ask('aierror', {});
    if (!r) return;
    openChat(
        tr('The last error, explained', '直近のエラーの説明'),
        tr('Explain the last error', '直近のエラーを説明'),
    );
    answerIntoChat();
}

/// `:revealos` — hand the file to Finder, with it selected.
async function cmdRevealOs() {
    const r = await ask('revealos', { pane: state.focus });
    if (r) say(tr(`showed ${r.revealed} in ${osCan.file_manager}`, `${r.revealed} を ${osCan.file_manager} で表示しました`));
}

/// The OS "Open with…" picker (Windows only — the engine says so).
async function cmdOpenWith() {
    const r = await ask('openwith', { pane: state.focus });
    if (r) say(tr(`handed ${r.opened} to “Open with”`, `${r.opened} を「プログラムから開く」に渡しました`));
}

/// The OS properties / Get-Info panel.
async function cmdProperties() {
    const r = await ask('properties', { pane: state.focus });
    if (r) say(tr(`opened ${ON_MAC ? 'Get Info' : 'Properties'} for ${r.shown}`, `${r.shown} の${ON_MAC ? '情報' : 'プロパティ'}を開きました`));
}

async function cmdEditExternal() {
    const r = await ask('editexternal', { pane: state.focus });
    if (!r) return;
    say(tr(`opened ${r.name} in ${r.editor}`, `${r.name} を ${r.editor} で開きました`));
}

/// F12 — give the shell the window, or give it back.
///
/// Two thirds of the height is not enough to read a build's output and too
/// much to keep a listing usable; the answer everywhere else is a key that
/// swaps between them rather than a compromise that suits neither.
/// F12 — the surface the keys are in fills the window.
///
/// The terminal build's `toggle_zoom` (keys.rs:363) zooms *the focused
/// surface*: standing in a file pane, that pane; standing in the shell, the
/// shell. This only ever grew the shell, whichever pane you were in — so F12
/// from a listing made the thing you were not looking at bigger.
/// `F11` — fill the screen, or stop.
///
/// The neighbour of F12 and a different question: F12 gives one *surface* the
/// room inside the window, F11 gives the window the room on the screen. The
/// terminal build has neither, having already been handed a whole terminal.
async function cmdFullscreen() {
    try {
        const on = await window.cian.fullscreen();
        say(on ? tr('full screen (F11 comes back)', '全画面（F11 で戻る）') : tr('left full screen', '全画面をやめました'));
    } catch (e) {
        say(tr(`cannot go full screen: ${e.message}`, `全画面にできません: ${e.message}`), true);
        return;
    }
    // The window changed shape, so the shell's idea of its own size is stale —
    // the same reason zoomFocused ends this way.
    if (term.on) ask('shellresize', shellSize());
    measureFoot();
}

function zoomFocused() {
    const now = el.work.dataset.zoom;
    if (now) {
        el.work.dataset.zoom = '';
        say(tr('back', '戻しました'));
    } else if (term.on && term.focused) {
        el.work.dataset.zoom = 'shell';
        say(tr('the shell is zoomed (F12 comes back)', 'シェルを広げました（F12 で戻る）'));
    } else {
        el.work.dataset.zoom = 'files';
        say(tr(`the ${state.focus === 'left' ? 'left' : 'right'} pane is zoomed (F12 comes back)`, `${state.focus === 'left' ? '左' : '右'}ペインを広げました（F12 で戻る）`));
    }
    // Whatever just changed shape, the shell's idea of its own size is stale.
    if (term.on) ask('shellresize', shellSize());
    measureFoot();
}

/// The two dividers, moved by Ctrl+Shift+arrow.
///
/// `main` is the share given to the *files* and `panes` the share given to
/// the left pane, which is how the terminal build holds them (`main_pct`,
/// `panes_pct`). The help has listed this key since the beginning and the
/// listing had no handler for it at all — only the shell's inner splits did.
const layout = { main: 75, panes: 50 };
const MIN_PCT = 15;
const STEP_PCT = 4;

function applyLayout(remember = true) {
    const clamp = (v) => Math.max(MIN_PCT, Math.min(100 - MIN_PCT, v));
    layout.main = clamp(layout.main);
    layout.panes = clamp(layout.panes);
    const r = document.documentElement.style;
    r.setProperty('--main-pct', `${layout.main}%`);
    r.setProperty('--panes-pct', `${layout.panes}%`);
    if (term.on) ask('shellresize', shellSize());
    // Measured after the boxes have moved, not from the percentages.
    requestAnimationFrame(placeGrips);
    if (remember) {
        ask('remember', { key: 'gui_main_pct', value: String(Math.round(layout.main)) });
        ask('remember', { key: 'gui_panes_pct', value: String(Math.round(layout.panes)) });
    }
}

/// Put the two grips on the seams they move.
///
/// Measured from the real boxes rather than computed from the percentages:
/// the seam between the panes is a 1px flex gap and the one above the shell
/// is a border, and neither is at exactly the fraction — the borders, the
/// gap and the foot bars all move it.
function placeGrips() {
    const panes = el.panes.getBoundingClientRect();
    const left = el.left.getBoundingClientRect();
    el.gripPanes.style.top = `${panes.top}px`;
    el.gripPanes.style.height = `${panes.height}px`;
    el.gripPanes.style.left = `${left.right - 4}px`;
    el.gripPanes.hidden = el.panes.classList.contains('one');
    const shellShown = term.on && !el.shell.hidden && !el.work.dataset.zoom;
    el.gripMain.hidden = !shellShown;
    if (shellShown) {
        el.gripMain.style.top = `${el.shell.getBoundingClientRect().top - 4}px`;
    }
}

/// Drag a divider. The percentage follows the pointer directly — a divider
/// that moves by a step per drag is a scrollbar, not a divider.
function startGripDrag(which, e) {
    e.preventDefault();
    document.body.classList.add('dragging');
    const work = el.work.getBoundingClientRect();
    const move = (ev) => {
        if (which === 'panes') {
            layout.panes = ((ev.clientX - work.left) / work.width) * 100;
        } else {
            layout.main = ((ev.clientY - work.top) / work.height) * 100;
        }
        applyLayout(false);
        placeGrips();
    };
    const done = () => {
        document.removeEventListener('mousemove', move);
        document.removeEventListener('mouseup', done);
        document.body.classList.remove('dragging');
        // Written down once, at the end — not on every pixel of the drag.
        applyLayout();
        saySplit();
    };
    document.addEventListener('mousemove', move);
    document.addEventListener('mouseup', done);
}

/// Move a divider from the keyboard — **the one door**, wherever the keys are.
///
/// In the shell the arrow first tries the nearest inner split along that axis;
/// only when there is none does it move the outer divider along the same axis.
/// That is the terminal build's rule (`keys.rs resize_split`), and it is what
/// makes the key mean "make the thing I am looking at bigger" in both places.
///
/// **Every arrow, not two.** With no inner split, Left/Right used to do
/// nothing from the shell — reported on 2026-09-06 as「シェルパネルで
/// Meta+Shift+矢印で窓サイズの変更ができなかった。ファイラパネルではできる」。
/// The panel's own capture handler was calling `shellresizepane` directly and
/// stopping the event, so this function was never reached at all: a second
/// copy of the rule that had only half of it.
function resizeSplit(key) {
    if (term.on && term.focused) {
        const wider = key === 'ArrowRight' || key === 'ArrowDown';
        const down = key === 'ArrowUp' || key === 'ArrowDown';
        ask('shellresizepane', { wider, down }).then((r) => {
            if (r && r.moved) { takeShell(r); return; }
            // No split that way: the outer divider along the same axis.
            if (key === 'ArrowUp') layout.main -= STEP_PCT;
            else if (key === 'ArrowDown') layout.main += STEP_PCT;
            else if (key === 'ArrowRight') layout.panes += STEP_PCT;
            else if (key === 'ArrowLeft') layout.panes -= STEP_PCT;
            applyLayout();
            // Said, as it is from the listing. From inside the shell the pane
            // divider is off-screen above, so without a line the key looks
            // like it did nothing at all — which is what it used to do.
            saySplit();
            if (r) takeShell(r);
        });
        return;
    }
    if (key === 'ArrowRight') layout.panes += STEP_PCT;
    else if (key === 'ArrowLeft') layout.panes -= STEP_PCT;
    // Down gives the files more room, which is to say the shell less.
    else if (key === 'ArrowDown') layout.main += STEP_PCT;
    else if (key === 'ArrowUp') layout.main -= STEP_PCT;
    else return;
    applyLayout();
    saySplit();
}

/// Where the two dividers stand now. One sentence, so the shell and the
/// listing report a move the same way.
function saySplit() {
    say(tr(`files ${Math.round(layout.main)}%   left pane ${Math.round(layout.panes)}%`,
           `ファイル ${Math.round(layout.main)}%   左ペイン ${Math.round(layout.panes)}%`));
}

/// `:preview` — follow the cursor.
///
/// Off by default and deliberately: reading every file the cursor passes over
/// is a lot of disk for a feature you want on the ten seconds you are looking
/// for something. On, it is the fastest way to find "the one with the error
/// in it".
const preview = { on: false, at: null };

function togglePreview() {
    preview.on = !preview.on;
    if (preview.on) {
        // The panel is where it draws, so the panel has to be there. cian-tui
        // always has one; here it is opened, and the shell inside it runs
        // untouched underneath — Shift+J gives it the keys and the pixels back.
        if (!term.on) openShell().then(() => { blurShell(); showPreview(); });
        else showPreview();
        say(tr('preview on — the shell panel follows the cursor (Shift+J for the shell)',
               'プレビュー: シェルパネルがカーソルを追います（Shift+J でシェルに戻る）'));
    } else {
        preview.at = null;
        paintPreview(null);
        say(tr('preview stopped', 'プレビューを止めました'));
    }
}

/// Show the preview, or hand the panel back to the shell.
///
/// **The whole panel, not the whole screen.** This used to call
/// `lookInside()`, which opens the editor over everything — that is opening a
/// file, not glancing at one, and it buried the listing you were walking. The
/// terminal build borrows the shell panel's area (preview.rs) and leaves the
/// PTY running behind it; so does this.
function paintPreview(body) {
    const showing = preview.on && !!body && !(term.on && term.focused);
    el.shell.classList.toggle('previewing', showing);
    el.sPreview.hidden = !showing;
    if (!showing) { el.sPreview.replaceChildren(); return; }
    el.sPreview.replaceChildren(body);
    el.sPreview.scrollTop = 0;
}

function previewNote(text) {
    const p = document.createElement('div');
    p.className = 'note';
    p.textContent = text;
    return p;
}

let previewSoon = null;
function showPreview() {
    if (!preview.on) return;
    // A beat behind the cursor. Held down, `j` would otherwise read every file
    // it passes, and the one you stop on is the only one that matters.
    clearTimeout(previewSoon);
    previewSoon = setTimeout(async () => {
        if (!preview.on) return;
        const which = state.focus;
        const pane = state[which];
        const row = pane && pane.entries[pane.cursor];
        if (!row || row.parent) { paintPreview(null); return; }
        // A server pane would download every file the cursor passed over,
        // which is the terminal build's reason for declining it too.
        if (pane.remote) { paintPreview(previewNote(tr('a server pane — no preview (it would download every file)', 'サーバのペイン ── プレビューしません（毎回ダウンロードになります）'))); return; }
        if (preview.at === row.path) return;
        preview.at = row.path;
        if (row.is_dir) {
            const r = await ask('peekdir', { path: row.path }).catch(() => null);
            if (preview.at !== row.path) return;
            paintPreview(r ? dirPreview(r) : previewNote(tr('cannot read this folder', 'このディレクトリは読めません')));
            return;
        }
        if (/\.(png|jpe?g|gif|webp|bmp|svg|avif|ico)$/i.test(row.name)) {
            // `bytes` reads the row under this pane's cursor, which is what a
            // preview is about — no second way of naming the same file.
            const r = await ask('bytes', { pane: which }).catch(() => null);
            if (preview.at !== row.path) return;
            if (!r || !r.kind) { paintPreview(previewNote(row.name)); return; }
            const img = document.createElement('img');
            img.src = `data:${r.kind};base64,${r.b64}`;
            paintPreview(img);
            return;
        }
        const f = await ask('viewpath', { path: row.path }).catch(() => null);
        if (preview.at !== row.path) return;
        if (!f) { paintPreview(previewNote(row.name)); return; }
        // A glance, not a browser: the head of the file is what a preview is.
        const head = f.lines.slice(0, 400).join('\n');
        const box = document.createElement('div');
        box.textContent = head || tr('(empty)', '（空です）');
        paintPreview(box);
    }, 250);
}

/// A directory, as rows. Capped for the same reason the terminal build caps
/// it: a preview is a glance, and entering it is one keypress away.
function dirPreview(r) {
    const box = document.createElement('div');
    for (const e of (r.entries || []).slice(0, 500)) {
        const line = document.createElement('div');
        line.className = 'dirrow' + (e.is_dir ? ' d' : '');
        line.textContent = (e.is_dir ? '▸ ' : '  ') + e.name;
        box.append(line);
    }
    if (!box.childNodes.length) box.append(previewNote(tr('(empty)', '（空です）')));
    return box;
}

/// `Ctrl+Shift+Enter` — the commands init.lua keeps, sent to the shell.
///
/// `enter: false` types the line and stops, which is for the commands worth
/// reading before running — the terminal build's distinction, kept.
/// `:shellname` — what this shell tab is for.
///
/// The terminal build wants this too: a tab strip is only worth having if the
/// labels tell the tabs apart, and `shell 2` never does. Empty puts the
/// number back.
async function cmdShellName(name) {
    if (!needShell()) return;
    const now = (term.names || [])[term.tab] || '';
    // An optional argument arrives as `''`, not `undefined` — so testing for
    // undefined renamed the tab to nothing the moment `:shellname` was typed
    // on its own, which is the one spelling that should ask.
    const want = name ? name : await askFor(tr('a name for this shell', 'このシェルの名前'), now);
    if (want === null) return;
    const r = await ask('shellrename', { name: want.trim() });
    if (!r) return;
    // takeShell lays the panes out again, and the strip is drawn from there.
    // Calling drawShell() by hand meant calling it with no screen at all.
    takeShell(r);
    say(want.trim() ? tr(`shell ${term.tab + 1}: ${want.trim()}`, `シェル ${term.tab + 1}: ${want.trim()}`) : tr('the name is gone', '名前を外しました'));
}

/// `:sessionlog` — everything this pane shows, teed to a file. On again to
/// stop. The frame turns carmine while it runs, which is the terminal
/// build's signal: a recorded shell should not look like an unrecorded one.
/// Put this shell pane in the sync group, or take it out.
///
/// With nobody chosen the group is every pane, so the first pane picked is the
/// one that *narrows* it — which is worth saying, because "1 pane in the
/// group" reads like less than "all of them" until you have done it once.
async function cmdSyncMember() {
    if (!needShell()) return;
    const r = await ask('shellsyncmember', {});
    if (!r) return;
    takeShell(r);
    say(r.members
        ? tr(`sync: only the ${r.members} chosen panes`, `同時入力: 選んだ ${r.members} ペインだけに送ります`)
        : tr('sync: every pane', '同時入力: 全ペインに送ります'));
}

async function cmdShellLog() {
    if (!needShell()) return;
    const d = new Date();
    const p = (n) => String(n).padStart(2, '0');
    const name = `cian-shell-${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}.log`;
    const r = await ask('shelllog', { pane: state.focus, name });
    if (!r) return;
    el.shell.classList.toggle('logging', !!r.logging);
    if (r.logging) say(tr(`recording: ${r.logging}`, `記録中: ${r.logging}`));
    else say(tr(`recording stopped: ${r.stopped || ''}`, `記録を止めました: ${r.stopped || ''}`));
}

async function cmdSnippets() {
    const r = await ask('snippets', {});
    if (!r) return;
    if (!r.rows.length) { say(tr('no snippets (init.lua’s cian.snippets)', 'スニペットがありません（init.lua の cian.snippets）')); return; }
    show(tr('Snippets', 'スニペット'), tr(`${r.rows.length} items`, `${r.rows.length} 件`), r.rows.map((x) => ({
        n: x.enter ? '⏎' : '',
        label: x.name,
        sub: x.cmd,
        cmd: x.cmd,
        enter: x.enter,
        confirm: x.confirm,
    })), {
        filter: true,
        hint: tr('type to narrow', '打って絞り込み'),
        foot: tr('type to narrow   Enter sends it to the shell   Esc close', '打って絞る   Enter シェルへ送る   Esc 閉じる'),
        pick: async (row) => {
            closeReport();
            // `confirm = true` in init.lua means "ask me before you send
            // this one" — it is put there for the snippets that do something.
            // The flag was being ignored, which made it a lie in the config.
            if (row.confirm && !await confirm(tr(`ran ${row.label}`, `${row.label} を実行`), row.cmd)) {
                say(tr('stopped', 'やめました'));
                return;
            }
            if (!term.on) await openShell();
            await ask('shellinput', { text: row.cmd + (row.enter ? '\n' : '') });
            setShellFocus(true);
            say(row.enter ? tr(`ran ${row.label}`, `${row.label} を実行`) : tr(`${row.label} is at the prompt — Enter runs it`, `${row.label} を置きました — Enter で実行`));
        },
    });
}

async function cmdSync() {
    if (!needShell()) return;
    const r = await ask('shellsync', {});
    if (!r) return;
    takeShell(r);
    say(r.sync ? tr('sync: every pane', '同期入力: 全ペインに送ります') : tr('sync stopped', '同期入力を止めました'));
}

async function splitShell(down) {
    const r = await ask('shellsplit', { pane: state.focus, down, ...shellSize() });
    if (!r) return;
    takeShell(r);
    say(down ? tr('split top / bottom', '上下に分割') : tr('split left / right', '左右に分割'));
}

/// Close one shell pane — the focused one, or the named one when a shell
/// has ended on its own.
///
/// Asked for first when it is a deliberate close, as the terminal build asks:
/// a split pane may be holding a program that is still running, and Shift+F10
/// is one key away from Shift+F9.
async function closePane(id) {
    const byHand = id === undefined;
    if (byHand && !await confirm(tr('Close this split pane', 'この分割パネルを閉じます'),
        tr('anything running in it ends', '動いているプログラムがあれば終わります'))) { say(tr('stopped', 'やめました')); return; }
    const r = await ask('shellpaneclose', id === undefined ? {} : { id });
    if (!r) return;
    if (r.gone) { closeShell(); say(tr('the shell is closed', 'シェルを閉じました')); return; }
    takeShell(r);
    if (byHand) say(tr('the split pane is closed', '分割パネルを閉じました'));
}

async function shellTab() {
    if (!term.on) { await openShell(); return; }
    const r = await ask('shelltab', { pane: state.focus, ...shellSize() });
    if (!r) return;
    takeShell(r);
    sayShellTab();
}

async function goTabOfShell(at) {
    const r = await ask('shellgo', { at });
    if (!r) return;
    takeShell(r);
    sayShellTab();
}

async function shellGo(how) {
    const r = await ask('shellgo', how);
    if (!r) return;
    takeShell(r);
    sayShellTab();
}

/// Close the whole shell tab — every split pane in it.
///
/// Asked for, as the terminal build asks: F10 sits one key from F9, and the
/// difference between them is a tab appearing and a tab with four panes in it
/// disappearing.
async function shellCloseTab() {
    if (!await confirm(tr('Close this shell tab (splits and all)', 'このシェルタブを閉じます（分割ごと）'),
        tr('anything running in it ends', '動いているプログラムがあれば終わります'))) { say(tr('stopped', 'やめました')); return; }
    const r = await ask('shellclose', {});
    if (!r) return;
    if (r.gone) { closeShell(); say(tr('the shell is closed', 'シェルを閉じました')); return; }
    takeShell(r);
    sayShellTab();
}

function drawShell(screen, into) {
    // A pane that is not on screen keeps running — that is the point of tabs —
    // and keeps sending screens. Without its own box to go in, drawing it
    // would let a build scrolling in another tab stamp itself over this one.
    const node = into || el.sPanes.querySelector(`.sgrid[data-id="${screen.id}"]`);
    if (!node) return;
    if (screen.id === term.showing) {
        term.rows = screen.rows;
        term.cols = screen.cols;
        // The tab strip, spelled the way the terminal build spells it, and
        // drawn even for one tab — the strip is where you learn that F9 makes
        // another, which a heading reading "シェル" never told anybody.
        el.sTabs.replaceChildren(...Array.from({ length: Math.max(1, term.tabs) }, (_, i) => {
            const t = document.createElement('span');
            // Its name where it has one. Four tabs called `shell 1..4` are
            // four tabs you have to open to tell apart, and the reason for
            // the second one is always that the first is busy with something
            // in particular.
            const name = (term.names || [])[i];
            t.textContent = name || `shell ${i + 1}`;
            t.title = name ? `${name}（shell ${i + 1}）` : `shell ${i + 1}`;
            if (i === term.tab) t.className = 'on';
            t.addEventListener('mousedown', () => goTabOfShell(i));
            // Double-click to rename, the way every tab strip renames.
            t.addEventListener('dblclick', () => { goTabOfShell(i); cmdShellName(); });
            return t;
        }));
        // What the shell itself says it is: `user@host: cwd`, which is the
        // only part of this bar carrying information.
        el.sTitle.textContent = screen.title || '';
        el.sAbout.textContent = `${screen.cols}×${screen.rows}`
            + (screen.scrollback ? tr(`   ↑ ${screen.scrollback} lines back`, `   ↑ ${screen.scrollback} 行戻っています`) : '');
    }
    const frag = document.createDocumentFragment();
    screen.lines.forEach((runs, row) => {
        const div = document.createElement('div');
        // The cursor is drawn by splitting the run it lands in, because a cell
        // is not an element here — runs are, and a run is however many cells
        // looked the same.
        let col = 0;
        for (const run of runs) {
            const text = run.t;
            const onThisRun = !screen.hidden && screen.cursor.row === row
                && screen.cursor.col >= col && screen.cursor.col < col + text.length;
            if (!onThisRun) {
                div.append(styled(run, text));
                col += text.length;
                continue;
            }
            const at = screen.cursor.col - col;
            if (at > 0) div.append(styled(run, text.slice(0, at)));
            const cur = styled(run, text.slice(at, at + 1) || ' ');
            cur.classList.add('cur');
            div.append(cur);
            if (at + 1 < text.length) div.append(styled(run, text.slice(at + 1)));
            col += text.length;
        }
        if (!div.childNodes.length) div.append(document.createTextNode(' '));
        frag.append(div);
    });
    node.replaceChildren(frag);
}

function styled(run, text) {
    const span = document.createElement('span');
    span.textContent = text;
    if (run.f) span.style.color = run.f.startsWith('c') ? `var(--${run.f})` : run.f;
    if (run.b) span.style.background = run.b.startsWith('c') ? `var(--${run.b})` : run.b;
    if (run.bold) span.style.fontWeight = '600';
    if (run.it) span.style.fontStyle = 'italic';
    if (run.ul) span.style.textDecoration = 'underline';
    if (run.inv) span.classList.add('inv');
    return span;
}

/// What a key means to a shell.
///
/// Not a lookup table of everything — the printable characters are themselves,
/// and only the ones that are not a character need naming. Ctrl+letter is the
/// arithmetic it has always been: the letter's position in the alphabet.
function shellBytes(e) {
    const k = e.key;
    // The input method's own keys are the input method's.
    //
    // On a Japanese Windows keyboard **Alt+` is how you turn 全角/半角 on and
    // off**, and `半角/全角` is the same switch under its own key. Both were
    // taken here — Alt+backtick became the two bytes `ESC` `` ` `` and went
    // to the shell, so the IME never saw it and the switch appeared dead.
    //
    // `null` rather than an encoding: the caller leaves the browser default
    // alone for an unencodable key, which is exactly what handing a key back
    // to Windows means.
    // **`e.key` では当たらない。** 日本語キーボードの `半角/全角` は
    // `Backquote` の位置にあり、`key` は環境によって `Zenkaku` だったり
    // `Process` だったり `Unidentified` だったりする。Alt+` も、JIS では
    // バッククォート自体が別の位置にあるので `key === '`'` で待っていても
    // 来ない。**物理位置で見る** ── ただし修飾なしの `Backquote` は US 配列で
    // ただのバッククォートなので、決して飲み込まない。
    const imeNamed = ['Zenkaku', 'Hankaku', 'ZenkakuHankaku', 'KanjiMode',
                      'Convert', 'NonConvert', 'Eisu', 'Kana', 'KanaMode'];
    if (imeNamed.includes(k)) return null;
    if (e.code === 'Backquote'
        && (e.altKey || imeNamed.includes(k) || k === 'Process' || k === 'Unidentified')) {
        return null;
    }
    if (e.altKey && k === '@') return null;
    // **ここだけは Ctrl のまま。** シェルの Ctrl は端末の制御文字で、
    // Ctrl+C は SIGINT。Mac の ⌘C は「コピー」── 名前が同じでも別のもの
    // なので、`mod()` で一緒にしてはいけない。
    if (e.ctrlKey && k.length === 1) {
        const up = k.toUpperCase();
        if (up >= 'A' && up <= 'Z') return String.fromCharCode(up.charCodeAt(0) - 64);
    }
    const named = {
        Enter: '\r', Tab: '\t', Backspace: '\x7f', Escape: '\x1b',
        ArrowUp: '\x1b[A', ArrowDown: '\x1b[B', ArrowRight: '\x1b[C', ArrowLeft: '\x1b[D',
        Home: '\x1b[H', End: '\x1b[F', Delete: '\x1b[3~',
        PageUp: '\x1b[5~', PageDown: '\x1b[6~',
    };
    if (named[k]) return named[k];
    if (k.length === 1) return e.altKey ? `\x1b${k}` : k;
    return null;
}

/// Select text in the shell with the mouse; it is on the clipboard the
/// moment the button comes up. The terminal build's gesture — a terminal
/// where selecting is copying is a terminal you never reach for Cmd+C in.
document.addEventListener('mouseup', () => {
    if (!term.on) return;
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed) return;
    if (!el.sPanes.contains(sel.anchorNode)) return;
    const text = sel.toString();
    if (!text.trim()) return;
    navigator.clipboard.writeText(text);
    say(tr(`${text.length} characters copied`, `${text.length} 文字をコピー`));
});

/// The selection in the shell, onto the clipboard.
function shellCopy() {
    const sel = window.getSelection();
    const text = sel && !sel.isCollapsed && el.sPanes.contains(sel.anchorNode) ? sel.toString() : '';
    if (!text) {
        say(tr('nothing selected — drag to select, or use the menu to interrupt',
               '選択がありません ── ドラッグで選ぶか、メニューから中断できます'), true);
        return;
    }
    navigator.clipboard.writeText(text);
    say(tr(`${text.length} characters copied`, `${text.length} 文字をコピー`));
}

/// The clipboard, into the shell.
async function shellPaste() {
    const text = await navigator.clipboard.readText().catch(() => '');
    if (!text) {
        say(tr('the clipboard is empty', 'クリップボードが空です'), true);
        return;
    }
    await ask('shellinput', { text });
}

document.addEventListener('keydown', (e) => {
    if (!term.on || !term.focused) return;
    // **Ctrl+C / X / V are the clipboard here, not control characters.**
    //
    // 2026-09-05: 「Shell パネルで Ctrl+c/x/v などの Ctrl シリーズを許可して
    // ほしい。デフォルトの Ctrl+C のキャンセルは右クリックおよび Shift+Enter
    // で表示できる中に含めて」. So the interrupt moved to the menu and these
    // three do what they do everywhere else in the window.
    //
    // **Only these three.** Ctrl+A is the start of the line, Ctrl+R is the
    // history, Ctrl+U throws the line away — a shell without them is not a
    // shell, and none of them is what anybody means by 「Ctrl シリーズ」.
    //
    // Nothing can be cut out of what has already scrolled past, so Ctrl+X
    // copies. Silently doing nothing would be worse; pretending to cut and
    // leaving the text there would be worse still.
    if (e.ctrlKey && !e.altKey && !e.metaKey && /^[cxvCXV]$/.test(e.key)) {
        e.stopPropagation();
        e.preventDefault();
        if (e.key.toLowerCase() === 'v') shellPaste();
        else shellCopy();
        return;
    }
    // Esc hands the keys back to the files. A shell wants Esc too — vi lives
    // in one — so it is the one key that has to be pressed twice to reach it,
    // the same bargain the terminal build makes.
    if (e.key === 'Escape' && !escTwice()) {
        e.stopPropagation();
        e.preventDefault();
        blurShell();
        return;
    }
    // The panel's own keys, before the shell's. F-keys are cian's here for
    // the same reason they are in the terminal build: a shell almost never
    // wants them, and a panel with no way to open a second tab is a panel you
    // leave to run one thing.
    // The menu. In a shell `:` is a character, so this is the only way to
    // cian's command line from here — which is exactly why cian-tui puts
    // `コマンド入力` at the top of the shell menu and advertises `S-Enter` on
    // the shell's hint row. The window advertised it too, on the *file* row,
    // and had it nowhere: from the shell the menu was right-click only.
    if (e.key === 'Enter' && e.shiftKey) {
        e.stopPropagation();
        e.preventDefault();
        openMenu(CONTEXT);
        return;
    }
    if (e.key === 'F9' && !e.shiftKey) { e.stopPropagation(); e.preventDefault(); shellTab(); return; }
    if (e.key === 'F12' && e.shiftKey) {
        e.stopPropagation();
        e.preventDefault();
        ask('shellpanezoom', {}).then((r) => {
            if (!r) return;
            takeShell(r);
            say(r.zoom ? tr('this pane only (Shift+F12 comes back)', 'このペインだけを表示（Shift+F12 で戻る）') : tr('back to the split', '分割に戻しました'));
        });
        return;
    }
    if (e.key === 'F12') {
        e.stopPropagation();
        e.preventDefault();
        zoomFocused();
        return;
    }
    if (e.key === 'F10' && !e.shiftKey) { e.stopPropagation(); e.preventDefault(); shellCloseTab(); return; }
    // The terminal build's three: split, split the other way, close the pane.
    if (e.shiftKey && (e.key === 'F8' || e.key === 'F9' || e.key === 'F10')) {
        e.stopPropagation();
        e.preventDefault();
        if (e.key === 'F10') closePane();
        else splitShell(e.key === 'F9');
        return;
    }
    // Ctrl+S here is not save — there is nothing to save in a shell — it is
    // "say this to all of them", which is what splits are for.
    if (e.key === 's' && mod(e)) {
        e.stopPropagation();
        e.preventDefault();
        ask('shellsync', {}).then((r) => {
            if (!r) return;
            takeShell(r);
            say(r.sync ? tr('sync: every pane', '同期入力: 全ペインに送ります') : tr('sync stopped', '同期入力を止めました'));
        });
        return;
    }
    // Ctrl+Shift+arrow drags the border the focused pane sits against.
    //
    // **Through `resizeSplit`, not past it.** This handler is on `document`
    // in the *capture* phase, so its `stopPropagation` keeps the event from
    // ever reaching the listing's bubble handler — where the same key lives.
    // Written out again here, it was the half of the rule without the
    // fallback, and in a shell with no inner split the key did nothing at all.
    if (mod(e) && e.shiftKey && e.key.startsWith('Arrow')) {
        e.stopPropagation();
        e.preventDefault();
        resizeSplit(e.key);
        return;
    }
    // F1-F8 go straight to a tab; the pane keys are the Shift ones.
    if (/^F[1-8]$/.test(e.key) && !e.shiftKey && !mod(e)) {
        e.stopPropagation();
        e.preventDefault();
        goTabOfShell(Number(e.key.slice(1)) - 1);
        return;
    }
    if (e.shiftKey && (e.key === 'F1' || e.key === 'F2')) {
        e.stopPropagation();
        e.preventDefault();
        ask('shellfocus', { step: e.key === 'F1' ? -1 : 1 }).then((r) => r && takeShell(r));
        return;
    }
    if (e.key === 'F1' || e.key === 'F2') {
        e.stopPropagation();
        e.preventDefault();
        shellGo({ step: e.key === 'F1' ? -1 : 1 });
        return;
    }
    // Scrolling back through what has gone past, rather than into the shell.
    if (e.shiftKey && (e.key === 'PageUp' || e.key === 'PageDown')) {
        e.stopPropagation();
        e.preventDefault();
        // Positive goes *back* through the history (cian-pty `scroll_back`,
        // and cian-tui passes `page` for PageUp). This had the sign the other
        // way round, so Shift+PageUp asked to go forward from the live end —
        // clamped at zero, a no-op. The panel's scrollback has never been
        // reachable from this window.
        scrollShell(e.key === 'PageUp' ? term.rows : -term.rows);
        return;
    }
    const bytes = shellBytes(e);
    if (bytes === null) {
        // A key the shell cannot encode still must not fall through to the
        // listing — F3 used to open the viewer over a shell being typed in.
        // Propagation stops; **the browser default is left alone**, which is
        // how the input method's own switch (Alt+` and 半角/全角) reaches
        // Windows rather than being typed into the shell.
        e.stopPropagation();
        return;
    }
    e.stopPropagation();
    e.preventDefault();
    ask('shellinput', { text: bytes });
}, true);

/// Two Escs in quick succession go through to the shell; one comes back to the
/// files. Anything else in between resets it.
let lastEsc = 0;
function escTwice() {
    const now = performance.now();
    const twice = now - lastEsc < 500;
    lastEsc = twice ? 0 : now;
    return twice;
}

async function scrollShell(lines) {
    const r = await ask('shellscroll', { lines });
    if (!r) return;
    takeShell(r);
    // Where you are in the history, said as cian-tui says it — a panel
    // showing old output with no sign that it is old is a panel you think
    // has stopped.
    const back = r.panes && r.panes.find((p) => p.focused);
    const at = back && back.screen ? back.screen.scrollback : 0;
    say(at ? tr(`${at} lines back — typing returns to the end`, `${at} 行さかのぼり中 — 何か入力すると戻ります`) : tr('at the newest output', '最新の出力'));
}

// ─────────────────────────────────────────────────────────────────────────
// Dropping files in from the desktop.
//
// The one thing a window can do that a terminal can only imitate. It **moves**
// them, which is what the terminal build's drop does and what dragging between
// two folders means everywhere else — and it asks first, by name, because a
// drop is the easiest gesture in the whole program to make by accident.
// ─────────────────────────────────────────────────────────────────────────
for (const which of ['left', 'right']) {
    const pane = el[which];
    // Clicking a pane puts the keys in it — anywhere in it, and whether or
    // not there is a row under the pointer.
    //
    // The row handlers moved the *cursor* and the current pane, which is not
    // the same question: with the shell focused, clicking a listing left the
    // keyboard in the shell, so neither surface looked right. Registered on
    // the pane once rather than on every row every repaint, so an empty pane
    // and the path line take focus too.
    pane.addEventListener('mousedown', async (e) => {
        setShellFocus(false);
        state.focus = which;
        draw('left');
        draw('right');
        // The empty ground below the listing clears the marks (grid.rs:372).
        // A pane full of marks and no obvious way to drop them is a pane you
        // reach for Esc in and hope.
        // `state[which]` can be null — a pane the engine has not answered for
        // yet, and every teardown path. The handler is async and threw into a
        // promise nobody was holding, which is the quietest way for a click
        // to go wrong.
        if (e.target.classList.contains('rows') && state[which] && state[which].marked > 0) {
            const next = await ask('unmarkall', { pane: which });
            if (next) { state[which] = next; draw(which); say(tr('marks cleared', 'マークを解除しました')); }
        }
    });
    // The menu on the pane's own background, its path line, an empty listing —
    // cian-tui opens it for a right-click anywhere in the pane (mouse.rs), and
    // this had it only on a row with something under the pointer.
    pane.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        setShellFocus(false);
        state.focus = which;
        draw('left');
        draw('right');
        openMenu(CONTEXT);
    });
    // The wheel takes the pane too. Two panes side by side and a wheel that
    // scrolls whichever one the pointer is over, while the keys stay in the
    // other, is two different answers to "where am I".
    pane.addEventListener('wheel', () => {
        if (state.focus === which && !(term.on && term.focused)) return;
        setShellFocus(false);
        state.focus = which;
        draw('left');
        draw('right');
    }, { passive: true });
    pane.addEventListener('dragover', (e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        pane.classList.add('dropping');
    });
    pane.addEventListener('dragleave', () => pane.classList.remove('dropping'));
    pane.addEventListener('drop', async (e) => {
        e.preventDefault();
        pane.classList.remove('dropping');
        const paths = [...e.dataTransfer.files]
            .map((f) => window.cian.pathOf(f))
            .filter(Boolean);
        if (!paths.length) { say(tr('the dropped items have no path I can read', '落とされたものの場所が分かりません'), true); return; }
        const dest = state[which];
        if (!dest) return;
        // A drop lands in `pane.cwd`, and on a remote pane that is still the
        // *local* directory from before the connection — the files would move
        // somewhere real and invisible, which is the worst combination.
        const names = paths.map((p) => p.split(/[\\/]/).pop());
        if (dest.remote) {
            if (!await confirm(tr(`Upload ${paths.length} to ${dest.remote}`, `${paths.length} 件を ${dest.remote} へアップロードします`), names.join('\n'))) {
                say(tr('stopped', 'やめました'));
                return;
            }
            const up = await ask('uploadpaths', { pane: which, paths });
            if (!up) return;
            state[which] = up;
            draw(which);
            if (up.errors.length) say(up.errors.join('  /  '), true);
            else say(tr(`uploaded ${up.ok}`, `${up.ok} 件をアップロードしました`));
            return;
        }
        if (!await confirm(tr(`Move ${paths.length} to ${dest.cwd}`, `${paths.length} 件を ${dest.cwd} へ移動します`), names.join('\n'))) {
            say(tr('stopped', 'やめました'));
            return;
        }
        const r = await ask('drop', { pane: which, paths });
        if (!r) return;
        beginOp(r, 'move', tr('move', '移動'));
    });
}

/// What the engine says unasked.
///
/// **Nothing was listening.** The bridge has offered `onEvent` since the spine
/// went in, and the renderer never subscribed — so a copy said "0 / 1" and
/// then sat there for ever, the panes did not reload when it finished, and a
/// job that failed reported nothing at all. The engine had been refusing to
/// copy a directory into itself, correctly and in silence.
window.cian.onEvent(async (msg) => {
    switch (msg.event) {
        case 'progress':
            // A transfer's op is not known until its first event says so.
            if (running && running.op === null) running.op = msg.op;
            if (!running || msg.op !== running.op) return;
            running.done = msg.done;
            running.total = Math.max(msg.total, running.total);
            running.bytes = msg.bytes ?? 0;
            running.bytesTotal = msg.bytes_total ?? 0;
            running.ms = msg.ms ?? 0;
            running.path = msg.path || '';
            prog.stalledAt = performance.now();
            drawProg();
            say(tr(`${running.verb}… ${msg.done} / ${running.total}  ${base(msg.path)}`, `${running.verb}中… ${msg.done} / ${running.total}  ${base(msg.path)}`));
            return;

        case 'done': {
            // A transfer clears its own bar where it was started — it is
            // awaited there, and the reply is what reloads the panes.
            if (running && running.kind === 'transfer' && msg.op === running.op) return;
            if (!running || msg.op !== running.op) {
                // A job cancelled while it was still in the queue: it never
                // ran, so nothing here is tracking it, and it still has to be
                // said — silence would read as "the cancel did not take".
                if (msg.cancelled) say(tr(`#${msg.op} cancelled`, `#${msg.op} を取り消しました`));
                return;
            }
            const verb = running.verb;
            running = null;
            el.prog.hidden = true;
            // Awaited, because the listings speak too — and whichever of the
            // two says its piece last is the one that stays on screen.
            await reread();
            // The marks were the *instruction*, and the instruction has been
            // carried out — cian-tui clears them here too (`actions.rs:3013`,
            // right after its own `reload_both`). The window never said so:
            // it relied on the engine's `list` rebuilding the pane from
            // scratch, which cleared the marks as a side effect and took the
            // history, the sort and the hidden-file setting with them. With
            // `list` fixed to walk rather than rebuild, this has to be asked
            // for out loud, which is where it belonged anyway.
            await ask('unmarkall', { pane: state.focus }).then((p) => {
                if (!p) return;
                state[state.focus] = p;
                draw(state.focus);
            });
            if (msg.cancelled) say(tr(`${verb} stopped (${msg.ok} done)`, `${verb}を中止しました（${msg.ok} 件は済み）`), true);
            // **Permission denied on Windows is a question, not a verdict.**
            // cian-tui raises `ConfirmElevate` here and offers to run the same
            // transfer as administrator; this build printed the error and
            // stopped, which on a managed machine is most of what a copy into
            // Program Files ever says. Asked rather than done: a UAC prompt
            // nobody asked for is worse than the error.
            else if (msg.elevate) { await offerElevate(msg, verb); }
            // Every failure, named. A count of them tells you something went
            // wrong without telling you what, which is the worst of both.
            else if (msg.errors.length) say(msg.errors.join('  /  '), true);
            else if (msg.skipped) say(tr(`${verb} ${msg.ok}, skipped ${msg.skipped}`, `${verb} ${msg.ok} 件、${msg.skipped} 件は飛ばしました`));
            else say(tr(`${verb} ${msg.ok} (${msg.ms} ms)`, `${verb} ${msg.ok} 件（${msg.ms} ms）`));
            // The desktop is told too, for the job that outlasted your
            // attention — which is the only kind worth interrupting for.
            notifyDone(msg.ms ?? 0, status.msg);
            return;
        }

        case 'ai': {
            const hand = aiWaiting;
            aiWaiting = null;
            if (msg.error) { say(msg.error, true); return; }
            // A plain answer hands over its text; a structured one (rows and
            // a `what`) hands over the whole payload.
            if (hand) await hand(msg.rows ? msg : msg.answer);
            return;
        }

        case 'shell':
            if (term.on) drawShell(msg);
            return;

        case 'shellnote':
            say(msg.note, true);
            return;

        case 'shellexit':
            // A shell that ended by itself — `exit`, Ctrl+D, a crash. Its
            // pane goes; the others keep running. Nothing is asked, because
            // the person already said so by typing exit.
            if (term.on) await closePane(msg.id);
            return;

        case 'finding':
            if (finder.open) el.findFoot.textContent = tr(`${msg.found} so far…`, `${msg.found} 件を見ています…`);
            return;

        case 'found':
            if (!finder.open) return;
            // The walk is over. Left true, the very next drawHits() painted
            // "（まだ探しています）" over this line forever.
            finder.walking = false;
            el.findFoot.textContent = msg.capped
                ? tr(`stopped at ${msg.total} — narrow it down`, `${msg.total} 件で打ち切り — 絞り込んでください`)
                : tr(`${msg.total}`, `${msg.total} 件`);
            rankNow();
            return;
    }
});

function base(p) {
    return String(p).split(/[\\/]/).pop();
}

/// `Ctrl+=` / `Ctrl+-` / `Ctrl+0` — the window's own type size.
///
/// The terminal build cannot do this: the font belongs to the emulator, so it
/// asks the emulator to change it and remembers a point size in `font_level`.
/// This build owns its window, so it just changes it — and keeps the number
/// under `gui_font`, because pixels here and points there are not the same
/// number even though they are the same idea.
const FONT = { min: 10, max: 28, at: 15 };

/// What size this look starts from — 端末譲り is deliberately tighter. Ctrl+0
/// returns here, and an inline override is only written when the choice
/// differs from it: a permanent inline style silently beat the look's own
/// 14px/19px forever after the first Ctrl+=.
function baseFont() {
    return document.documentElement.dataset.look === 'terminal' ? 14 : 15;
}

function setFont(px, remember = true) {
    FONT.at = Math.max(FONT.min, Math.min(FONT.max, px));
    const r = document.documentElement.style;
    if (FONT.at === baseFont()) {
        r.removeProperty('--size');
        r.removeProperty('--cell-h');
    } else {
        r.setProperty('--size', `${FONT.at}px`);
        // The rows have to grow with the type or the listing keeps its old
        // spacing and the text collides with it.
        r.setProperty('--cell-h', `${Math.round(FONT.at * 1.45)}px`);
    }
    if (viewer.ed) viewer.ed.updateOptions({ fontSize: FONT.at });
    if (term.on) ask('shellresize', shellSize());
    // The foot bars grew or shrank with everything else; the panes have to be
    // told, or the last row hides under them.
    measureFoot();
    if (remember) ask('remember', { key: 'gui_font', value: String(FONT.at) });
}

/// What the last session chose. Applied before the first draw, so the window
/// never flashes the default look on its way to the chosen one.
async function recall() {
    const s = await ask('settings', {});
    if (!s) return;
    if (s.look) {
        const at = LOOKS.findIndex(([v]) => (v || 'hakuji') === s.look);
        if (at >= 0) setLook(at, false);
    }
    if (s.style) {
        const at = STYLES.findIndex(([v]) => v === s.style);
        if (at >= 0) setStyle(at, false);
    }
    // Configured is on. The terminal build has no switch — `sync_ime` runs
    // whenever `cian.ime{}` exists — and the window asked you to type `:ime`
    // first, so the same init.lua behaved differently in the two builds.
    // `:ime` stays, as the way to stop it and as the diagnosis.
    if (s.ime) { ime.on = true; syncIme(); }
    if (s.font) {
        const px = Number(s.font);
        if (px >= FONT.min && px <= FONT.max) setFont(px, false);
    }
    if (s.view && VIEWS.includes(s.view)) setView(s.view, false);
    if (s.hints === '0') { hintsOn = false; drawHints(); }
    // Where the dividers were left. Applied without saving them straight back.
    const pct = (v, fallback) => (Number.isFinite(Number(v)) && Number(v) > 0 ? Number(v) : fallback);
    layout.main = pct(s.main_pct, layout.main);
    layout.panes = pct(s.panes_pct, layout.panes);
    applyLayout(false);
    applyKeymaps(s.keymaps);
    // The eighteen, and whichever of them the terminal build was last set to
    // — because they are one program and `theme` is one setting.
    const t = await ask('themes', {});
    if (t) {
        for (const p of t.list) palettes.set(p.name, p);
        if (Array.isArray(t.grounds)) grounds = t.grounds;
        // `theme` wins unless one of the window's own looks (陰翳・端末譲り)
        // was chosen *after* it — setPalette resets gui_look to 白磁, so a
        // surviving non-白磁 look is by definition the later choice.
        if (t.now && palettes.has(t.now) && (!s.look || s.look === 'hakuji')) {
            setPalette(t.now, false);
        }
    }
    // init.lua can set the two the engine owns, so ask what it actually
    // holds rather than assuming the defaults. Written as one round trip with
    // nothing to change: `switches` with no argument only answers.
    const sw = await ask('switches', {});
    if (sw) { switches.verify = !!sw.verify; switches.cloud = !!sw.cloud; }
    // …and the rest of what init.lua asked for. state.toml holds what the app
    // chose; this holds what the person wrote, and it comes second because a
    // setting written by hand should not be overwritten by whatever the app
    // happened to do last — except where the app's own key is the same
    // question, in which case state.toml has already answered it above.
    const c = s.cfg || {};
    if (!s.view && c.view && VIEWS.includes(c.view)) setView(c.view, false);
    if (!s.style && c.edit_style) {
        const at = STYLES.findIndex(([v]) => v === (c.edit_style === 'vi' ? 'vim' : c.edit_style));
        if (at >= 0) setStyle(at, false);
    }
    if (s.hints === null && c.key_hints === false) { hintsOn = false; drawHints(); }
    if (typeof c.notify === 'boolean') switches.notify = c.notify;
    if (Number.isFinite(c.notify_min_secs)) notifyAfterMs = c.notify_min_secs * 1000;
    if (c.preview === true) preview.on = true;
    // `cian.set_option("lang", "en")`, the same option cian-tui reads.
    if (c.lang === 'en' || c.lang === 'ja') { lang = c.lang; paintMarkup(); }
    // `hidden` is a toggle on the engine, so ask for it only when init.lua
    // wants it on and it is not already.
    if (c.show_hidden === true && state.left && !state.left.hidden_shown) await toggleHidden();
    // What the context menu needs before it can decide which launcher rows
    // exist at all. Kept rather than re-asked: the menu is built synchronously
    // on a keystroke, and a row that appears a moment after the menu does is
    // worse than one that never appears.
    // `cian.set_option("tab_width", n)`。**端末版は `viewer::set_tab_width`
    // でずっと効いていて、窓版は受け取って捨てていた** ── Monaco の既定の 4
    // のまま。開いているものにも、次に開くものにも当てる。
    if (Number.isFinite(c.tab_width) && c.tab_width > 0) {
        cfg.tabWidth = c.tab_width;
        applyTabWidth();
    }
    applyFace(c.font_face);
    cfg.ai = c.ai === true;
    cfg.snippets = c.snippets === true;
    cfg.macros = c.macros === true;
    cfg.hosts = c.ssh_hosts === true;
    // Same reason, for the OS group: asked once, used synchronously.
    if (s.os) Object.assign(osCan, s.os);
    if (Array.isArray(s.sharepoint)) sharepoint = s.sharepoint;
    // Lua's own complaints go in the same queue as mine — from where the
    // person stands they are one thing: "my config did not take".
    keymapErrors = [...(s.config_errors || []), ...keymapErrors];
}

/// The window changed shape, so the shell's idea of its own size is stale.
///
/// Nothing watched for this: a PTY opened at 82×7 stayed 82×7 however far the
/// window was dragged, and every full-screen program in it drew to the wrong
/// rectangle. Debounced, because a drag is a hundred of these.
let resizeTimer = null;

// The wheel over the shell walks its scrollback, as it does in cian-tui
// (mouse.rs) — the panel is `overflow: hidden`, so without this the gesture
// did nothing at all and Shift+PageUp was the only way back.
el.sPanes.addEventListener('wheel', (e) => {
    if (!term.on) return;
    e.preventDefault();
    // Wheel up goes back, as it does in cian-tui (mouse.rs:754).
    scrollShell(e.deltaY < 0 ? 3 : -3);
}, { passive: false });

// Right-click in the shell opens the shell's own menu — it opened nothing.
el.sPanes.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    setShellFocus(true);
    openMenu(CONTEXT);
});

el.gripPanes.addEventListener('mousedown', (e) => startGripDrag('panes', e));
el.gripMain.addEventListener('mousedown', (e) => startGripDrag('main', e));

window.addEventListener('resize', () => {
    measureFoot();
    placeGrips();
    if (viewer.ed) viewer.ed.layout();
    // The shell is handled by the observer below, which sees the panel's box
    // change for any reason — a drag, a font change, the hint bar going away
    // — rather than only for this one.
});

recall().then(() => {
    // The third surface, from the start.
    //
    // cian-tui's normal layout is three — the two file panes and the shell —
    // and a window where the shell only exists after Shift+J is a window
    // where the shell is not part of the program. Opened without focus: the
    // keys still belong to the listing until Shift+J asks for them.
    if (!term.on) openShell({ focus: false });
});

/// Keep the PTY's size equal to the box it is drawn in.
///
/// Watched rather than timed. The shell is opened before the hint bar has
/// been drawn and before the panel has its final height, so any single
/// re-measure is a guess about when layout finishes — and a PTY told the
/// wrong number paints its bottom rows off the end of the panel. This fires
/// when the box actually changes, which is the condition itself.
new ResizeObserver(() => {
    if (!term.on) return;
    const size = shellSize();
    if (size.cols === term.cols && size.rows === term.rows) return;
    term.cols = size.cols;
    term.rows = size.rows;
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => ask('shellresize', size), 60);
}).observe(el.sPanes);

/// Measure again once the bundled font is actually in.
///
/// The observer above watches the *box*, which is the right thing to watch
/// for a panel that changes size — and blind to this: the box does not move
/// when the font underneath it is swapped. cian.ttf is twelve megabytes, so
/// the first `measureCell()` can run against the fallback, and a fallback one
/// pixel shorter per line is one row too many in the panel. The rows that do
/// not fit are clipped by `.sgrid`'s `overflow: hidden`, and the row that
/// gets clipped is the last one — the one being typed on.
///
/// Reported from a Windows machine on 2026-08-31, where the font takes longer
/// to arrive and the software renderer changes when layout settles. Not
/// reproduced on a Mac, where it loads before anything asks.
document.fonts.ready.then(() => {
    if (term.on) {
        const size = shellSize();
        if (size.rows !== term.rows || size.cols !== term.cols) {
            term.rows = size.rows;
            term.cols = size.cols;
            ask('shellresize', size);
        }
    }
    // The listing counts rows the same way, and the hint bar's own height
    // feeds the space everything else is laid out in.
    drawHints();
    // …and the opening line goes after that listing, not before it: this
    // `refresh()` is what used to paint over the greeting. `recall()` is
    // awaited too, because the face it reports is the one `cian.font{ face }`
    // asked for and that arrives with the settings.
    Promise.all([refresh(), recall()]).then(greet);
});

drawHints();

/// The opening line: what init.lua got wrong, or what cian is drawing in.
///
/// **Called last, on purpose.** It used to sit in the first `refresh().then`,
/// which is before `fonts.ready` and before the config has arrived — so it
/// measured the fallback, named it as the answer, and was then painted over
/// by the `refresh()` that `fonts.ready` runs. Written, correct-looking in the
/// source, and never once on screen. Measured 2026-09-06.
function greet() {
    if (keymapErrors.length) {
        say(tr(`init.lua keymap: ${keymapErrors.join('  /  ')}`, `init.lua の keymap: ${keymapErrors.join('  /  ')}`), true);
        return;
    }
    const face = resolvedFace();
    const size = getComputedStyle(document.body).fontSize;
    // `cian.font{ face = … }` named something this machine does not have.
    // Worth interrupting the greeting for: the browser walks past a name it
    // cannot find without a word, so the only other way to find out is to
    // notice that the letters are not the ones that were asked for.
    if (cfg.faceAsked && !faceIsHere(cfg.faceAsked)) {
        say(tr(
            `${cfg.faceAsked} is not installed — drawing in ${face} ${size}`,
            `${cfg.faceAsked} は入っていません — ${face} ${size} で描いています`,
        ), true);
        return;
    }
    // The message half only — the whole bar carries chips now, and reading
    // the element back would fold the badge and counts into the greeting.
    say(`${status.msg}   ·   ${face} ${size}`);
}
