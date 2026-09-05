"""いつものチェック — 冗長・死にロジック・名前の散らかりを機械で洗う。

    python3 scripts/audit.py            # 全部
    python3 scripts/audit.py dead       # 死にコードだけ（dup / naming も同様）

**目視でやらない。** 3万行を人が読み返すのは無理で、読み返したつもりに
なるのが一番危ない。毎回同じ物差しで測れるように道具にしてある。

**コンパイラが見ているものは見ない。** Rust の `dead_code` は、そのクレートの
中で誰にも呼ばれない非公開の項目を自分で見つけるし、cian は
`cargo clippy -D warnings` が緑の状態で保たれている。ここが見るのは
**コンパイラの目が届かないところ** ―― 誰も使っていない `pub`、握り潰した
`#[allow(dead_code)]`、そして「関数としては呼ばれているが、人間が辿り着けない」
メニュー項目やコマンド。実際に死んでいたのは毎回そこだった。

出るのは「候補」であって「誤り」ではない。判断は人（と AI）がする。
"""
from __future__ import annotations

import collections
import difflib
import glob
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

def _members() -> list:
    """ワークスペースが持っているクレート全部。

    **手書きの一覧にしない。** ここは長らく9個の名前を並べていて、10個目
    （cian-ffi）を足したとき、その中身は死にコードも用語のブレも一切
    見られないまま素通りした。**検査は足したときだけでなく、増えたときにも
    黙る。**members から取れば、次のクレートは黙って外れようがない。
    """
    with open(os.path.join(ROOT, 'Cargo.toml'), encoding='utf-8') as fh:
        src = fh.read()
    block = re.search(r'members\s*=\s*\[(.*?)\]', src, re.S)
    names = re.findall(r'"crates/([^"]+)"', block.group(1) if block else '')
    if len(names) < 5:
        raise SystemExit('audit: Cargo.toml から members を読めませんでした')
    return sorted(names)


CRATES = _members()
#: 本体のソース。tests.rs は読み手が違う（テストが唯一の利用者でも死んではいない）
SRC = sorted(p for c in CRATES
             for p in glob.glob(os.path.join(ROOT, 'crates', c, 'src', '**', '*.rs'),
                                recursive=True)
             if os.path.basename(p) != 'tests.rs')
TESTS = sorted(glob.glob(os.path.join(ROOT, 'crates', '*', 'src', 'tests.rs'))
               + glob.glob(os.path.join(ROOT, 'crates', '*', 'tests', '*.rs')))


def _read(p: str) -> str:
    with open(p, encoding='utf-8') as f:
        return f.read()


def _rel(p: str) -> str:
    return os.path.relpath(p, ROOT)


def _strip(src: str) -> str:
    """コメントと文字列を落とす。**名前を数えるのに散文を混ぜない。**

    文字リテラルを先に落とすこと。`'"'` の中の引用符が次の引用符と対になり、
    **その間のコードごと消える** ―― cian は `'"'` を実際に使っているので、
    これを忘れた版は「使われている」を大量に取りこぼした。
    """
    src = re.sub(r"'(?:[^'\\]|\\.)'", "'.'", src)
    src = re.sub(r'//[^\n]*', '', src)
    src = re.sub(r'/\*[\s\S]*?\*/', '', src)
    return re.sub(r'"(?:[^"\\\n]|\\.)*"', '""', src)


def _body(src: str, at: int) -> str:
    """`at` から始まる項目の本体を、波括弧の対応で取り出す。"""
    i = src.find('{', at)
    if i < 0:
        return ''
    depth, j = 0, i
    while j < len(src):
        if src[j] == '{':
            depth += 1
        elif src[j] == '}':
            depth -= 1
            if depth == 0:
                return src[i:j + 1]
        j += 1
    return src[i:]


FN = re.compile(r'^[ \t]*(?:pub(?:\([^)]*\))? )?(?:const |async |unsafe )*fn '
                r'([a-z_][a-z0-9_]*)', re.M)


def _functions(src: str):
    """(名前, 行, 本体) を返す。"""
    for m in FN.finditer(src):
        yield m.group(1), src[:m.start()].count('\n') + 1, _body(src, m.end())


# ── ① 死にコード ────────────────────────────────
def dead() -> int:
    print('=' * 72)
    print('① 死にコード（コンパイラに見えないもの）')
    print('=' * 72)
    found = 0
    all_src = {p: _read(p) for p in SRC}
    all_test = ''.join(_read(p) for p in TESTS)
    stripped = {p: _strip(s) for p, s in all_src.items()}
    everything = '\n'.join(stripped.values()) + '\n' + _strip(all_test)

    # (a) 誰も使っていない `pub`。dead_code は pub には黙っているので、
    #     ワークスペースの誰も呼んでいない公開 API はここでしか出てこない。
    print('  ■ ワークスペースの誰も使っていない pub')
    pub = re.compile(r'^[ \t]*pub (?:const |async |unsafe )*'
                     r'(fn|struct|enum|trait|const|type) ([A-Za-z_][A-Za-z0-9_]*)', re.M)
    for p, src in all_src.items():
        # バイナリクレートの pub は外から呼ばれない前提なので数えない
        if '/cian-bin/' in p or '/cian-gui/' in p:
            continue
        for m in pub.finditer(src):
            kind, name = m.group(1), m.group(2)
            if name.startswith('_') or name == 'main':
                continue
            # **ドットを除かない。** Rust の呼び出しは `p.target_paths()` で、
            # 除いた版は「使われている」を全部取りこぼした
            uses = len(re.findall(r'(?<![\w])' + re.escape(name) + r'(?![\w])',
                                  everything))
            if uses <= 1:
                line = src[:m.start()].count('\n') + 1
                print(f'    ★ {_rel(p)}:{line} pub {kind} {name}')
                found += 1

    # (b) 握り潰した dead_code。**1つ1つが「なぜ残すのか」の借金。**
    print('  ■ #[allow(dead_code)]（残す理由が要る）')
    for p, src in all_src.items():
        for m in re.finditer(r'#\[allow\([^)]*dead_code[^)]*\)\]', src):
            line = src[:m.start()].count('\n') + 1
            nxt = src[m.end():m.end() + 120].strip().splitlines()
            what = nxt[0][:60] if nxt else ''
            if any(k in what for k in DEAD_OK):
                continue
            print(f'    ★ {_rel(p)}:{line} → {what}')
            found += 1

    # (c) 人間が辿り着けないもの。**関数としては生きていても、押す道がなければ
    #     死んでいる。** リリース直前に、パレットが存在しないコマンドを5つ
    #     出していたのが実際にこれ。
    print('  ■ 辿り着けないメニュー項目・コマンド')
    lib = stripped.get(os.path.join(ROOT, 'crates', 'cian-tui', 'src', 'lib.rs'), '')
    menu = _body(lib, lib.find('enum MenuItem'))
    reachable = '\n'.join(v for k, v in stripped.items() if not k.endswith('lib.rs'))
    for m in re.finditer(r'^\s*([A-Z][A-Za-z0-9]*)[,({]', menu, re.M):
        name = m.group(1)
        if not re.search(r'MenuItem::' + re.escape(name) + r'\b', reachable):
            print(f'    ★ MenuItem::{name} — どのメニューにも入っていない')
            found += 1

    # **腕ごとに見る。別名は文書に載らなくてよい。** `:cp | :copy` の
    # `:copy` を「文書に無い」と言い出すと、指摘が全部ノイズになる ―― 実際
    # 名前単位で見た最初の版は67件を出し、そのほとんどが別名だった。
    # 一つも載っていない腕だけが、押す道の無いコマンド。
    pal = set(re.findall(r'^\s*\("([a-z][a-z0-9]*)",',
                         _read(os.path.join(ROOT, 'crates', 'cian-tui', 'src',
                                            'palette.rs')), re.M))
    manual = _read(os.path.join(ROOT, 'crates', 'cian-tui', 'src', 'lib.rs'))
    readme = _read(os.path.join(ROOT, 'README.md')) + _read(
        os.path.join(ROOT, 'README.ja.md'))
    for arm in _arms():
        if any(v in pal or f':{v}' in readme
               or re.search(r'[:`]' + re.escape(v) + r'\b', manual) for v in arm):
            continue
        print(f'    ★ :{" / :".join(sorted(arm))} — どの名前も文書に無い')
        found += 1

    print(f'  → {found} 件' if found else '  なし')
    return found


def _arms() -> list[set[str]]:
    """`:` コマンドを match の腕ごとに。同じ腕の名前は互いの別名。

    **一番外側の腕だけ。** `:editstyle vim|notepad` の引数も同じ形をしていて、
    数えると「`:notepad` が文書に無い」を二重に言い出す。深さは字下げで分かり、
    最小の字下げがコマンド本体の段。
    """
    out: list[set[str]] = []
    arm = re.compile(r'^([ \t]*)((?:"[a-z][a-z0-9.]*"(?: \| )?)+) =>', re.M)
    for f in ('commands.rs', 'viewer.rs'):
        src = _read(os.path.join(ROOT, 'crates', 'cian-tui', 'src', f))
        hits = [(len(m.group(1)), m.group(2)) for m in arm.finditer(src)]
        if not hits:
            continue
        top = min(i for i, _ in hits)
        out += [set(re.findall(r'"([a-z][a-z0-9.]*)"', names))
                for i, names in hits if i == top]
    return out


def _verbs() -> set[str]:
    """`:` コマンドの名前、全部。"""
    return {v for arm in _arms() for v in arm}


#: 似ているが**分けたままでよい**関数（理由つき）。
#
# ここに書くのは「重複を見逃す」ためではなく、**なぜ括らないか**を残すため。
# 括った方が高くつく組がある。
DUP_OK = {
    # 前後の鏡像。1本にすると「どちらのメソッドを呼ぶか」「矢印」「メッセージ」
    # で `if back` が3つ入る。10行の読める関数2本が、13行の分岐だらけ1本に
    # 化けるだけで、読む側は毎回どちらの方向の話かを追うことになる
    ('pane_go_back', 'pane_go_forward'),
    # 共有していた「返答から JSON 配列を切り出す」7行は `json_array` に出した。
    # 残る類似は、別々の構造体を別々の項目に組み立てている部分そのもので、
    # 括るには型を1つにするしかない ―― 片方は移動先を、もう片方は理由だけを
    # 持つので、1つにした型は常にどちらかの欄が空になる
    ('parse_junk_reply', 'parse_structure_reply'),
}


#: 4回以上出ても**それでよい行**（理由つき）。
#
# 同じ言い回しを同じ意味で使っているだけのものは、括ると読めなくなる。
LINE_OK = {
    # 画面に出る文言。`self.say(en, ja)` に括れば全部1行短くなるが、
    # **何と表示されるかがその場で読めなくなる**。文言はコードより頻繁に
    # 直すので、直す人が探す場所に置いておく
    'self.message = Some(tr(',
    # 別々のポップアップが、同じキーに同じ意味を与えているだけ。1つの
    # ハンドラに束ねるには、中身の違うリストを1つの型にするしかない
    "KeyCode::Char('k') | KeyCode::Up",
    'let base = if sel {',
    # 転送ごとに違うクロージャを載せる制御構造体。共有できるのは名前だけ
    'cian_scp::Ctl {',
    # エラーに文脈を付ける定型。付け忘れた1箇所が「No such file」とだけ
    # 言うので、**書いてあること自体に意味がある**
    '.with_context(|| format!(',
    # 選択範囲の矩形。オペレータごとに anchor が違う
    'cian_core::textops::Block::between(',
    # 転送の入口。4つの操作が同じ一文で SFTP を開くだけ
    'open_sftp(&handle)',
    # パスから表示名を取る定型
    'path.file_name().map(|s| s.to_string_lossy()',
    # レビュー行のチェックボックスの色。4つのレビューが同じ見た目を持つ
    'let box_c = if it.selected {',
    # リモートペインの現在地。左右それぞれで前後を見るので4回出る
    '.remote_view().map(|(_, p)|',
}


#: 握り潰したままでよい dead_code（理由つき）
DEAD_OK = {
    # **死んでいない。** Lua ランタイムを app の生存期間ぶん生かしておくためだけ
    # の保持で、読まないことが役目。落とすと ext_open のハンドルが道連れになる
    '_lua: Option<Lua>',
}


# ── ② 重複・冗長 ────────────────────────────────
def dup() -> int:
    print()
    print('=' * 72)
    print('② 重複（中身が似ている関数 / 同じ行の繰り返し）')
    print('=' * 72)
    found = 0
    for p in SRC:
        src = _read(p)
        # テストは互いに似ていて当然（同じ段取りを条件だけ変えて並べる）ので
        # 落とす。**列0のモジュールだけを切ること。** どこにでもある
        # `#[cfg(test)]` を探した版は、テスト専用ヘルパに付いた字下げ済みの
        # 1つを拾って soft.rs の3分の2を監査から消していた ―― 指摘が減るので
        # 「きれいになった」に見える
        m = re.search(r'^#\[cfg\(test\)\]', src, re.M)
        if m:
            src = src[:m.start()]
        funcs = {}
        for name, line, body in _functions(src):
            # テスト専用のヘルパは、本体の関数を真似て作るので似ていて当然
            head = src.splitlines()[max(0, line - 3):line - 1]
            if any('#[cfg(test)]' in h for h in head):
                continue
            norm = re.sub(r'//[^\n]*', '', body)
            norm = re.sub(r'\s+', ' ', norm).strip()
            if len(norm) > 300:
                funcs[name] = (norm, line, body.count('\n') + 1)
        names = sorted(funcs)
        for i, a in enumerate(names):
            for b in names[i + 1:]:
                # 入れ子の関数は本体を含むので似ていて当然
                if (funcs[a][1] <= funcs[b][1] <= funcs[a][1] + funcs[a][2]
                        or funcs[b][1] <= funcs[a][1] <= funcs[b][1] + funcs[b][2]):
                    continue
                s = difflib.SequenceMatcher(None, funcs[a][0], funcs[b][0])
                if s.quick_ratio() < 0.80:
                    continue
                r = s.ratio()
                if r >= 0.80 and (a, b) not in DUP_OK and (b, a) not in DUP_OK:
                    print(f'  ★ {r:.2f} {_rel(p)}: {a}:{funcs[a][1]} ↔ {b}:{funcs[b][1]}')
                    found += 1

    # 同じ行が何度も出てくる（コピペの跡）
    c = collections.Counter(
        l.strip() for p in SRC for l in _read(p).splitlines()
        if len(l.strip()) > 70 and not l.strip().startswith(('//', '///', '*')))
    # **総数を先に言う。** 上位8件だけ出していた版は、潰しても下位が繰り上がって
    # 件数が動かず、「進んでいない」ように見えた。多いのは省くが、いくつ省いたかは
    # 言う ―― 黙って切ると「これで全部」に読める
    over = [(n, l) for l, n in c.items()
            if n >= 4 and not any(k in l for k in LINE_OK)]
    over.sort(reverse=True)
    for n, line in over[:10]:
        print(f'  ★ 同じ行が {n} 回  {line[:78]}')
    if len(over) > 10:
        print(f'    …ほか {len(over) - 10} 種類')
    found += len(over)
    print(f'  → {found} 件' if found else '  なし')
    return found


# ── ③ 名前の散らかり ──────────────────────────────
#
# **同じものを別の名前で呼んでいないか。** 呼び分けが意図的でないなら、
# 読む側は毎回「どちらだったか」を考えることになる。
TERM_FAMILIES = {
    'ペイン':     ['ペイン', '枠'],
    'フォルダ':   ['フォルダー', 'フォルダ', 'ディレクトリ', 'ノートブック'],
    'マーク':     ['マーク', '選択'],
    'ビューア':   ['ビューア', 'エディタ', 'パネル'],
    'シェル':     ['シェル', '端末', 'ターミナル'],
    '書庫':       ['書庫', 'アーカイブ'],
    '取り消し':   ['取り消し', '元に戻す', 'アンドゥ'],
    '一覧':       ['一覧', 'リスト'],
    '設定':       ['設定', 'コンフィグ'],
    # 2026-09-05 に数えて出てきたもの。同じ画面の中で見出しが「ノートの
    # 置き場所」、ボタンが「保存場所を選ぶ」になっていた
    '保存場所':   ['保存場所', '置き場所', '保存先'],
    'タイトル':   ['タイトル', '題名'],
    '貼り付け':   ['貼り付け', '貼付'],
    '削除':       ['削除', '消去'],
    '作る':       ['新しい', '新規', '作成', '作る'],
}

#: 使い分けているもの（意図があるので指摘しない）
TERM_OK = {
    # **粒度が違う。** 「枠」は画面上の四角（ビューアの枠、ポップアップの枠）で、
    # 「ペイン」はファイル一覧を持つ左右の単位。枠はペインの一部でもある
    'ペイン': {'ペイン', '枠'},
    # 「マーク」は Space で付ける印（複数・操作の対象）、「選択」はビューアの
    # 範囲選択とメニューの行選び。**揃えると対象が何か分からなくなる**
    'マーク': {'マーク', '選択'},
    # 3つとも別物。ビューア＝読む、エディタ＝書く（同じ窓の2つの状態）、
    # パネル＝ペインに嵌まっている状態そのもの
    'ビューア': {'ビューア', 'エディタ', 'パネル'},
    # 「シェル」は内蔵のシェル枠、「端末」は cian が乗っている外側の端末。
    # **混ぜると「どちらの話か」が消える** ―― IME やキーの説明で致命的になる
    'シェル': {'シェル', '端末', 'ターミナル'},
    # 画面に出る「リスト」は :renamelist の名前一覧。一覧表示のことではない
    '一覧': {'一覧', 'リスト'},
    # **語の硬さが2つある。** ファイラは硬い漢語（ディレクトリ・新規・選択）、
    # cian モードは柔らかい和語（フォルダ・新しい・選ぶ）── 読む人が違う。
    # vim 使いがファイルを操る画面と、「メモとるかぁ」で開く画面。
    # `フォルダー` と `ノートブック` はどちらでもない **ただの揺れ** なので、
    # 家族に入れたまま除外しない ── 1つでも出たら鳴る
    'フォルダ': {'フォルダ', 'ディレクトリ'},
    # **押すものと、起きたことの報告。** 「元に戻す」はボタンの名前
    # （macOS も Windows も undo をそう呼ぶ）で、「取り消し」は
    # 「やらなかった／やめた」という報告 ── 置換を取り消しました、
    # 待機中なら取り消し。混ぜると、報告が押せるものに見える
    '取り消し': {'取り消し', '元に戻す'},
    # 同じ理由。`新規タブ`（ファイラ、端末版のキー表と同じ語）と
    # `新しいノート`（cian モード）。`作成` は電話の「作る」ボタンの確定側で、
    # 2026-09-04 に本人が指定したもの
    '作る': {'新しい', '新規', '作成', '作る'},
}

#: 単数で通す（2026-08 に決めた）。複数形が残っていないか
PLURAL_OK = {
    # init.lua が `view = "details"` と書き、エクスプローラもそう呼ぶ。
    # **ここだけは複数形が正名**
    'details', 'icons',
    # 本家 vim の実名
    'oldfiles', 'files',
    # 名詞そのもの
    'toggles', 'keys', 'vimkeys', 'colors', 'always',
    # 置き場所は複数ある（自分専用と、みんなで書くところ）。単数の `:note` は
    # 「1つのノート」に読めて `:newnote` と紛れる
    'notes',
    # 複数形ではない。unix のコマンド名と "save as"
    'ls', 'saveas', 'less', 'ps', 'gitstatus', 'status',
}


def _ui_text() -> str:
    """画面に出る文言だけを取り出す。**3つの前端すべてから。**

    **コメントと変数名は読み手が違う。** 混ぜて数えると「散らかっている」
    ように見えてしまい、指摘が信用されなくなる。

    2026-09-05 まで、ここは Rust しか読んでいなかった。窓版の `tr()` と電話の
    Swift は一度も数えられておらず、**揺れはそこで育った** ── 同じ画面の中で
    見出しが「ノートの置き場所」、ボタンが「保存場所を選ぶ」になっていて、
    検査は「揃っています」と言い続けた。数えていないものは揃わない。
    """
    out = []
    for p in SRC:
        src = _read(p)
        src = re.sub(r'^\s*//[^\n]*', '', src, flags=re.M)
        out += [m.group(1) for m in re.finditer(r'"((?:[^"\\\n]|\\.)*)"', src)]

    # 窓版 ── `tr(英語, 日本語)` の2つめだけ。英語側や変数名を数えると、
    # 家族の言葉がコードの中の英単語に当たって鳴る
    gui = os.path.join(ROOT, 'gui', 'renderer.js')
    if os.path.exists(gui):
        src = _read(gui)
        q = r"'(?:[^'\\]|\\.)*'|\"(?:[^\"\\]|\\.)*\"|`(?:[^`\\]|\\.)*`"
        for m in re.finditer(rf'tr\(\s*(?:{q})\s*,\s*({q})', src):
            out.append(m.group(1)[1:-1])

    # 電話（Swift）はここで読んでいた。**2026-09-05 に amber へ出た** ──
    # `~/workspace/amber`。読む先を `if os.path.isdir(...)` で守っていたので、
    # 出したあとも黙って通っていた。`keycover.py` が窓版を `tr()` に通した回に
    # 72 → 2 種に落ちたまま百分率を出し続けたのと同じ形。**無いなら無いと
    # 書く**ほうが、黙って飛ばすより強い。amber 側で同じ検査を持つのは別の日に。

    return '\n'.join(x for x in out if re.search(r'[ぁ-んァ-ヶ一-龯]', x))


def naming() -> int:
    print()
    print('=' * 72)
    print('③ 名前・用語の散らかり')
    print('=' * 72)
    found = 0
    ui = _ui_text()

    print('  ■ 画面に出る用語（同じものを別の言葉で呼んでいないか）')
    for label, words in TERM_FAMILIES.items():
        hit = {w: len(re.findall(re.escape(w), ui)) for w in words}
        hit = {w: n for w, n in hit.items() if n}
        ok = TERM_OK.get(label, set())
        if len(hit) > 1 and set(hit) - ok:
            print(f'    ★ {label}: ' + ' / '.join(f'{w} {n}回' for w, n in hit.items()))
            found += 1

    print('  ■ コマンド名（基本単数。複数形は理由が要る）')
    for v in sorted(_verbs()):
        if v in PLURAL_OK or not v.endswith('s') or v.endswith('ss'):
            continue
        if v[:-1] in _verbs():
            print(f'    ★ :{v} と :{v[:-1]} が両方ある')
        else:
            print(f'    ★ :{v} — 複数形')
        found += 1

    # 「メニューにはあるがコマンドが無い」は測ろうとして**やめた**。
    # cian のメニュー項目と関数名に命名の対応が無いため（`MenuItem::Copy` は
    # `clip_targets()` を呼ぶ）、名前照合では作れない。ゆるくすると66件が
    # 全部素通りし、厳しくすると55件が誤検出になった。**素通りする検査は
    # 「所見なし」に見えるぶん、無い検査より悪い。** 非対称は人が見る。

    print(f'  → {found} 件' if found else '  揃っています')
    return found


#: ブラウザ・Electron 側が用意しているもの。ここに無い名前を呼んでいたら、
#: それは書き間違いか、消し忘れた呼び出し。
JS_GLOBALS = {
    'window', 'document', 'navigator', 'console', 'setTimeout', 'clearTimeout',
    'setInterval', 'clearInterval', 'requestAnimationFrame', 'fetch', 'alert',
    'Promise', 'Array', 'Object', 'String', 'Number', 'Boolean', 'Math', 'JSON',
    'Date', 'Map', 'Set', 'Error', 'RegExp', 'Intl', 'parseInt', 'parseFloat',
    'isNaN', 'encodeURIComponent', 'decodeURIComponent', 'structuredClone',
    # `decodeURIComponent` was here and its three siblings were not, so the
    # first use of `decodeURI` read as a call to nothing. `Symbol` and `CSS`
    # were the same omission, and both had been sitting in the report as
    # standing false positives — which is how a report teaches people to
    # skim it.
    'decodeURI', 'encodeURI', 'Symbol', 'CSS',
    'require', 'module', 'process', 'Buffer', '__dirname', 'queueMicrotask',
    'getComputedStyle', 'URL', 'Blob', 'TextDecoder', 'TextEncoder',
    'if', 'for', 'while', 'switch', 'catch', 'return', 'typeof', 'function',
    'await', 'new', 'else', 'do', 'of', 'in', 'delete', 'void', 'throw', 'yield',
    'async', 'class', 'super', 'this',
}


def _strip_js(src: str) -> str:
    """JS からコメントと文字列を落とす。**一度の走査で、順番を作らない。**

    正規表現を順番に当てる版を2つ書いて、2つとも壊れた。バッククォートを
    先に外すと、コメントの散文に書いた ` が数に入って囲みが1つずれ、
    テンプレートの中身がコードとして数えられる。コメントを先に外すと
    `'http://…'` の `//` から行末までが消えて、その行の呼び出しが見えなくなる
    ―― **物差しが黙る方の壊れ方**で、わざと壊しても何も言わなくなった。

    どちらの順番も間違いなので、順番を持たない。いま何の中にいるかを見ながら
    1文字ずつ進めば、`//` が文字列の中にあるのか外にあるのかは迷いようがない。
    """
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c == '/' and i + 1 < n and src[i + 1] == '/':
            while i < n and src[i] != '\n':
                i += 1
            continue
        if c == '/' and i + 1 < n and src[i + 1] == '*':
            end = src.find('*/', i + 2)
            i = n if end < 0 else end + 2
            continue
        if c in '\'"`':
            quote = c
            i += 1
            while i < n:
                if src[i] == '\\':
                    i += 2
                    continue
                if src[i] == quote:
                    i += 1
                    break
                # A template's `${…}` is code, not text, and a call inside one
                # is a call. Kept rather than blanked.
                if quote == '`' and src[i] == '$' and i + 1 < n and src[i + 1] == '{':
                    depth = 1
                    i += 2
                    start = i
                    while i < n and depth:
                        if src[i] == '{':
                            depth += 1
                        elif src[i] == '}':
                            depth -= 1
                        i += 1
                    # Spaced apart, because the text between two `${…}` runs
                    # is dropped and the two would otherwise be glued into one
                    # token: `${md} ${p(x)}` read as a call to `mdp`. It
                    # invented a missing function, and — worse — could hide a
                    # real one by welding its name to whatever precedes it.
                    out.append(' ')
                    out.append(src[start:i - 1])
                    out.append(' ')
                    continue
                i += 1
            out.append('""')
            continue
        out.append(c)
        i += 1
    return ''.join(out)


def js() -> int:
    """レンダラの JS を、node --check が見ないところで見る。

    **JS は文句を言わない。** 同じ名前で関数を二度書けば後ろが黙って勝ち、
    存在しない関数を呼んでも実行してその行に来るまで何も起きない。
    実際にこの二つを同時にやって、`node --check` も `cargo test` も
    `audit.py` も全部緑のまま、起動した瞬間に落ちた。

    構文解析ではなく字面で見ているので取りこぼしはある。狙っているのは
    「動かしさえすれば必ず出るのに、動かすまで誰も気づかない」種類だけ。
    """
    print()
    print('=' * 72)
    print('④ レンダラの JS（重複定義・呼べない呼び出し）')
    print('=' * 72)
    n = 0
    for path in sorted(glob.glob(os.path.join(ROOT, 'gui', '*.js'))):
        src = _read(path)
        defs = re.findall(r'^\s*(?:async\s+)?function\s+([A-Za-z_$][\w$]*)', src, re.M)
        dupes = [name for name, c in collections.Counter(defs).items() if c > 1]
        if dupes:
            n += len(dupes)
            print(f'  ■ {_rel(path)}: 二度定義されている関数')
            for name in sorted(dupes):
                print(f'      {name}()  ── 後ろの定義が前を黙って置き換える')

        # 定義されている名前 = 関数宣言 + const/let/var + 引数っぽいもの。
        known = set(defs) | set(JS_GLOBALS)
        known |= set(re.findall(r'\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)', src))
        known |= set(re.findall(r'\b([A-Za-z_$][\w$]*)\s*=>', src))
        # クラスの中のメソッド。`name(args) {` が行頭に来る形だけを見る。
        known |= set(re.findall(r'^\s{2,}(?:async\s+|static\s+)*([A-Za-z_$][\w$]*)\s*\([^()]*\)\s*\{',
                                src, re.M))
        # 分割代入で受けたもの: const { app, BrowserWindow } = require(...)
        for names in re.findall(r'\b(?:const|let|var)\s*\{([^}]*)\}', src):
            known |= {x.strip().split(':')[-1].strip() for x in names.split(',') if x.strip()}
        for args in re.findall(r'\(([^()]*)\)\s*=>', src):
            known |= {a.strip().split('=')[0].strip() for a in args.split(',') if a.strip()}
        for args in re.findall(r'function[^(]*\(([^()]*)\)', src):
            known |= {a.strip().split('=')[0].strip() for a in args.split(',') if a.strip()}

        # 呼び出し。`.foo(` はメソッドなので除く（何に生えているかは追えない）。
        missing = collections.Counter()
        bare = _strip_js(src)
        for m in re.finditer(r'(?<![.\w$])([A-Za-z_$][\w$]*)\s*\(', bare):
            if bare[max(0, m.start() - 4):m.start()].rstrip().endswith('new'):
                continue
            if m.group(1) not in known:
                missing[m.group(1)] += 1
        # **呼ばずに渡す形。** `run: cmdJobs` のように関数を値として渡すと、
        # 上の「名前(」では一生見えない ―― メニューを作り直したとき、存在しない
        # 関数を4つ渡していて監査は「なし」と言った。押せば必ず落ちる行を、
        # 押すまで誰も知らない。
        for m in re.finditer(r'(?<![.\w$])(?:run|pick|move|leave|act|group|then|catch)\s*:\s*'
                             r'([A-Za-z_$][\w$]*)\s*[,}\n)]', bare):
            if m.group(1) not in known and m.group(1) not in ('null', 'true', 'false', 'undefined'):
                missing[m.group(1)] += 1
        if missing:
            n += len(missing)
            print(f'  ■ {_rel(path)}: どこにも定義がない呼び出し')
            for name, c in missing.most_common():
                print(f'      {name}()  × {c}')
        # 双子。**Rust では測っているのに JS では見ていなかった。**
        # 「統合できるものはないか」を印象で答えると、読まずに似ていると
        # 決めた組（openMenu と show）を畳んで、実際に違う二つを1つの
        # フラグ付き関数にしてしまう。測って答える。
        funcs = {}
        for m in re.finditer(r'^\s*(?:async\s+)?function\s+([A-Za-z_$][\w$]*)', src, re.M):
            body = _body(src, src.index('{', m.end()))
            norm = re.sub(r'//[^\n]*', '', body)
            norm = re.sub(r'\s+', ' ', norm).strip()
            if len(norm) > 300:
                funcs[m.group(1)] = (norm, src[:m.start()].count('\n') + 1)
        names = sorted(funcs)
        for i, a2 in enumerate(names):
            for b2 in names[i + 1:]:
                sm = difflib.SequenceMatcher(None, funcs[a2][0], funcs[b2][0])
                if sm.quick_ratio() < 0.80:
                    continue
                r = sm.ratio()
                if r >= 0.80 and (a2, b2) not in DUP_OK and (b2, a2) not in DUP_OK:
                    print(f'  ★ {r:.2f} {_rel(path)}: {a2}:{funcs[a2][1]} ↔ {b2}:{funcs[b2][1]}')
                    n += 1

        # 同じ行の繰り返し。Rust 側と同じ物差し。
        c = collections.Counter(
            l.strip() for l in src.splitlines()
            if len(l.strip()) > 70 and not l.strip().startswith(('//', '///', '*')))
        for line, k in sorted(((l, k) for l, k in c.items() if k >= 4), key=lambda x: -x[1])[:5]:
            print(f'  ★ 同じ行が {k} 回  {line[:78]}')
            n += 1
    if not n:
        print('  なし')
    return n


def main() -> int:
    which = sys.argv[1] if len(sys.argv) > 1 else 'all'
    n = 0
    if which in ('all', 'dead'):
        n += dead()
    if which in ('all', 'dup'):
        n += dup()
    if which in ('all', 'naming'):
        n += naming()
    if which in ('all', 'js'):
        n += js()
    print()
    print('=' * 72)
    print(f'合計 {n} 件の候補' if n else 'きれいです')
    print('候補であって誤りではない。意図があって分けているものは')
    print('scripts/audit.py の TERM_OK / PLURAL_OK に理由つきで書く。')
    print('=' * 72)
    return 0


if __name__ == '__main__':
    sys.exit(main())
