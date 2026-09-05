// Put the editor's runtime under gui/vendor/, trimmed to what is actually run.
//
//     node gui/vendor.js
//
// Not committed, and not downloaded either — copied out of node_modules, which
// is where `npm install` has already put it. The same reasoning as the bundled
// font: several megabytes that never change do not belong in every clone
// forever, and the release workflow is the place that assembles them.
//
// **The language services are in, and they were not at first.** They are 7 MB —
// the TypeScript compiler among them — and leaving them out looked obviously
// right: colouring comes from `basic-languages`, which is 640 KB for
// eighty-one languages, and shipping a compiler to colour a batch file would
// be absurd.
//
// It was wrong. Monaco asks for `vs/language/typescript/tsMode` the moment a
// `.js` file is opened, and without it every such file threw on the way in.
// The file displayed and the colouring worked, so the damage was one exception
// per open — the kind of thing that is fine until it is the exception hiding
// the real one. Seven megabytes inside a bundle that is already 173 is not a
// saving worth an error message.

const fs = require('node:fs');
const path = require('node:path');

const HERE = __dirname;
const MODULES = path.join(HERE, 'node_modules');
const OUT = path.join(HERE, 'vendor');

/// Everything the editor loads at runtime, and nothing else.
const WANTED = [
    ['monaco-editor/min/vs/loader.js', 'monaco/vs/loader.js'],
    ['monaco-editor/min/vs/base', 'monaco/vs/base'],
    ['monaco-editor/min/vs/editor', 'monaco/vs/editor'],
    ['monaco-editor/min/vs/basic-languages', 'monaco/vs/basic-languages'],
    ['monaco-editor/min/vs/language', 'monaco/vs/language'],
    // Japanese only. The other eight locales are 1.5 MB for languages this
    // is not offered in; English is built into editor.main.js.
    ['monaco-editor/min/vs/nls.messages.ja.js', 'monaco/vs/nls.messages.ja.js'],
    ['monaco-vim/dist/monaco-vim.umd.js', 'monaco-vim.js'],
    // The diagrams. 3.4 MB, which in a 173 MB bundle is not a number worth an
    // argument — and it is the second thing (after real type) that a window
    // can do with Markdown that a terminal cannot: the terminal build folds a
    // mermaid graph into an arrow list, and this draws the graph.
    ['mermaid/dist/mermaid.min.js', 'mermaid.js'],
    ['mermaid/LICENSE', 'mermaid.LICENSE'],
    // The licences travel with the code they cover. Both are MIT, and a
    // release that shipped the minified JavaScript without them would be
    // distributing someone's work with the terms filed off.
    ['monaco-editor/LICENSE', 'monaco-editor.LICENSE'],
    ['monaco-editor/ThirdPartyNotices.txt', 'monaco-editor.ThirdPartyNotices.txt'],
    ['monaco-vim/LICENSE', 'monaco-vim.LICENSE'],
];

/// Source maps double the size and are read by nobody here — this is a
/// dependency being run, not one being debugged.
function copy(from, to) {
    const stat = fs.statSync(from);
    if (stat.isDirectory()) {
        fs.mkdirSync(to, { recursive: true });
        for (const name of fs.readdirSync(from)) copy(path.join(from, name), path.join(to, name));
        return;
    }
    if (from.endsWith('.map')) return;
    fs.mkdirSync(path.dirname(to), { recursive: true });
    fs.copyFileSync(from, to);
}

function bytes(dir) {
    let n = 0;
    for (const name of fs.readdirSync(dir)) {
        const at = path.join(dir, name);
        const s = fs.statSync(at);
        n += s.isDirectory() ? bytes(at) : s.size;
    }
    return n;
}

const missing = WANTED.filter(([from]) => !fs.existsSync(path.join(MODULES, from)));
if (missing.length) {
    console.error('node_modules に無いものがあります:');
    for (const [from] of missing) console.error(`  ${from}`);
    console.error('\ngui/ で npm install を先に走らせてください。');
    process.exit(1);
}

fs.rmSync(OUT, { recursive: true, force: true });
for (const [from, to] of WANTED) copy(path.join(MODULES, from), path.join(OUT, to));

// The font: HackGen Console NF — Japanese, monospaced, Nerd glyphs — carried
// with the app rather than hoped for on the machine. Relying on it being
// installed put the whole listing in Hiragino (proportional) and drew the
// shell prompt as tofu on any Mac without it, which is most Macs. Copied,
// never downloaded: the release workflow fetches it once into `vendor-font/`
// and this takes it from there.
//
// **It used to live in `crates/cian-gui/fonts/`**, fetched for the winit
// build and borrowed by this one. That crate left on 2026-09-06 and the
// borrow would have gone with it — the release's "the font, or stop" guard
// would have caught it, loudly, in CI. Moved somewhere that belongs to
// nobody instead.
const FONT_SOURCES = [
    process.env.CIAN_FONT,
    path.join(HERE, '..', 'vendor-font', 'cian.ttf'),
    path.join(require('node:os').homedir(), 'Library/Fonts/HackGenConsoleNF-Regular.ttf'),
    'C:/Windows/Fonts/HackGenConsoleNF-Regular.ttf',
    '/usr/share/fonts/truetype/hackgen/HackGenConsoleNF-Regular.ttf',
].filter(Boolean);
const font = FONT_SOURCES.find((p) => fs.existsSync(p));
if (font) {
    copy(font, path.join(OUT, 'fonts', 'cian.ttf'));
} else {
    console.error('フォントが見つかりません（無くても動きますが、一覧が等幅になりません）。探した場所:');
    for (const p of FONT_SOURCES) console.error(`  ${p}`);
    console.error('release.yml と同じ HackGen_NF の zip から vendor-font/cian.ttf に置いてください。');
}
console.log(`gui/vendor/  ${(bytes(OUT) / 1024 / 1024).toFixed(1)} MB`);
