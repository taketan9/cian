#!/bin/sh
# 窓版 cian を macOS の .app にして、Dock に置けるようにする。
#
# **中身は入れない。** 入れた日から古くなるからで、隣の amber の .app が
# その形になっている ── `gui/` を丸ごと写したので、写した日のメモが一枚
# 紛れ込んだまま今も入っている。ここに置くのは Electron 本体と、リポジトリの
# `gui/main.js` を読む数行だけ。直したものが次に開いたときに出る、というのが
# 「常に最新の cian」の中身で、そのために .app は薄い。
#
# エンジンも同じ考えで、起動のたびに `cargo build` を通す。何も変えていなければ
# 一瞬で、変えていれば新しい方が動く（`gui/engine.js` は release と debug の
# **新しい方**を選ぶ）。
#
#   packaging/macos/bundle-gui.sh [--dock] [出力先.app]
#
# 出力先の既定は /Applications/cian.app。--dock を付けると Dock にも並べる。
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
dock=no
app=

for a in "$@"; do
    case "$a" in
        --dock) dock=yes ;;
        -*)     echo "知らない引数: $a" >&2; exit 2 ;;
        *)      app=$a ;;
    esac
done
app=${app:-/Applications/cian.app}
case "$app" in /*) ;; *) app=$(pwd)/$app ;; esac   # Dock も plist も絶対パスしか見ない

# Electron 本体。run.sh と同じ順で探す ── 環境変数、リポジトリの隣の配布版、
# npm で入れたもの。
electron=
if [ -n "${CIAN_ELECTRON_DIST:-}" ] && [ -d "${CIAN_ELECTRON_DIST}/Electron.app" ]; then
    electron=$CIAN_ELECTRON_DIST/Electron.app
fi
if [ -z "$electron" ]; then
    for d in "$root"/../electron-v* "$root/gui/node_modules/electron/dist"; do
        [ -d "$d/Electron.app" ] && electron=$d/Electron.app && break
    done
fi
[ -n "$electron" ] || {
    echo "Electron が見つかりません: (cd gui && npm install)" >&2
    exit 1
}

# 開いた瞬間に困るものは、ここで言う。どちらも .app の欠陥ではないので止めない。
[ -f "$root/gui/vendor/monaco/vs/loader.js" ] \
    || echo "注意: gui/vendor がありません（エディタは開けません）: node gui/vendor.js" >&2
ls "$root"/target/*/cian-server >/dev/null 2>&1 \
    || echo "注意: cian-server がまだありません。初回の起動でビルドします（数分かかります）" >&2

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
work=$tmp/cian.app

echo "Electron: $electron"
cp -R "$electron" "$work"
rm -rf "$work/Contents/_CodeSignature"   # 中身を変えたあとの古い署名は「壊れている」と読まれる

# 名前とアイコン。Electron の plist を土台に、cian のものだけ書き換える。
# CFBundleVersion は Electron のまま ── これは実体の版で、cian の版ではない。
plist=$work/Contents/Info.plist
set_key() {
    /usr/libexec/PlistBuddy -c "Set :$1 $2" "$plist" >/dev/null 2>&1 \
        || /usr/libexec/PlistBuddy -c "Add :$1 string $2" "$plist" >/dev/null
}
set_key CFBundleName cian
set_key CFBundleDisplayName cian
set_key CFBundleExecutable cian
set_key CFBundleIdentifier com.taketan.cian
set_key CFBundleIconFile cian.icns
set_key CFBundleShortVersionString "$version"

python3 "$root/packaging/macos/icns.py" "$work/Contents/Resources/cian.icns"
rm -f "$work/Contents/Resources/electron.icns"

# Dock は最前面のアプリを**実行ファイルの居場所**から決める。だから exec する
# Electron は同じ .app の中でなければならない ── 外の Electron を exec すると、
# Dock に出るのは Electron の名前と Electron のアイコンになる。
cat > "$work/Contents/MacOS/cian" <<LAUNCH
#!/bin/zsh
# cian の入口。Dock も Finder もここを叩く。
#
# Dock から開くと PATH は最小限（/usr/bin:/bin:…）で、cargo はそこに無い。
PATH=/usr/local/bin:/opt/homebrew/bin:\$PATH

# エンジンを最新にしてから開く。変えていなければ一瞬で終わる。
#
# **失敗しても開く** ── ファイラーが開かないよりは、古いエンジンで開いた方が
# ましなので。ただし**黙らせない**: 落ちた理由はここに残る。「直したのに効かない」の
# 答えがどこにも無い、というのを何度もやっている。
log=\${HOME}/Library/Logs/cian-launch.log
{
    echo "--- \$(date '+%Y-%m-%d %H:%M:%S')  $root"
    cd "$root" && cargo build --release -p cian-server
} > "\$log" 2>&1 || echo "cargo build が失敗しました（古いエンジンで開きます）: \$log" >> "\$log"

exec "\$0:A:h/Electron" "\$@"
LAUNCH
chmod +x "$work/Contents/MacOS/cian"

# Electron が読むのはここ。本体を指すだけの数行を置く。
mkdir -p "$work/Contents/Resources/app"
cat > "$work/Contents/Resources/app/package.json" <<JSON
{
  "name": "cian",
  "version": "$version",
  "private": true,
  "main": "main.js"
}
JSON
cat > "$work/Contents/Resources/app/main.js" <<JS
// 本体はここに無い。リポジトリの gui/ を読む ── そこを直せば、次に開いたときに出る。
// \`__dirname\` を使う道（index.html・preload.js・cian.png・engine.js）は require が
// 解決した先、つまりリポジトリの gui/ を向くので、写さなくても全部つながる。
const fs = require('node:fs');
const body = '$root/gui/main.js';

if (fs.existsSync(body)) {
    require(body);
} else {
    // 窓が出ないまま黙って終わるのが一番わかりにくい。理由を出してから終わる。
    const { app, dialog } = require('electron');
    app.whenReady().then(() => {
        dialog.showErrorBox('cian', '本体が見つかりません:\n' + body
            + '\n\nリポジトリを動かしたときは、packaging/macos/bundle-gui.sh を掛け直してください。');
        app.quit();
    });
}
JS

# その場で作った .app が「壊れている」と言われないための署名。配布用の署名では
# なく、公証でもない ── それには Apple の証明書が要る。作った機械で動かす分には
# これで足りる。**中身を全部置いたあとで署名する**（署名してから差し替えると、
# 別のものを指す署名が残り、macOS はそれを破損と読む）。
codesign --force --deep --sign - "$work" >/dev/null 2>&1 \
    || echo "注意: 署名できませんでした（動きますが、初回に警告が出ることがあります）" >&2

rm -rf "$app"
mkdir -p "$(dirname "$app")"
mv "$work" "$app" 2>/dev/null || { cp -R "$work" "$app" && rm -rf "$work"; }
trap - EXIT
rm -rf "$tmp"

lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
[ -x "$lsregister" ] && "$lsregister" -f "$app" >/dev/null 2>&1

echo "built $app  (cian $version)"

# Dock に並べる。
#
# **`killall Dock` は Dock がまだ書いていない分を捨てる。** 書くのは `defaults`、
# 読むのは起動時の Dock、という二人がいて、間に cfprefsd の写しが挟まる ──
# `defaults read` と `~/Library/Preferences/com.apple.dock.plist` が食い違って
# いるのを実際に見た（2026-09-10）。並べ替えている最中に掛けると、その並べ替えが
# 消える。だから**入れたあとに読み直して、入っているかを確かめる**。
if [ "$dock" = yes ]; then
    in_dock() {
        defaults export com.apple.dock - 2>/dev/null \
            | plutil -p - 2>/dev/null \
            | grep -q "$(basename "$app")"
    }
    if in_dock; then
        echo "Dock には既に居ます"
    else
        defaults write com.apple.dock persistent-apps -array-add "<dict><key>tile-data</key><dict><key>file-data</key><dict><key>_CFURLString</key><string>$app</string><key>_CFURLStringType</key><integer>0</integer></dict></dict></dict>"
        killall Dock
        sleep 3
        if in_dock; then
            echo "Dock に置きました"
        else
            echo "Dock に置けませんでした（Dock が並びを書き戻した可能性）。もう一度掛けてください" >&2
            exit 1
        fi
    fi
fi
