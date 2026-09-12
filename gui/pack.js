/*
 * 配布物を組み立てる ── Electron 本体を同梱して、**exe をダブルクリックで
 * 起動する形**にする。
 *
 *   node gui/pack.js --out /tmp/dist                     … フル（初回配布用）
 *   node gui/pack.js --out /tmp/dist --platform win32    … Windows 版を組む
 *   node gui/pack.js --out /tmp/dist --app-only          … アプリだけ（更新用・数十MB）
 *   node gui/pack.js --out /tmp/dist --rcedit rcedit.exe … exe のアイコンを焼く
 *   node gui/pack.js --where                             … 何を入れるかだけ言う
 *
 * **なぜ electron-builder を使わないか。** crmaine が同じ判断をしていて、
 * 理由がそちらの `gui/pack.js` に書いてある ── 取得を伴うツールは途中で
 * 落ちたときに中途半端な生成物を残し、**それが正常に見える**。ここは
 * コピーとリネームだけで作り、そのかわり**出口で必ず検証する**。
 *
 * 出来上がり（Windows）:
 *   cian/
 *     cian.exe               ← electron.exe をリネームしたもの
 *     resources/app/         ← gui/ 一式（node_modules は除く）と cian-server.exe
 *     GUI.txt CHECK.ja.txt DEPLOY.ja.txt README LICENSE
 *     examples/init.lua
 *
 * **exe の隣に .lua を置かない。** cian は exe の隣の設定を読み書き両方で
 * 優先する（`config_write_path`）。同梱版では exe の隣は `resources/app/`
 * なので、そこに init.lua が1つ紛れると全員のしおりがそこへ保存されに行く。
 * 出口の検証がこれを見る ── 説明ではなく、機械で止める。
 * 設定は `~/.config/cian` に置かせる（crmaine が config.json でそうしている）。
 */
const fs = require('fs');
const path = require('path');
const os = require('os');
const { execFileSync } = require('child_process');

const ROOT = path.join(__dirname, '..');
const NAME = 'cian';

function arg(name, fallback = null) {
    const i = process.argv.indexOf(`--${name}`);
    if (i < 0) return fallback;
    const v = process.argv[i + 1];
    return (!v || v.startsWith('--')) ? true : v;
}
const has = (name) => process.argv.includes(`--${name}`);

const version = (() => {
    const toml = fs.readFileSync(path.join(ROOT, 'Cargo.toml'), 'utf8');
    const m = toml.match(/^version = "(.*)"$/m);
    return m ? m[1] : '0.0.0';
})();

/// `gui/` から**持っていかない**もの。
///
/// `node_modules` は 508MB あり、その中で実際に要るものは `vendor.js` が
/// `gui/vendor/` に写し終えている。残りは道具と読み物 ── 動くのに要らない
/// ものをアプリの中に入れると、次に中を開けた人が「これは何のために
/// 要るのか」を考えることになる。
const SKIP_IN_GUI = new Set([
    'node_modules', 'package-lock.json',
    'drive.js', 'pack.js', 'vendor.js',       // 道具（開発のときだけ）
    'run.bat', 'run.sh',                      // 同梱版には入口が既にある
    'REQUESTS.ja.md', 'ROADMAP.ja.md',        // 読み物
]);

/// 上の階から持っていくもの。`[元, 先]`。
function topFiles(platform) {
    const win = platform === 'win32';
    const rows = [
        ['README.md', 'README.md'],
        ['README.ja.md', 'README.ja.md'],
        ['LICENSE', 'LICENSE'],
        ['examples/init.lua', 'examples/init.lua'],
    ];
    if (win) {
        // **同梱版は読む順が違う。** `START.ja.txt` が「cian.exe を叩く」の
        // 一枚で、`GUI.txt` は Electron を自分で用意する道（前面だけの zip）
        // の説明。両方入れておくと、社内で Electron を共用したくなった日に
        // 読むものが手元にある。
        rows.push(['packaging/windows/START.ja.txt', 'START.ja.txt']);
        rows.push(['packaging/windows/GUI.txt', 'GUI.txt']);
        rows.push(['packaging/windows/CHECK.ja.txt', 'CHECK.ja.txt']);
        rows.push(['packaging/windows/DEPLOY.ja.txt', 'DEPLOY.ja.txt']);
    } else {
        rows.push(['packaging/macos/GUI.ja.txt', 'GUI.ja.txt']);
    }
    return rows.filter(([from]) => fs.existsSync(path.join(ROOT, from)));
}

function copyDir(src, dst, skip = new Set()) {
    fs.mkdirSync(dst, { recursive: true });
    for (const name of fs.readdirSync(src)) {
        if (skip.has(name)) continue;
        const s = path.join(src, name);
        const d = path.join(dst, name);
        const st = fs.lstatSync(s);
        if (st.isSymbolicLink()) {
            // Electron の中は symlink だらけ（Frameworks の Versions/Current）。
            // 辿って写すと**中身が二重になって 2 倍の大きさになる**ので、
            // link のまま写す。
            const to = fs.readlinkSync(s);
            fs.rmSync(d, { force: true, recursive: true });
            fs.symlinkSync(to, d);
        } else if (st.isDirectory()) {
            copyDir(s, d, skip);
        } else {
            fs.copyFileSync(s, d);
        }
    }
}

function dirSize(dir) {
    let n = 0;
    for (const name of fs.readdirSync(dir)) {
        const p = path.join(dir, name);
        const st = fs.lstatSync(p);
        if (st.isSymbolicLink()) continue;
        n += st.isDirectory() ? dirSize(p) : st.size;
    }
    return n;
}
const mb = (n) => `${(n / 1048576).toFixed(0)} MB`;

/// Electron の展開先を探す。**run.bat / run.sh と同じ順**にする ── 探す順が
/// 道具ごとに違うと、「どの Electron で動いたのか」が答えられなくなる。
function findElectron(platform) {
    const given = arg('electron');
    const cand = [];
    if (given && given !== true) cand.push(given);
    if (process.env.CIAN_ELECTRON_DIST) cand.push(process.env.CIAN_ELECTRON_DIST);
    for (const d of [path.join(ROOT, '..'), ROOT]) {
        if (!fs.existsSync(d)) continue;
        for (const name of fs.readdirSync(d)) {
            if (name.startsWith('electron-v')) cand.push(path.join(d, name));
        }
    }
    cand.push(path.join(__dirname, 'node_modules', 'electron', 'dist'));
    for (const c of cand) {
        if (platform === 'darwin' && fs.existsSync(path.join(c, 'Electron.app'))) return c;
        if (platform === 'win32' && fs.existsSync(path.join(c, 'electron.exe'))) return c;
        if (platform === 'linux' && fs.existsSync(path.join(c, 'electron'))) return c;
    }
    return null;
}

/// エンジン。同梱版は**リポジトリの target を見に行けない**ので、ここで
/// 入れておかないと開いた瞬間に「cian-server not found」になる。
function engineFile(platform) {
    const exe = platform === 'win32' ? 'cian-server.exe' : 'cian-server';
    const given = arg('engine');
    if (given && given !== true) return given;
    for (const profile of ['release', 'debug']) {
        const p = path.join(ROOT, 'target', profile, exe);
        if (fs.existsSync(p)) return p;
    }
    return null;
}

/// アプリ（`resources/app` の中身）を置く。フルでも `--app-only` でもここを通る。
function layApp(appDir, platform) {
    fs.rmSync(appDir, { recursive: true, force: true });
    copyDir(__dirname, appDir, SKIP_IN_GUI);
    const engine = engineFile(platform);
    if (engine) {
        const to = path.join(appDir, path.basename(engine));
        fs.copyFileSync(engine, to);
        if (platform !== 'win32') fs.chmodSync(to, 0o755);
        console.log(`  + ${path.basename(engine)}（${mb(fs.statSync(engine).size)}）`);
    }
    // 窓の絵。`main.js` は**自分の隣**を先に見るので、同梱版ではここに要る。
    for (const n of ['cian.ico', 'cian.png']) {
        const from = path.join(ROOT, n);
        if (fs.existsSync(from)) fs.copyFileSync(from, path.join(appDir, n));
    }
    // 同梱版の package.json は `main` さえ正しければいい。devDependencies を
    // 残すと、中を見た人が「npm install が要るのか」と読む。
    const pkg = JSON.parse(fs.readFileSync(path.join(appDir, 'package.json'), 'utf8'));
    delete pkg.devDependencies;
    delete pkg.dependencies;
    delete pkg.scripts;
    pkg.version = version;
    fs.writeFileSync(path.join(appDir, 'package.json'), JSON.stringify(pkg, null, 2) + '\n');
}

/// **出口の検証。** 欠けたまま配ると、気づくのは向こうの机の上になる。
/// `vendor.js` は写せなくても続けるので、フォントが抜けた 6MB 軽い zip が
/// 一度そのまま出ている（2026-09-06）。だから「無ければ落とす」をここに置く。
function checkOut(target, appDir, platform) {
    const bad = [];
    const need = ['main.js', 'renderer.js', 'index.html', 'engine.js', 'preload.js',
                  'vendor/monaco/vs/loader.js', 'vendor/fonts/cian.ttf'];
    for (const n of need) {
        if (!fs.existsSync(path.join(appDir, n))) bad.push(`resources/app/${n} が無い`);
    }
    const exe = platform === 'win32' ? 'cian-server.exe' : 'cian-server';
    if (!fs.existsSync(path.join(appDir, exe))) bad.push(`エンジン（${exe}）が無い`);
    const launcher = platform === 'win32' ? path.join(target, `${NAME}.exe`)
                                          : path.join(target, `${NAME}.app`);
    if (!fs.existsSync(launcher)) bad.push(`${path.basename(launcher)} が無い`);
    if (fs.existsSync(path.join(appDir, '..', 'default_app.asar'))) {
        bad.push('resources/default_app.asar が残っている（Electron の既定画面が開く）');
    }
    // **exe の隣の .lua**。これがあると、しおりの保存先がこのフォルダになる。
    for (const n of fs.readdirSync(appDir)) {
        if (n.endsWith('.lua')) bad.push(`resources/app/${n} ── 設定を exe の隣に置かない`);
    }
    if (bad.length) {
        console.error('\nNG:');
        for (const b of bad) console.error(`  - ${b}`);
        process.exit(1);
    }
}

function packWindows(target, dist) {
    fs.mkdirSync(target, { recursive: true });
    for (const name of fs.readdirSync(dist)) {
        if (name === 'resources') {
            // 既定の app（default_app.asar）は入れない ── 残すと、こちらの
            // resources/app より先に読まれて Electron の案内画面が開く。
            for (const r of fs.readdirSync(path.join(dist, 'resources'))) {
                if (r === 'app' || r === 'default_app.asar') continue;
                const s = path.join(dist, 'resources', r);
                const d = path.join(target, 'resources', r);
                if (fs.statSync(s).isDirectory()) copyDir(s, d);
                else fs.copyFileSync(s, d);
            }
            continue;
        }
        const s = path.join(dist, name);
        const d = path.join(target, name);
        if (fs.statSync(s).isDirectory()) copyDir(s, d);
        else fs.copyFileSync(s, d);
    }
    const from = path.join(target, 'electron.exe');
    if (fs.existsSync(from)) {
        fs.renameSync(from, path.join(target, `${NAME}.exe`));
        console.log(`  + ${NAME}.exe`);
    }
    return path.join(target, 'resources', 'app');
}

/// exe のアイコンは PE リソースなので、差し替えに `rcedit` が要る（GitHub の
/// electron/rcedit のリリース資産、1MB ほどの単体 exe）。crmaine も同じ道具で
/// 焼いている。**無くても配布はできる** ── 窓とタスクバーは `main.js` の
/// `BrowserWindow` の icon で cian になり、エクスプローラ上の exe の絵だけが
/// Electron のままになる。
function burnIcon(target) {
    const rcedit = arg('rcedit');
    const ico = path.join(ROOT, 'cian.ico');
    const exe = path.join(target, `${NAME}.exe`);
    if (!(rcedit && rcedit !== true) || !fs.existsSync(exe)) {
        console.log('  - exe のアイコンは Electron のまま（--rcedit <rcedit.exe> で焼ける）');
        return;
    }
    if (!fs.existsSync(ico)) { console.log('  - cian.ico が無い'); return; }
    try {
        execFileSync(rcedit, [exe,
            '--set-icon', ico,
            '--set-version-string', 'ProductName', 'cian',
            '--set-version-string', 'FileDescription', 'cian — two-pane file manager',
            '--set-version-string', 'CompanyName', 'cian',
            '--set-file-version', version,
            '--set-product-version', version,
        ], { stdio: 'inherit' });
        console.log('  + exe のアイコンと名前を焼いた（rcedit）');
    } catch (e) {
        console.error(`NG: rcedit に失敗: ${e.message}`);
        process.exit(1);
    }
}

function packMac(target, dist) {
    const app = path.join(target, `${NAME}.app`);
    fs.mkdirSync(target, { recursive: true });
    fs.rmSync(app, { recursive: true, force: true });
    copyDir(path.join(dist, 'Electron.app'), app);
    // 中身を変えたあとの古い署名は「壊れている」と読まれる。
    fs.rmSync(path.join(app, 'Contents', '_CodeSignature'), { recursive: true, force: true });
    const macos = path.join(app, 'Contents', 'MacOS');
    if (fs.existsSync(path.join(macos, 'Electron'))) {
        fs.renameSync(path.join(macos, 'Electron'), path.join(macos, NAME));
    }
    const res = path.join(app, 'Contents', 'Resources');
    fs.rmSync(path.join(res, 'default_app.asar'), { force: true });
    const plist = path.join(app, 'Contents', 'Info.plist');
    const set = (k, v) => {
        try {
            execFileSync('/usr/libexec/PlistBuddy', ['-c', `Set :${k} ${v}`, plist], { stdio: 'ignore' });
        } catch {
            execFileSync('/usr/libexec/PlistBuddy', ['-c', `Add :${k} string ${v}`, plist], { stdio: 'ignore' });
        }
    };
    set('CFBundleName', NAME);
    set('CFBundleDisplayName', NAME);
    set('CFBundleExecutable', NAME);
    // **バンドル ID は bundle-gui.sh と同じものにする。** 別の ID を名乗ると
    // 設定も権限も別のアプリとして扱われる。
    set('CFBundleIdentifier', 'com.taketan.cian');
    set('CFBundleShortVersionString', version);
    const icns = path.join(ROOT, 'packaging', 'macos', 'icns.py');
    if (fs.existsSync(icns)) {
        execFileSync('python3', [icns, path.join(res, 'cian.icns')], { stdio: 'inherit' });
        fs.rmSync(path.join(res, 'electron.icns'), { force: true });
        set('CFBundleIconFile', 'cian.icns');
    }
    console.log(`  + ${NAME}.app`);
    return path.join(res, 'app');
}

/// その場で作った .app が「壊れている」と言われないための署名。配布用でも
/// 公証でもない ── それには Apple の証明書が要る。**中身を全部置いたあとで
/// 掛ける**（署名してから差し替えると、別のものを指す署名が残り、macOS は
/// それを破損と読む）。`packaging/macos/bundle-gui.sh` が同じことをしている。
function signMac(target) {
    const app = path.join(target, `${NAME}.app`);
    try {
        execFileSync('codesign', ['--force', '--deep', '--sign', '-', app], { stdio: 'ignore' });
        console.log('  + 署名した（ad-hoc）');
    } catch {
        console.log('  - 署名できなかった（動きますが、初回に警告が出ることがあります）');
    }
}

/// 組み立てたものを `/Applications` に置いて、Dock にも並べる（`--install`）。
///
/// **ここに置く .app は「その日の cian」になる。** 中身を持たずリポジトリを
/// 読む薄い .app が欲しいときは `packaging/macos/bundle-gui.sh --dock` の方。
/// 二つは同じ場所を取り合うので、後から掛けた方が残る。
function installMac(target, to) {
    const app = path.join(target, `${NAME}.app`);
    if (fs.existsSync(to)) {
        console.log(`  - 置き換えます: ${to}`);
        fs.rmSync(to, { recursive: true, force: true });
    }
    fs.mkdirSync(path.dirname(to), { recursive: true });
    execFileSync('/bin/cp', ['-R', app, to]);
    const lsregister = '/System/Library/Frameworks/CoreServices.framework'
        + '/Frameworks/LaunchServices.framework/Support/lsregister';
    if (fs.existsSync(lsregister)) {
        try { execFileSync(lsregister, ['-f', to], { stdio: 'ignore' }); } catch {}
    }
    console.log(`  + ${to}`);
    if (!has('dock')) return;
    // **`killall Dock` は Dock がまだ書いていない分を捨てる。** 書くのは
    // `defaults`、読むのは起動時の Dock で、間に cfprefsd の写しが挟まる。
    // だから入れたあとに読み直して、入っているかを確かめる（2026-09-10）。
    const inDock = () => {
        try {
            const out = execFileSync('/bin/sh', ['-c',
                'defaults export com.apple.dock - 2>/dev/null | plutil -p - 2>/dev/null'],
                { encoding: 'utf8' });
            return out.includes(path.basename(to));
        } catch { return false; }
    };
    if (inDock()) { console.log('  = Dock には既に居ます'); return; }
    execFileSync('defaults', ['write', 'com.apple.dock', 'persistent-apps', '-array-add',
        `<dict><key>tile-data</key><dict><key>file-data</key><dict>`
        + `<key>_CFURLString</key><string>${to}</string>`
        + `<key>_CFURLStringType</key><integer>0</integer></dict></dict></dict>`]);
    execFileSync('killall', ['Dock']);
    execFileSync('/bin/sh', ['-c', 'sleep 3']);
    console.log(inDock() ? '  + Dock に置きました'
                         : '  - Dock に入りませんでした（Dock を並べ替えている最中だと消えます。もう一度）');
}

function main() {
    if (has('where')) {
        console.log('入れるもの:');
        console.log(`  gui/（${[...SKIP_IN_GUI].join(' と ')} を除く）`);
        for (const [from, to] of topFiles(arg('platform', process.platform))) {
            console.log(`  ${from}  →  ${to}`);
        }
        console.log('  target/{release,debug}/cian-server[.exe]');
        return;
    }
    const out = arg('out');
    if (!out || out === true) {
        console.error('使い方: node gui/pack.js --out <出力先> [--platform win32|darwin]');
        console.error('        [--electron <展開先>] [--engine <cian-server>] [--rcedit <rcedit.exe>]');
        console.error('        [--app-only] [--zip] [--install [先.app]] [--dock]');
        console.error('        --where で中身だけ表示');
        process.exit(2);
    }
    const platform = arg('platform', process.platform);
    const target = path.join(out, NAME);
    const appOnly = has('app-only');

    console.log(`cian ${version} を ${platform} 向けに組み立てます → ${target}`);

    if (appOnly) {
        // 更新用。**Electron には触らない** ── 触らないから数十MB で済む。
        const appDir = platform === 'darwin'
            ? path.join(target, `${NAME}.app`, 'Contents', 'Resources', 'app')
            : path.join(target, 'resources', 'app');
        if (!fs.existsSync(path.dirname(appDir))) {
            console.error(`NG: ${path.dirname(appDir)} がありません（先にフルで組んでください）`);
            process.exit(1);
        }
        layApp(appDir, platform);
        checkOut(target, appDir, platform);
        console.log(`できました（アプリだけ・${mb(dirSize(appDir))}）`);
        return;
    }

    const dist = findElectron(platform);
    if (!dist) {
        console.error(`NG: ${platform} の Electron が見つかりません。`);
        console.error('  --electron <展開先> で渡すか、gui で npm install してください。');
        process.exit(1);
    }
    console.log(`Electron: ${dist}`);
    fs.rmSync(target, { recursive: true, force: true });
    const appDir = platform === 'darwin' ? packMac(target, dist) : packWindows(target, dist);
    layApp(appDir, platform);
    if (platform === 'win32') burnIcon(target);
    for (const [from, to] of topFiles(platform)) {
        const d = path.join(target, to);
        fs.mkdirSync(path.dirname(d), { recursive: true });
        fs.copyFileSync(path.join(ROOT, from), d);
    }
    // 共有マシン用の置き場。**空で入れる** ── ここに .lua を入れて配ると、
    // 初回だけ各人の ~/.config/cian に写される（写すのは main.js）。
    fs.mkdirSync(path.join(target, 'default-config'), { recursive: true });
    checkOut(target, appDir, platform);
    if (platform === 'darwin') signMac(target);
    console.log(`できました（${mb(dirSize(target))}）`);

    if (platform === 'darwin' && has('install')) {
        const to = arg('install');
        installMac(target, to && to !== true ? to : `/Applications/${NAME}.app`);
    }

    if (has('zip')) {
        const zip = `${target}-${platform === 'win32' ? 'win-x64' : 'macos'}.zip`;
        fs.rmSync(zip, { force: true });
        if (platform === 'darwin' || os.platform() !== 'win32') {
            execFileSync('/usr/bin/ditto', ['-c', '-k', '--sequesterRsrc', '--keepParent', target, zip]);
        } else {
            execFileSync('powershell', ['-c', `Compress-Archive -Path "${target}" -DestinationPath "${zip}" -Force`]);
        }
        console.log(`  + ${path.basename(zip)}（${mb(fs.statSync(zip).size)}）`);
    }
}

main();
