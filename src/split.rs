//! Split rows: a diff's old and new lines facing each other, for the side-by-side view.

use std::collections::{HashMap, HashSet};

use crate::diff::{CharRange, FileDiff, Row, Span, change_blocks};

/// One side's line, as a split half paints it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Half {
    pub no: u32,
    pub spans: Vec<Span>,
    pub emphasis: Vec<CharRange>,
    pub cr: bool,
}

/// A split row: an unchanged line on both halves, or changed lines facing each other.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pair {
    pub old: Option<Half>,
    pub new: Option<Half>,
    /// `false` for an unchanged line shown on both halves.
    pub changed: bool,
}

/// One screen row of the diff pane: a unified row, or a split pair.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Visible {
    Row(Row),
    Pair(Pair),
}

/// `diff`'s split rows: each change block's anchors face each other, the rest face by position.
#[must_use]
pub fn split_rows(diff: &FileDiff) -> Vec<Visible> {
    let similar: HashSet<(u32, u32)> = diff.similar.iter().copied().collect();
    let rows = &diff.rows;
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0;
    for (dels, inss) in change_blocks(rows) {
        out.extend(rows[i..dels.start].iter().map(unchanged));
        face(&rows[dels.clone()], &rows[inss.clone()], &similar, &mut out);
        i = inss.end;
    }
    out.extend(rows[i..].iter().map(unchanged));
    out
}

/// A row outside any change block: a context line on both halves, or a fold as is.
fn unchanged(row: &Row) -> Visible {
    match row {
        Row::Context { old_no, new_no, spans } => {
            let half = |no: u32| Half { no, spans: spans.clone(), emphasis: Vec::new(), cr: false };
            Visible::Pair(Pair { old: Some(half(*old_no)), new: Some(half(*new_no)), changed: false })
        }
        _ => Visible::Row(row.clone()),
    }
}

/// Push one block's pairs: anchors facing, the gaps between them filled by position.
fn face(dels: &[Row], inss: &[Row], similar: &HashSet<(u32, u32)>, out: &mut Vec<Visible>) {
    let (mut k, mut j) = (0, 0);
    for (ak, aj) in anchors(dels, inss, similar).into_iter().chain([(dels.len(), inss.len())]) {
        for g in 0..(ak - k).max(aj - j) {
            out.push(pair(dels[k..ak].get(g), inss[j..aj].get(g), similar));
        }
        if ak < dels.len() {
            out.push(pair(dels.get(ak), inss.get(aj), similar));
        }
        (k, j) = (ak + 1, aj + 1);
    }
}

/// A changed pair; emphasis paints only on a similar pair that faces.
fn pair(old: Option<&Row>, new: Option<&Row>, similar: &HashSet<(u32, u32)>) -> Visible {
    let numbers = old.and_then(Row::old_no).zip(new.and_then(Row::new_no));
    let faced = numbers.is_some_and(|n| similar.contains(&n));
    let half = |row: &Row, no: Option<u32>| Half {
        no: no.unwrap_or_default(),
        spans: row.spans().to_vec(),
        emphasis: if faced { row.emphasis().to_vec() } else { Vec::new() },
        cr: row.cr_marker(),
    };
    Visible::Pair(Pair {
        old: old.map(|r| half(r, r.old_no())),
        new: new.map(|r| half(r, r.new_no())),
        changed: true,
    })
}

/// The block's anchors as `(deletion, insertion)` indices: similar pairs and twins unique to
/// both sides (patience), keeping the longest set that never crosses.
fn anchors(dels: &[Row], inss: &[Row], similar: &HashSet<(u32, u32)>) -> Vec<(usize, usize)> {
    let (old, new): (Vec<String>, Vec<String>) =
        (dels.iter().map(Row::text).collect(), inss.iter().map(Row::text).collect());
    let mut counts: HashMap<&str, (usize, usize)> = HashMap::new();
    for text in &old {
        counts.entry(text).or_default().0 += 1;
    }
    for text in &new {
        counts.entry(text).or_default().1 += 1;
    }
    let new_at: HashMap<&str, usize> =
        new.iter().enumerate().map(|(j, text)| (text.as_str(), j)).collect();
    let ins_at: HashMap<u32, usize> =
        inss.iter().enumerate().filter_map(|(j, r)| Some((r.new_no()?, j))).collect();
    let mut found = Vec::new();
    for (k, d) in dels.iter().enumerate() {
        if counts[old[k].as_str()] == (1, 1) {
            found.push((k, new_at[old[k].as_str()]));
        }
        let Some(o) = d.old_no() else { continue };
        found.extend(similar.iter().filter(|p| p.0 == o).filter_map(|p| Some((k, *ins_at.get(&p.1)?))));
    }
    // Within one deletion, larger insertions first, so the increasing run keeps at most one.
    found.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    found.dedup();
    longest_increasing(&found)
}

/// The longest run of `found` whose insertion indices strictly increase.
fn longest_increasing(found: &[(usize, usize)]) -> Vec<(usize, usize)> {
    // `tails[n]`: the index in `found` ending the best run of length `n + 1`.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![None; found.len()];
    for (i, &(_, j)) in found.iter().enumerate() {
        let n = tails.partition_point(|&t| found[t].1 < j);
        prev[i] = n.checked_sub(1).map(|m| tails[m]);
        if n == tails.len() {
            tails.push(i);
        } else {
            tails[n] = i;
        }
    }
    let mut run = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(i) = at {
        run.push(found[i]);
        at = prev[i];
    }
    run.reverse();
    run
}

#[cfg(test)]
mod tests {
    use super::{Pair, Visible, split_rows};
    use crate::diff::{FileDiff, Row};
    use crate::highlight::Highlighter;
    use crate::theme;
    use std::fmt::Write as _;

    fn build(old: &str, new: &str) -> FileDiff {
        let hl = Highlighter::new(theme::resolve(Some("catppuccin")).syntax);
        FileDiff::build("a.rs".into(), None, old, new, &hl)
    }

    fn text(half: Option<&super::Half>) -> Option<String> {
        half.map(|h| h.spans.iter().map(|s| s.text.as_str()).collect())
    }

    /// Each changed pair's `(old, new)` line texts, in order.
    fn changed(rows: &[Visible]) -> Vec<(Option<String>, Option<String>)> {
        rows.iter()
            .filter_map(|v| match v {
                Visible::Pair(p @ Pair { changed: true, .. }) => {
                    Some((text(p.old.as_ref()), text(p.new.as_ref())))
                }
                _ => None,
            })
            .collect()
    }

    /// Every line of `diff` appears in exactly one split row, on its own side, in order.
    fn assert_each_line_once(diff: &FileDiff, rows: &[Visible]) {
        let mut want: (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
        for row in &diff.rows {
            for line in if let Row::Fold { lines } = row { lines.clone() } else { vec![row.clone()] } {
                want.0.extend(line.old_no());
                want.1.extend(line.new_no());
            }
        }
        let mut got: (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
        for v in rows {
            match v {
                Visible::Pair(p) => {
                    got.0.extend(p.old.as_ref().map(|h| h.no));
                    got.1.extend(p.new.as_ref().map(|h| h.no));
                }
                Visible::Row(Row::Fold { lines }) => {
                    got.0.extend(lines.iter().filter_map(Row::old_no));
                    got.1.extend(lines.iter().filter_map(Row::new_no));
                }
                Visible::Row(other) => panic!("a stray unified row: {other:?}"),
            }
        }
        assert_eq!(got, want, "each line once, in order");
    }

    #[test]
    fn a_rewrite_faces_its_lines_by_position() {
        let d = build("keep\nalpha one\nbeta two\ngamma three\n", "keep\nzzz\nyyy\n");
        let rows = split_rows(&d);
        assert_each_line_once(&d, &rows);
        assert_eq!(
            changed(&rows),
            [
                (Some("alpha one".into()), Some("zzz".into())),
                (Some("beta two".into()), Some("yyy".into())),
                (Some("gamma three".into()), None),
            ]
        );
        assert!(matches!(&rows[0], Visible::Pair(Pair { changed: false, .. })), "context stays");
    }

    #[test]
    fn a_similar_pair_faces_its_partner_past_an_inserted_line() {
        let d = build("let total = sum + 1;\n", "let fresh = 0;\nlet total = sum + 2;\n");
        let rows = split_rows(&d);
        assert_each_line_once(&d, &rows);
        assert_eq!(
            changed(&rows),
            [
                (None, Some("let fresh = 0;".into())),
                (Some("let total = sum + 1;".into()), Some("let total = sum + 2;".into())),
            ]
        );
        let Visible::Pair(p) = &rows[1] else { panic!() };
        assert!(!p.new.as_ref().unwrap().emphasis.is_empty(), "a facing similar pair keeps emphasis");
    }

    #[test]
    fn a_crossing_twin_never_crosses_and_a_dropped_pair_loses_emphasis() {
        // The ending change keeps the twin inside the block, where it crosses the similar pair.
        let d = build("let total = sum + 1;\nunique_line();\r\n", "unique_line();\nlet total = sum + 2;\n");
        assert_eq!(d.pairs.len(), 2, "pair_homologs crosses them: {:?}", d.pairs);
        let rows = split_rows(&d);
        assert_each_line_once(&d, &rows);
        for v in &rows {
            let Visible::Pair(p) = v else { continue };
            let (old, new) = (text(p.old.as_ref()), text(p.new.as_ref()));
            let faces_partner = old.as_deref() == Some("let total = sum + 1;")
                && new.as_deref() == Some("let total = sum + 2;");
            let emphasized = p.old.iter().chain(&p.new).any(|h| !h.emphasis.is_empty());
            assert_eq!(emphasized, faces_partner, "emphasis only where the similar pair faces: {p:?}");
        }
    }

    #[test]
    fn a_line_ending_change_faces_its_twin_with_the_cr_marker() {
        let d = build("a\r\nb\r\n", "a\nb\n");
        let rows = split_rows(&d);
        assert_each_line_once(&d, &rows);
        assert_eq!(
            changed(&rows),
            [(Some("a".into()), Some("a".into())), (Some("b".into()), Some("b".into()))]
        );
        let Visible::Pair(p) = &rows[0] else { panic!() };
        assert!(p.old.as_ref().unwrap().cr, "the old half keeps its CR marker");
    }

    #[test]
    fn a_large_rewrite_full_of_brace_and_blank_twins_stays_aligned() {
        // Over 512 lines a side, similar keeps shared `}` and blank lines inside one block.
        let (mut old, mut new) = (String::new(), String::new());
        for i in 0..800 {
            writeln!(old, "    let a{i} = compute_old({i});").unwrap();
            if i % 5 == 0 {
                old.push_str("    }\n\n");
            }
        }
        for i in 0..800 {
            writeln!(new, "    let b{i} = other_new({i}, x);").unwrap();
            if i % 7 == 0 {
                new.push_str("    }\n\n");
            }
        }
        let d = build(&old, &new);
        assert!(d.pairs.len() > d.similar.len() + 100, "the block holds many twins");
        let rows = split_rows(&d);
        assert_each_line_once(&d, &rows);
        // Non-unique twins never anchor, so no run of one-sided rows staggers the block.
        let mut one_sided = 0;
        let mut longest = 0;
        for (o, n) in changed(&rows) {
            one_sided = if o.is_none() || n.is_none() { one_sided + 1 } else { 0 };
            longest = longest.max(one_sided);
        }
        let (dels, inss) = d.rows.iter().fold((0usize, 0usize), |(a, b), r| match r {
            Row::Deletion { .. } => (a + 1, b),
            Row::Insertion { .. } => (a, b + 1),
            _ => (a, b),
        });
        assert!(longest <= dels.abs_diff(inss), "one-sided run {longest} beyond the size gap");
    }
}
