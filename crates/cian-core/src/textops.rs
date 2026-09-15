//! Whole-buffer text transforms — the "整形" half of an editor: sorting and
//! de-duplicating lines, converting between full- and half-width forms,
//! and normalising indentation.
//!
//! Every function here takes lines and returns lines, so the viewer can apply
//! one to a visual selection or to the whole file with the same call, and so
//! the rules are testable without a screen.
//!
//! The width conversions are the reason this module is not just a couple of
//! `sort()` calls. Japanese documents arrive with ASCII written full-width
//! (`ＡＢＣ１２３`) because some other tool did it, and with katakana written
//! half-width (`ｱｲｳ`) because a mainframe did — and the two want converting in
//! *opposite* directions. So "to half-width" means ASCII only and "to
//! full-width" means kana only, which is what a person actually wants when
//! they reach for either.

/// Sort `lines`, optionally descending. Comparison is by the text as written:
/// a "natural" order that understands embedded numbers is a different feature
/// and a surprising default.
pub fn sort(lines: &[String], descending: bool) -> Vec<String> {
    let mut out = lines.to_vec();
    out.sort();
    if descending {
        out.reverse();
    }
    out
}

/// Drop repeated lines, keeping the first of each. Unlike `uniq(1)` this does
/// not require the input to be sorted — in a document the duplicates are
/// scattered, and demanding a sort first would reorder something the user
/// wanted left alone.
pub fn uniq(lines: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    lines.iter().filter(|l| seen.insert((*l).clone())).cloned().collect()
}

/// Full-width ASCII → half-width, and the half-width katakana forms → their
/// full-width equivalents. In other words: make the Latin text normal, and
/// make the kana normal. Both are "the half-width one is wrong" in practice.
pub fn to_halfwidth(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // A half-width kana followed by a (semi-)voiced mark is one character.
        if let Some(base) = halfwidth_kana_base(c) {
            let next = chars.get(i + 1).copied();
            if let Some(comb) = next.and_then(|n| combine_kana(base, n)) {
                out.push(comb);
                i += 2;
                continue;
            }
            out.push(base);
            i += 1;
            continue;
        }
        out.push(match c {
            // The full-width ASCII block sits exactly 0xFEE0 above ASCII.
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ', // ideographic space
            other => other,
        });
        i += 1;
    }
    out
}

/// Half-width ASCII → full-width. The mirror of [`to_halfwidth`]'s ASCII half,
/// for the times a form or a fixed-width report wants full-width digits.
pub fn to_fullwidth(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '!'..='~' => char::from_u32(c as u32 + 0xFEE0).unwrap_or(c),
            ' ' => '\u{3000}',
            other => other,
        })
        .collect()
}

/// The full-width kana a half-width one maps to, ignoring voice marks.
fn halfwidth_kana_base(c: char) -> Option<char> {
    const TABLE: &[(char, char)] = &[
        ('｡', '。'), ('｢', '「'), ('｣', '」'), ('､', '、'), ('･', '・'),
        ('ｦ', 'ヲ'), ('ｧ', 'ァ'), ('ｨ', 'ィ'), ('ｩ', 'ゥ'), ('ｪ', 'ェ'), ('ｫ', 'ォ'),
        ('ｬ', 'ャ'), ('ｭ', 'ュ'), ('ｮ', 'ョ'), ('ｯ', 'ッ'), ('ｰ', 'ー'),
        ('ｱ', 'ア'), ('ｲ', 'イ'), ('ｳ', 'ウ'), ('ｴ', 'エ'), ('ｵ', 'オ'),
        ('ｶ', 'カ'), ('ｷ', 'キ'), ('ｸ', 'ク'), ('ｹ', 'ケ'), ('ｺ', 'コ'),
        ('ｻ', 'サ'), ('ｼ', 'シ'), ('ｽ', 'ス'), ('ｾ', 'セ'), ('ｿ', 'ソ'),
        ('ﾀ', 'タ'), ('ﾁ', 'チ'), ('ﾂ', 'ツ'), ('ﾃ', 'テ'), ('ﾄ', 'ト'),
        ('ﾅ', 'ナ'), ('ﾆ', 'ニ'), ('ﾇ', 'ヌ'), ('ﾈ', 'ネ'), ('ﾉ', 'ノ'),
        ('ﾊ', 'ハ'), ('ﾋ', 'ヒ'), ('ﾌ', 'フ'), ('ﾍ', 'ヘ'), ('ﾎ', 'ホ'),
        ('ﾏ', 'マ'), ('ﾐ', 'ミ'), ('ﾑ', 'ム'), ('ﾒ', 'メ'), ('ﾓ', 'モ'),
        ('ﾔ', 'ヤ'), ('ﾕ', 'ユ'), ('ﾖ', 'ヨ'),
        ('ﾗ', 'ラ'), ('ﾘ', 'リ'), ('ﾙ', 'ル'), ('ﾚ', 'レ'), ('ﾛ', 'ロ'),
        ('ﾜ', 'ワ'), ('ﾝ', 'ン'),
    ];
    TABLE.iter().find(|(h, _)| *h == c).map(|(_, f)| *f)
}

/// Fold a voiced (ﾞ) or semi-voiced (ﾟ) mark into the kana before it.
fn combine_kana(base: char, mark: char) -> Option<char> {
    let voiced = "カキクケコサシスセソタチツテトハヒフヘホ";
    let semi = "ハヒフヘホ";
    match mark {
        'ﾞ' | '\u{3099}' | '\u{309B}' => voiced
            .chars()
            .position(|c| c == base)
            .and_then(|_| char::from_u32(base as u32 + 1))
            .or(if base == 'ウ' { Some('ヴ') } else { None }),
        'ﾟ' | '\u{309A}' | '\u{309C}' => {
            semi.chars().position(|c| c == base).and_then(|_| char::from_u32(base as u32 + 2))
        }
        _ => None,
    }
}

/// Leading tabs → `width` spaces each. Only leading whitespace is touched: a
/// tab inside a line is usually a column separator in data, and expanding it
/// would silently change the file's meaning.
pub fn expand_tabs(lines: &[String], width: usize) -> Vec<String> {
    lines
        .iter()
        .map(|l| {
            let indent: String = l.chars().take_while(|c| *c == '\t' || *c == ' ').collect();
            let rest = &l[indent.len()..];
            let expanded: String = indent
                .chars()
                .map(|c| if c == '\t' { " ".repeat(width) } else { " ".to_string() })
                .collect();
            format!("{expanded}{rest}")
        })
        .collect()
}

/// Every tab in the line, not only the leading ones, expanded to the next
/// stop.
///
/// Kept apart from [`expand_tabs`] on purpose. In a tab-separated file the
/// tabs in the middle of a line are the data — they are what separates the
/// fields — so converting them silently as part of "fix the indentation"
/// would quietly turn a table into prose. This one has to be asked for.
pub fn expand_all_tabs(lines: &[String], width: usize) -> Vec<String> {
    let width = width.max(1);
    lines
        .iter()
        .map(|l| {
            let mut out = String::with_capacity(l.len());
            let mut at = 0usize;
            for c in l.chars() {
                if c == '\t' {
                    let n = width - (at % width);
                    out.push_str(&" ".repeat(n));
                    at += n;
                } else {
                    out.push(c);
                    at += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                }
            }
            out
        })
        .collect()
}

/// Leading runs of `width` spaces → tabs. The inverse of [`expand_tabs`], with
/// the same "leading only" rule and for the same reason.
pub fn unexpand_tabs(lines: &[String], width: usize) -> Vec<String> {
    if width == 0 {
        return lines.to_vec();
    }
    lines
        .iter()
        .map(|l| {
            let spaces = l.chars().take_while(|c| *c == ' ').count();
            let rest: String = l.chars().skip(spaces).collect();
            format!("{}{}{}", "\t".repeat(spaces / width), " ".repeat(spaces % width), rest)
        })
        .collect()
}

/// Re-indent to a consistent step — VS Code's "reformat the indentation"
/// without knowing the language.
///
/// The nesting is read from the *existing* indentation: each distinct depth
/// that appears becomes one level, and levels are re-emitted at `width`
/// spaces each. That fixes a document indented 3-and-5 and 2 by different
/// hands without needing to parse it, and leaves blank lines blank.
pub fn reindent(lines: &[String], width: usize) -> Vec<String> {
    // The ladder of indent widths actually used, smallest first.
    let mut steps: Vec<usize> = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent_width(l))
        .collect();
    steps.sort_unstable();
    steps.dedup();
    lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                return String::new();
            }
            let depth = steps.iter().position(|w| *w == indent_width(l)).unwrap_or(0);
            format!("{}{}", " ".repeat(depth * width), l.trim_start())
        })
        .collect()
}

/// The visual width of a line's leading whitespace, counting a tab as 8 —
/// the width every terminal and printer has agreed on for long enough that
/// mixed-indent files were laid out against it.
fn indent_width(line: &str) -> usize {
    let mut w = 0;
    for c in line.chars() {
        match c {
            ' ' => w += 1,
            '\t' => w = (w / 8 + 1) * 8,
            _ => break,
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 窓版の `cellWidth` が、エンジンと同じ桁を答えるか。
    ///
    /// **二つの前端が別々に幅を数えていた。** 端末版とエンジンは
    /// `unicode-width` を引き、窓版は `gui/renderer.js` に East Asian Width の
    /// 主な帯を手で並べていた ── 2026-09-15 に全符号点を突き合わせたら
    /// **11350 個で食い違っていた**。目に見えたのは2つで、どちらも実機の
    /// 「全角と半角が混ざるとおかしい」の正体だった:
    ///
    ///   * `ｶﾞｷﾞｸﾞ` ── 半角カナの濁点 `U+FF9E` を窓が1桁と数えていた
    ///     （エンジンは0桁）。シェルのカーソルが3字ぶん手前に落ちた
    ///   * `✅` ── `U+2705` を窓が1桁と数えていた（エンジンは2桁）。
    ///     カーソルが1桁右へ流れた
    ///
    /// `scripts/widths.py` は「**桁を測る道具を通しているか**」を見ていて、
    /// その道具が**同じ答えを返すか**は誰も見ていなかった。ここで見る。
    ///
    /// 直すのは表だけでいい ── 窓版の `cellWidth` は、この表を引くほかに
    /// 何も判断していない。
    ///
    /// 読むのは表そのもの ── `renderer.js` の `W_ZERO` と `W_WIDE` は
    /// `始まりの間隔.長さ` を16進で並べた文字列なので、JS を動かさなくても
    /// 復号できる。表がずれたら、ここが落ちる。
    ///
    /// 直し方: `unicode-width` の版を上げたら、この表を作り直す。作り方は
    /// `renderer.js` の `W_ZERO` の上に書いてある。
    #[test]
    fn cell_width_matches_unicode_width() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../gui/renderer.js");
        if !path.exists() {
            eprintln!("gui/renderer.js not found at {}; skipping", path.display());
            return;
        }
        let src = std::fs::read_to_string(&path).unwrap();

        /// `const NAME = unpackWidths(  '…' + '…' );` の中の文字列をつなぐ。
        fn table(src: &str, name: &str) -> Vec<(u32, u32)> {
            let head = format!("const {name} = unpackWidths(");
            let at = src.find(&head).unwrap_or_else(|| panic!("{name} が無い"));
            let rest = &src[at + head.len()..];
            let end = rest.find(");").expect("閉じ括弧が無い");
            let mut packed = String::new();
            let mut chars = rest[..end].chars();
            while let Some(c) = chars.next() {
                if c != '\'' {
                    continue;
                }
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    packed.push(c);
                }
            }
            let mut out = Vec::new();
            let mut at: i64 = -1;
            for part in packed.split_whitespace() {
                let (d, n) = part.split_once('.').expect("始まり.長さ の形");
                let a = at + i64::from_str_radix(d, 16).unwrap() + 1;
                let b = a + i64::from_str_radix(n, 16).unwrap();
                out.push((a as u32, b as u32));
                at = b;
            }
            assert!(!out.is_empty(), "{name} が空");
            out
        }

        let zero = table(&src, "W_ZERO");
        let wide = table(&src, "W_WIDE");
        // **表が急に縮んでいないか。** 数を数える検査には下限を書く ──
        // 覆いが減っても百分率は出せてしまう（`keycover.py` は 72 → 2 種に
        // 減ったあとも、百分率だけは出し続けた）。
        assert!(zero.len() >= 390, "幅0の表が縮んでいる: {} 範囲", zero.len());
        assert!(wide.len() >= 120, "幅2の表が縮んでいる: {} 範囲", wide.len());

        let has = |t: &[(u32, u32)], c: u32| t.binary_search_by(|&(a, b)| {
            if c < a { std::cmp::Ordering::Greater }
            else if c > b { std::cmp::Ordering::Less }
            else { std::cmp::Ordering::Equal }
        }).is_ok();

        let mut bad = Vec::new();
        for c in 0u32..=0x10FFFF {
            let Some(ch) = char::from_u32(c) else { continue };
            let want = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            // `renderer.js` の `cellWidth` と同じ順で決める。
            let got = if (0x20..0x7f).contains(&c) {
                1
            } else if c == 0x17d8 {
                3
            } else if has(&zero, c) {
                0
            } else if has(&wide, c) {
                2
            } else {
                1
            };
            if got != want {
                bad.push((c, got, want));
            }
        }
        assert!(
            bad.is_empty(),
            "窓版の cellWidth がエンジンと食い違う符号点が {} 個: {:x?}",
            bad.len(),
            &bad[..bad.len().min(8)],
        );
    }

    fn blk(top: usize, bottom: usize, left: usize, right: usize) -> Block {
        Block { top, bottom, left, right }
    }

    /// The rectangle comes out of every line it covers, and a line too short
    /// to reach into it is left exactly as it was.
    #[test]
    fn block_delete_cuts_a_column_and_spares_short_lines() {
        let src = v(&["abcdef", "abcdef", "ab", "abcdef"]);
        assert_eq!(
            block_delete(&src, blk(0, 3, 2, 4)),
            v(&["abef", "abef", "ab", "abef"]),
        );
        assert_eq!(block_text(&src, blk(0, 2, 2, 4)), v(&["cd", "cd", ""]));
    }

    /// Inserting down a column pads the short lines out, because the whole
    /// point is that the column lines up afterwards.
    #[test]
    fn block_insert_pads_short_lines_so_the_column_aligns() {
        let src = v(&["abcdef", "ab", ""]);
        assert_eq!(block_insert(&src, blk(0, 2, 3, 4), "# "), v(&["abc# def", "ab # ", "   # "]));
        // Appending works at the right edge, padding the same way.
        assert_eq!(block_append(&v(&["ab", "abcd"]), blk(0, 1, 0, 3), "|"), v(&["ab |", "abc|d"]));
    }

    /// Replace is a delete and an insert in one step — rewriting a column of
    /// values without doing it twice.
    #[test]
    fn block_replace_swaps_the_rectangle() {
        let src = v(&["id=001 x", "id=002 y", "id=0"]);
        assert_eq!(
            block_replace(&src, blk(0, 2, 3, 6), "999"),
            v(&["id=999 x", "id=999 y", "id=999"]),
        );
    }

    /// The rectangle is built from two cursor positions in any order, and
    /// includes the cell the anchor sits on.
    #[test]
    fn a_block_spans_both_cursors_inclusively() {
        let ascii = v(&["abcdefgh", "abcdefgh", "abcdefgh", "abcdefgh"]);
        assert_eq!(Block::between(&ascii, (3, 5), (1, 2)), blk(1, 3, 2, 6));
        assert_eq!(Block::between(&ascii, (1, 2), (3, 5)), blk(1, 3, 2, 6));
        assert_eq!(
            block_text(&v(&["abcdef"]), Block::between(&v(&["abcdef"]), (0, 1), (0, 3))),
            v(&["bcd"]),
        );
    }

    /// The point of counting columns: a rectangle drawn over mixed-width text
    /// is a rectangle on screen, and every line loses the same columns.
    /// The line-selection version: the left edge is column zero, and the
    /// right edge is wherever each line already stops — no squaring off.
    #[test]
    fn a_line_selection_takes_text_at_either_end() {
        let src = v(&["one", "", "three", "four"]);
        assert_eq!(line_affix(&src, 0, 2, "# ", false), v(&["# one", "# ", "# three", "four"]));
        assert_eq!(line_affix(&src, 0, 2, ",", true), v(&["one,", ",", "three,", "four"]));
        // Past the end is not a panic.
        assert_eq!(line_affix(&src, 2, 99, "x", true), v(&["one", "", "threex", "fourx"]));
    }

    #[test]
    fn a_block_is_rectangular_on_screen_not_in_characters() {
        // "あい" is four columns wide; "abcd" is four columns of one each.
        let src = v(&["あいうえ", "abcdefgh", "あbcう"]);
        // Columns 2..6 — the second full-width character, and the characters
        // under it on the other lines.
        let b = Block { top: 0, bottom: 2, left: 2, right: 6 };
        assert_eq!(block_text(&src, b), v(&["いう", "cdef", "bcう"]));
        assert_eq!(block_delete(&src, b), v(&["あえ", "abgh", "あ"]));

        // Dragged from a full-width character on one line to an ASCII one
        // below, the block still covers whole columns.
        let b2 = Block::between(&src, (0, 1), (1, 4));
        assert_eq!((b2.left, b2.right), (2, 5), "columns, not character indices");

        // A character the edge cuts through is left out rather than taken
        // whole: the block must never reach past the rectangle on screen.
        let heads = v(&["## 事前準備", "- ふたつめ"]);
        let four = Block { top: 0, bottom: 1, left: 0, right: 4 };
        assert_eq!(block_text(&heads, four), v(&["## ", "- ふ"]), "事 is only half inside");
        // Both edges cutting a character leaves nothing on that line.
        let one = v(&["あい"]);
        let cut = Block { top: 0, bottom: 0, left: 1, right: 3 };
        assert_eq!(block_text(&one, cut), v(&[""]), "neither is wholly inside");

        // Padding to a column pads in columns, so an inserted marker lines up
        // under the same place on every line.
        let ragged = v(&["あい", "ab", ""]);
        let at6 = Block { top: 0, bottom: 2, left: 6, right: 6 };
        assert_eq!(block_insert(&ragged, at6, "|"), v(&["あい  |", "ab    |", "      |"]));
    }

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn sort_and_uniq_do_the_obvious_thing() {
        assert_eq!(sort(&v(&["c", "a", "b"]), false), v(&["a", "b", "c"]));
        assert_eq!(sort(&v(&["c", "a", "b"]), true), v(&["c", "b", "a"]));
        // Duplicates need not be adjacent, and the first of each survives in
        // place — a document's repeats are scattered, and sorting first would
        // reorder what the user wanted left alone.
        assert_eq!(uniq(&v(&["b", "a", "b", "c", "a"])), v(&["b", "a", "c"]));
    }

    /// "To half-width" means the Latin text; "to full-width" means the kana.
    /// Both directions are what someone means when they reach for either.
    #[test]
    fn width_conversion_goes_the_way_people_mean() {
        assert_eq!(to_halfwidth("ＡＢＣ１２３（ｘ）"), "ABC123(x)");
        assert_eq!(to_halfwidth("全角\u{3000}空白"), "全角 空白");
        assert_eq!(to_fullwidth("ABC123"), "ＡＢＣ１２３");
        // Kana: half-width in, full-width out — including the voice marks,
        // which are separate characters half-width and joined full-width.
        assert_eq!(to_halfwidth("ｱｲｳ"), "アイウ");
        assert_eq!(to_halfwidth("ｶﾞｷﾞﾊﾟﾋﾟ"), "ガギパピ");
        assert_eq!(to_halfwidth("ｳﾞ"), "ヴ");
        // Text already in the right form is left exactly as it is.
        assert_eq!(to_halfwidth("すでに正しい text"), "すでに正しい text");
    }

    /// `expand all` reaches the separators an indent-only expand protects,
    /// and lands each one on its stop rather than emitting a fixed run.
    #[test]
    fn expand_all_converts_the_separators_too() {
        let src = v(&["col1\tcol2\tcol3", "あ\tい\tう"]);
        assert_eq!(
            expand_all_tabs(&src, 8),
            v(&["col1    col2    col3", "あ      い      う"]),
            "each field starts on a stop, whatever came before it",
        );
        // The indent-only form leaves them alone, which is why both exist.
        assert_eq!(expand_tabs(&src, 8), src);
    }

    #[test]
    fn tab_conversion_touches_only_the_indent() {
        // A tab inside the line is a column separator in data; expanding it
        // would silently change what the file means.
        assert_eq!(expand_tabs(&v(&["\ta\tb"]), 4), v(&["    a\tb"]));
        assert_eq!(unexpand_tabs(&v(&["    a b"]), 4), v(&["\ta b"]));
        // A partial step keeps its leftover spaces rather than rounding.
        assert_eq!(unexpand_tabs(&v(&["      x"]), 4), v(&["\t  x"]));
    }

    /// Re-indent reads the nesting from what is there, so a file indented by
    /// three different hands comes out on one ladder.
    #[test]
    fn reindent_rebuilds_a_consistent_ladder() {
        let src = v(&["top", "   three", "        eight", "   three again", "", "top again"]);
        assert_eq!(
            reindent(&src, 2),
            v(&["top", "  three", "    eight", "  three again", "", "top again"]),
        );
        // A tab counts as eight columns, which is the width the mixed file
        // was laid out against in the first place.
        assert_eq!(reindent(&v(&["a", "\tb"]), 4), v(&["a", "    b"]));
    }
}

/// A rectangle over the buffer: whole lines `top..=bottom`, columns
/// `left..right` (end-exclusive) within each.
///
/// Lines shorter than `left` are the awkward case every block editor has to
/// answer. cian's answer: a delete leaves them alone (there is nothing inside
/// the rectangle to remove) and an insert pads them out with spaces to reach
/// the column, because the point of inserting down a column is that the
/// A rectangular selection, in **display columns** rather than characters.
///
/// This is the difference between a rectangle and a ragged edge. A full-width
/// character takes two columns, so counting characters puts the right-hand
/// edge somewhere different on every line that mixes kana with ASCII — which
/// is most of the lines this exists for. Sakura Editor reckons in columns, and
/// so does this.
///
/// The block covers exactly the characters that lie *wholly* inside it. A
/// full-width character the edge cuts through is left out, so a four-column
/// block over `## 事前準備` gives `## ` and not `## 事`: the selection never
/// reaches further right than the rectangle drawn on screen. Half a character
/// is not something a text file can hold, and taking the whole of one would
/// mean editing a column the user did not select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub top: usize,
    pub bottom: usize,
    /// First display column inside the block.
    pub left: usize,
    /// One past the last display column inside the block.
    pub right: usize,
}

/// How wide a character is drawn, with a tab reaching the next stop from `at`.
/// A zero-width mark counts as nothing, which keeps a combining accent
/// attached to the character it belongs to.
///
/// Public because the renderer, the mouse and the block selection all have to
/// agree; when the renderer had its own idea — every character one column —
/// a tab after a full-width character landed on the wrong stop and a
/// tab-separated file failed to line up however wide the stops were set.
pub fn char_cols(c: char, at: usize) -> usize {
    if c == '\t' {
        let w = crate::viewer::tab_width();
        return w - (at % w);
    }
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
}

/// The screen column character `at` starts in, and the width of the whole
/// line. Both in drawn columns, because that is what "how far right is the
/// cursor" and "how wide is this file" are asking about — a tab is one
/// character and up to eight columns, a Japanese character one and two.
pub fn col_span(line: &str, at: usize) -> (usize, usize) {
    let mut col = 0usize;
    let mut start = None;
    for (i, ch) in line.chars().enumerate() {
        if i == at {
            start = Some(col);
        }
        col += char_cols(ch, col);
    }
    (start.unwrap_or(col), col)
}

/// Where each character of `line` starts, and how wide it is, plus the width
/// of the whole line. One left-to-right pass, because a tab's width depends on
/// where it begins.
fn columns(line: &str) -> (Vec<(usize, usize)>, usize) {
    let mut out = Vec::new();
    let mut at = 0usize;
    for c in line.chars() {
        let w = char_cols(c, at);
        out.push((at, w));
        at += w;
    }
    (out, at)
}

impl Block {
    /// The rectangle between two cursor positions, in any order.
    ///
    /// The positions arrive as `(line, character)` — where the cursor actually
    /// is — and are converted to columns here, because only the text knows how
    /// wide its own characters are.
    pub fn between(lines: &[String], a: (usize, usize), b: (usize, usize)) -> Block {
        // `past` asks for the column *after* the character, so the one the
        // cursor sits on is inside the block rather than just outside it.
        let col_of = |(l, c): (usize, usize), past: bool| -> usize {
            let Some(text) = lines.get(l) else { return 0 };
            let (cols, total) = columns(text);
            match cols.get(c) {
                Some((start, w)) => start + if past { *w } else { 0 },
                None => total + usize::from(past),
            }
        };
        Block {
            top: a.0.min(b.0),
            bottom: a.0.max(b.0),
            left: col_of(a, false).min(col_of(b, false)),
            right: col_of(a, true).max(col_of(b, true)),
        }
    }

    /// Where this block starts and ends in `line`, as character indices —
    /// the characters lying wholly within its columns.
    pub fn char_range(&self, line: &str) -> (usize, usize) {
        let (cols, _) = columns(line);
        // The first character that starts at or after the left edge, and the
        // first one that would run past the right edge.
        let from = cols.iter().position(|(s, _)| *s >= self.left).unwrap_or(cols.len());
        let to = cols.iter().position(|(s, w)| s + w > self.right).unwrap_or(cols.len());
        (from, to.max(from))
    }
}

/// Grow `chars` with spaces until it is `col` display columns wide.
fn pad_to(chars: &mut Vec<char>, col: usize) {
    let mut at = chars.iter().fold(0usize, |a, c| a + char_cols(*c, a));
    while at < col {
        chars.push(' ');
        at += 1;
    }
}

/// Cut the rectangle out of each line. Short lines keep what they have.
pub fn block_delete(lines: &[String], b: Block) -> Vec<String> {
    edit_block(lines, b, |chars, from, to| {
        if from >= chars.len() {
            return; // nothing of this line lies inside the rectangle
        }
        chars.drain(from..to.min(chars.len()));
    })
}

/// Insert `text` at the rectangle's left edge on every line, padding short
/// lines with spaces so the inserted column actually lines up.
pub fn block_insert(lines: &[String], b: Block, text: &str) -> Vec<String> {
    edit_block_at(lines, b, b.left, |chars, from, _| {
        for (i, c) in text.chars().enumerate() {
            chars.insert(from + i, c);
        }
    })
}

/// Append `text` at the rectangle's right edge on every line, padding to reach
/// it. The block equivalent of vim's `A`, for adding a trailing column.
pub fn block_append(lines: &[String], b: Block, text: &str) -> Vec<String> {
    edit_block_at(lines, b, b.right, |chars, _, to| {
        for (i, c) in text.chars().enumerate() {
            chars.insert(to + i, c);
        }
    })
}

/// Replace the rectangle's contents with `text` on every line: a delete and an
/// insert in one step, which is how a column of values gets rewritten.
pub fn block_replace(lines: &[String], b: Block, text: &str) -> Vec<String> {
    edit_block_at(lines, b, b.left, |chars, from, to| {
        if from < chars.len() {
            chars.drain(from..to.min(chars.len()));
        }
        for (i, c) in text.chars().enumerate() {
            chars.insert(from + i, c);
        }
    })
}

/// Put `text` at the start of every line in `top..=bottom`, or at each line's
/// own end.
///
/// The line-selection counterpart to [`block_insert`] and [`block_append`].
/// The difference that matters is the right-hand one: a block appends at a
/// *column*, padding short lines to reach it, while this appends at whatever
/// each line's end happens to be. "Put a comma on the end of all of these" is
/// the request, and it does not want the lines squared off first.
pub fn line_affix(lines: &[String], top: usize, bottom: usize, text: &str, at_end: bool) -> Vec<String> {
    let mut out = lines.to_vec();
    for i in top..=bottom.min(out.len().saturating_sub(1)) {
        if at_end {
            out[i].push_str(text);
        } else {
            out[i].insert_str(0, text);
        }
    }
    out
}

/// The rectangle's contents, one string per line, for the clipboard.
pub fn block_text(lines: &[String], b: Block) -> Vec<String> {
    (b.top..=b.bottom)
        .filter_map(|i| lines.get(i))
        .map(|l| {
            let (from, to) = b.char_range(l);
            let chars: Vec<char> = l.chars().collect();
            if from >= chars.len() {
                String::new()
            } else {
                chars[from..to.min(chars.len())].iter().collect()
            }
        })
        .collect()
}

/// Run `f` over each line the block covers, as chars, and rebuild.
fn edit_block(lines: &[String], b: Block, f: impl Fn(&mut Vec<char>, usize, usize)) -> Vec<String> {
    let mut out = lines.to_vec();
    for i in b.top..=b.bottom.min(out.len().saturating_sub(1)) {
        let (from, to) = b.char_range(&out[i]);
        let mut chars: Vec<char> = out[i].chars().collect();
        f(&mut chars, from, to);
        out[i] = chars.into_iter().collect();
    }
    out
}

/// The same, but padding each line out to `col` display columns first — for
/// the edits that put text *at* a column, where a line too short to reach it
/// has to be filled or the column will not line up.
fn edit_block_at(
    lines: &[String],
    b: Block,
    col: usize,
    f: impl Fn(&mut Vec<char>, usize, usize),
) -> Vec<String> {
    let mut out = lines.to_vec();
    for i in b.top..=b.bottom.min(out.len().saturating_sub(1)) {
        let mut chars: Vec<char> = out[i].chars().collect();
        pad_to(&mut chars, col);
        let padded: String = chars.iter().collect();
        let (from, to) = b.char_range(&padded);
        f(&mut chars, from, to);
        out[i] = chars.into_iter().collect();
    }
    out
}
