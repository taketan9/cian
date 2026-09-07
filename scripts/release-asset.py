#!/usr/bin/env python3
"""公開済みの Release に、資材を1つ足す。

    python3 scripts/release-asset.py v3.0.1 --zip cian-server-win-x64.exe
    python3 scripts/release-asset.py v3.0.1 --add packaging/GUI.txt
    python3 scripts/release-asset.py v3.0.1 --zip cian-server-macos.bin --dry-run

## なぜ手順を1本にするのか

2026-09-07、crmaine の配布先（会社の運用端末）で**社内の網が `.exe` の直接
ダウンロードを止め**、Release からエンジンを落とせなかった。zip なら通るので
`cian-server-win-x64.exe.zip` を足した。そのとき手で踏んだ手順が7つあり、
**そのうち3つは飛ばしても成功したように見える**:

  ① 公開されている資材を**落とす**（作り直さない）
  ② 公開されている `SHA256SUMS` と照合して、落としたものが本物か確かめる
  ③ zip を作る
  ④ **取り出してバイト一致を確認する**
  ⑤ `SHA256SUMS` の既存行を**一字も変えずに**追記する
  ⑥ 上げる
  ⑦ **落とし直して**、Release に載っているもので照合し直す

飛ばすと危ないのは ①④⑦。作り直した zip は「同じ名前で中身が違うもの」に
なり得るし、上げた後に確かめなければ「置いたつもり」で終わる。**いちばん
気付けない壊れ方**で、しかも気付くのは会社端末で困っている人になる。

だから手順ではなくコードにしてある。次に頼まれたとき、同じ順になる。

## 作り直さない、という一点

`--zip` が包むのは**その Release に載っているバイト列**であって、いま手元で
ビルドできるものではない。同じコミットから建てても Rust の出力は環境で変わる
し、そもそも `cargo clean` の後なら手元には何も無い。「公開されている exe を
zip にした」以外のものを置いてはいけない。

## 罠（`SHA256SUMS` を手で継ぎ足した版について）

このスクリプトが触った版に対して `release` ワークフローを**回し直すと**、
バイナリが新しくビルドされ、ハッシュが全部違う `SHA256SUMS` で上書きされる。
「彼が検証済みの版の資材は差し替えない」が破れる形。回し直すなら新しい版を
切ること。

## 次の版からは要らない

`release.yml` は Windows / macOS のエンジンを最初から zip でも出す（依頼 185）。
これは**既に出てしまった版**に後から足すための道具。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SUMS = "SHA256SUMS"


def run(*args: str, **kw) -> subprocess.CompletedProcess:
    """`gh` を呼ぶ。落ちたら、その言い分ごと止める。"""
    p = subprocess.run(args, capture_output=True, text=True, **kw)
    if p.returncode != 0:
        sys.exit(f"✗ {' '.join(args)}\n{p.stderr.strip() or p.stdout.strip()}")
    return p


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def make_zip(src: Path, dest: Path) -> None:
    """中身は1つだけ、直下、名前はそのまま。

    `zipfile` を使うのは、入るものを1つずつ決めるため。macOS の `zip` は
    拡張属性や `__MACOSX` を混ぜることがあり、それが入ると「中身は1つだけ」
    という約束が守れない。
    """
    with zipfile.ZipFile(dest, "w", zipfile.ZIP_DEFLATED) as z:
        z.write(src, arcname=src.name)


def zip_holds_exactly(zip_path: Path, name: str, want_sha: str) -> None:
    """**取り出したものが元とバイト一致するか。** ④ の中身。

    「zip を作った」は「正しい zip を作った」ではない。ここで見ないと、
    名前だけ合っていて中身が違う資材を公開することになる。
    """
    with zipfile.ZipFile(zip_path) as z:
        names = z.namelist()
        if names != [name]:
            sys.exit(f"✗ zip の中身が {names} ── {name} 1つだけであるべき")
        got = hashlib.sha256(z.read(name)).hexdigest()
    if got != want_sha:
        sys.exit(f"✗ zip から取り出したものが元と違う\n  元  {want_sha}\n  zip {got}")


def merge_sums(existing: str, name: str, digest: str) -> str:
    """`SHA256SUMS` に1行足す。**既存の行は一字も変えない。**

    並びは `sha256sum cian-*` が作る順＝シェルの glob 順＝辞書順に合わせる。
    次に自動生成へ切り替わったとき、同じ形になっているように。
    """
    lines = [l for l in existing.splitlines() if l.strip()]
    for l in lines:
        if l.split(maxsplit=1)[1:] == [name]:
            sys.exit(f"✗ {name} は既に {SUMS} にある")
    lines.append(f"{digest}  {name}")
    lines.sort(key=lambda l: l.split(maxsplit=1)[1])
    return "\n".join(lines) + "\n"


def assets_of(tag: str, repo: str) -> list[str]:
    out = run("gh", "release", "view", tag, "-R", repo, "--json", "assets").stdout
    return [a["name"] for a in json.loads(out)["assets"]]


def main() -> int:
    ap = argparse.ArgumentParser(description="公開済みの Release に資材を1つ足す")
    ap.add_argument("tag", nargs="?", help="足す先のタグ（例: v3.0.1）")
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--zip", metavar="資材名",
                   help="その Release に載っている資材を zip で包んで足す")
    g.add_argument("--add", metavar="パス", help="手元のファイルをそのまま足す")
    ap.add_argument("--repo", default="taketan9/cian")
    ap.add_argument("--dry-run", action="store_true",
                    help="上げる直前まで全部やって、上げずに止める")
    ap.add_argument("--self-test", action="store_true", help=argparse.SUPPRESS)
    args = ap.parse_args()

    if args.self_test:
        return self_test()
    if not args.tag:
        ap.error("どのタグに足すのかが要ります")
    if not (args.zip or args.add):
        ap.error("--zip か --add のどちらかが要ります")

    work = Path(tempfile.mkdtemp(prefix="cian-release-"))
    try:
        have = assets_of(args.tag, args.repo)
        print(f"{args.tag} の資材: {', '.join(have)}")

        if args.zip:
            if args.zip not in have:
                sys.exit(f"✗ {args.zip} は {args.tag} に無い")
            name = args.zip + ".zip"
            if name in have:
                sys.exit(f"✗ {name} は既にある（差し替えはこの道具の仕事ではない）")

            # ① 公開されているものを落とす。作り直さない。
            print(f"① {args.zip} を落とす")
            run("gh", "release", "download", args.tag, "-R", args.repo,
                "-p", args.zip, "-p", SUMS, "-D", str(work))
            src = work / args.zip

            # ② それが本物か、公開されている SHA256SUMS で確かめる。
            sums = (work / SUMS).read_text()
            want = next((l.split()[0] for l in sums.splitlines()
                         if l.split(maxsplit=1)[1:] == [args.zip]), None)
            if want is None:
                sys.exit(f"✗ {SUMS} に {args.zip} の行が無い")
            got = sha256(src)
            if got != want:
                sys.exit(f"✗ 落としたものが公開ハッシュと違う\n  {want}\n  {got}")
            print(f"② 照合 ok  {got}")

            # ③④ 包んで、取り出して確かめる。
            out = work / name
            make_zip(src, out)
            zip_holds_exactly(out, args.zip, got)
            print(f"③④ {name} ── 中身 1つ・{out.stat().st_size:,} バイト・取り出してバイト一致 ok")
        else:
            local = Path(args.add)
            if not local.is_file():
                sys.exit(f"✗ {local} が無い")
            name = local.name
            if name in have:
                sys.exit(f"✗ {name} は既にある（差し替えはこの道具の仕事ではない）")
            out = work / name
            shutil.copy2(local, out)
            run("gh", "release", "download", args.tag, "-R", args.repo,
                "-p", SUMS, "-D", str(work))
            print(f"①〜④ {name} をそのまま足す（{out.stat().st_size:,} バイト）")

        # ⑤ SHA256SUMS に追記。既存行は触らない。
        before = (work / SUMS).read_text()
        after = merge_sums(before, name, sha256(out))
        kept = [l for l in after.splitlines() if not l.endswith(f"  {name}")]
        if sorted(kept) != sorted(l for l in before.splitlines() if l.strip()):
            sys.exit("✗ 既存の行が変わってしまった ── 中止")
        (work / SUMS).write_text(after)
        print(f"⑤ {SUMS} に1行追記（既存 {len(kept)} 行は無傷）")

        if args.dry_run:
            print(f"\n--dry-run なのでここで止めます。作ったものは {work}")
            return 0

        # ⑥ 上げる。SHA256SUMS だけは既にあるので置き換える。
        run("gh", "release", "upload", args.tag, str(out), "-R", args.repo)
        run("gh", "release", "upload", args.tag, str(work / SUMS),
            "-R", args.repo, "--clobber")
        print("⑥ 上げました")

        # ⑦ 落とし直して、Release に載っているもので確かめる。
        #    ここまでやって初めて「置いた」と言える。
        check = work / "verify"
        check.mkdir()
        run("gh", "release", "download", args.tag, "-R", args.repo, "-D", str(check))
        sums = (check / SUMS).read_text()
        bad = []
        for line in sums.splitlines():
            if not line.strip():
                continue
            digest, fname = line.split(maxsplit=1)
            f = check / fname
            if not f.is_file():
                bad.append(f"{fname}: 落ちてこない")
            elif sha256(f) != digest:
                bad.append(f"{fname}: ハッシュが合わない")
        if bad:
            sys.exit("✗ 上げた後の照合で問題:\n  " + "\n  ".join(bad))
        print(f"⑦ 落とし直して全 {len(sums.splitlines())} 点を照合 ── ok")
        print(f"\n✓ {args.tag} に {name} を足しました")
        return 0
    finally:
        if not args.dry_run:
            shutil.rmtree(work, ignore_errors=True)


def self_test() -> int:
    """網に出ない部分だけを、その場で確かめる。

    危ないのは zip の中身と `SHA256SUMS` の継ぎ足しで、どちらも純粋な関数。
    **足した検査は壊して鳴らす**という規則がここにも要るので、壊れた形を
    ぶつけて止まることまで見る。
    """
    d = Path(tempfile.mkdtemp(prefix="cian-selftest-"))
    try:
        src = d / "cian-server-win-x64.exe"
        src.write_bytes(os.urandom(4096))
        want = sha256(src)
        z = d / "cian-server-win-x64.exe.zip"
        make_zip(src, z)
        zip_holds_exactly(z, src.name, want)
        with zipfile.ZipFile(z) as zf:
            assert zf.namelist() == [src.name], zf.namelist()
        print("  zip: 中身1つ・直下・名前そのまま・バイト一致 ok")

        # 余計なものが入った zip は弾かれるか。
        bad = d / "bad.zip"
        with zipfile.ZipFile(bad, "w") as zf:
            zf.write(src, arcname=src.name)
            zf.writestr("__MACOSX/._x", b"junk")
        try:
            zip_holds_exactly(bad, src.name, want)
        except SystemExit:
            print("  zip: 余計なものが入っていれば止まる ok")
        else:
            sys.exit("✗ 余計なものが入った zip を通してしまった")

        before = ("aaa  cian-gui-win-x64.zip\n"
                  "bbb  cian-server-win-x64.exe\n")
        after = merge_sums(before, "cian-server-win-x64.exe.zip", "ccc")
        assert after.splitlines() == [
            "aaa  cian-gui-win-x64.zip",
            "bbb  cian-server-win-x64.exe",
            "ccc  cian-server-win-x64.exe.zip",
        ], after
        for line in before.splitlines():
            assert line in after, line
        print("  sums: 既存行は無傷・辞書順に挿入 ok")

        try:
            merge_sums(after, "cian-server-win-x64.exe.zip", "ddd")
        except SystemExit:
            print("  sums: 二重登録は止まる ok")
        else:
            sys.exit("✗ 同じ名前を二度足せてしまった")
        print("\n✓ 網に出ない部分は全部通りました")
        return 0
    finally:
        shutil.rmtree(d, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
