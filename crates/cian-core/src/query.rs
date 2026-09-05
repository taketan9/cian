//! What a search box means.
//!
//! One place, because the terminal's `/` and the window's filter are two
//! front ends of one program: three boxes deciding separately is three boxes
//! that agree until somebody types two words.
//!
//! This module says *what to match*. The matching itself stays where the list
//! is — the pane filter in `lib.rs`, and whatever else grows a query later.

/// A query, read as an OR of ANDs.
///
/// `仕事 週報` wants both. `仕事 OR 家` wants either. Written together —
/// `仕事 週報 OR 家` — the OR is the weaker join, so that reads as
/// (仕事 AND 週報) OR (家), which is how everybody writes it and nobody
/// explains it.
///
/// `OR`, `or`, `|` and `｜` all mean the same thing: nobody remembers which
/// one an app wanted, and the full-width bar is what a Japanese keyboard
/// gives you without switching.
pub fn terms(query: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut group: Vec<String> = Vec::new();
    for word in query.split_whitespace() {
        if matches!(word, "OR" | "or" | "|" | "｜") {
            // A bare `OR` at the start, or two in a row, is somebody still
            // typing — not an empty group that matches everything.
            if !group.is_empty() {
                out.push(std::mem::take(&mut group));
            }
            continue;
        }
        group.push(word.to_lowercase());
    }
    if !group.is_empty() {
        out.push(group);
    }
    out
}

/// Whether `hay` answers the query. An empty query matches everything.
pub fn hits(hay: &str, query: &str) -> bool {
    let groups = terms(query);
    if groups.is_empty() {
        return true;
    }
    let hay = hay.to_lowercase();
    groups.iter().any(|g| g.iter().all(|w| hay.contains(w)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_box_is_an_or_of_ands() {
        assert_eq!(terms("仕事 週報"), vec![vec!["仕事".to_string(), "週報".to_string()]]);
        assert_eq!(
            terms("仕事 OR 家"),
            vec![vec!["仕事".to_string()], vec!["家".to_string()]]
        );
        // OR のほうが弱い綴じ ── (仕事 AND 週報) OR (家)。
        assert_eq!(
            terms("仕事 週報 or 家"),
            vec![vec!["仕事".to_string(), "週報".to_string()], vec!["家".to_string()]]
        );
        // 打ちかけの OR は、何にでも当たる空の組にしない。
        assert_eq!(terms("OR"), Vec::<Vec<String>>::new());
        assert_eq!(terms("仕事 OR OR 家").len(), 2);
        // 全角の縦棒も同じ ── 日本語キーボードで切り替えずに出るのはこちら。
        assert_eq!(terms("あ ｜ い").len(), 2);

        assert!(hits("週報 仕事 定型", "仕事 週報"));
        assert!(!hits("週報 仕事", "仕事 買い物"));
        assert!(hits("買うもの 家", "仕事 OR 家"));
        assert!(hits("なんでも", ""), "空の問いは全部に当たる");
        // 大文字小文字は問わない。
        assert!(hits("Weekly Report", "weekly"));
    }
}
