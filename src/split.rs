//! Split rows: a diff's old and new lines facing each other, for the side-by-side view.

use std::collections::HashMap;

use crate::diff::{Row, change_blocks};
use crate::model::Side;

/// A split row: an unchanged line on both halves, or changed lines facing each other.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Pair {
    /// An unchanged line: one context row, painted on both halves.
    Same(Row),
    /// A deletion facing an insertion; a blank half is `None`.
    Changed { old: Option<Row>, new: Option<Row> },
}

impl Pair {
    /// The row the half on `side` paints; a blank half is `None`.
    #[must_use]
    pub fn half(&self, side: Side) -> Option<&Row> {
        match (self, side) {
            (Pair::Same(row), _) => Some(row),
            (Pair::Changed { old, .. }, Side::Old) => old.as_ref(),
            (Pair::Changed { new, .. }, Side::New) => new.as_ref(),
        }
    }
}

/// One screen row of the diff pane: a unified row, or a split pair.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Visible {
    Row(Row),
    Pair(Pair),
}

impl Visible {
    /// The unified row; a split pair is none.
    #[must_use]
    pub fn row(&self) -> Option<&Row> {
        match self {
            Visible::Row(row) => Some(row),
            Visible::Pair(_) => None,
        }
    }

    /// The line on `side` this row holds, if any.
    #[must_use]
    pub fn no(&self, side: Side) -> Option<u32> {
        match self {
            Visible::Row(row) => row.no(side),
            Visible::Pair(p) => p.half(side)?.no(side),
        }
    }

    /// The old-side line on this row, if any.
    #[must_use]
    pub fn old_no(&self) -> Option<u32> {
        self.no(Side::Old)
    }

    /// The new-side line on this row, if any.
    #[must_use]
    pub fn new_no(&self) -> Option<u32> {
        self.no(Side::New)
    }

    /// Whether this row anchors a comment: every kind but a fold.
    #[must_use]
    pub fn is_content(&self) -> bool {
        self.row().is_none_or(Row::is_content)
    }

    /// The hidden line count of a fold, else 0.
    #[must_use]
    pub fn hidden(&self) -> usize {
        self.row().map_or(0, Row::hidden)
    }

    /// A fold's identity across rebuilds, else `None`.
    #[must_use]
    pub fn fold_anchor(&self) -> Option<u32> {
        self.row().and_then(Row::fold_anchor)
    }

    /// The content row a copy or a click reads: a split row's half on `side`, never a fold or gap.
    #[must_use]
    pub fn text_row(&self, side: Option<Side>) -> Option<&Row> {
        match self {
            Visible::Row(row) => row.is_content().then_some(row),
            Visible::Pair(p) => p.half(side?),
        }
    }

    /// The text a copy reads here, per [`Visible::text_row`].
    #[must_use]
    pub fn content_text(&self, side: Option<Side>) -> Option<String> {
        self.text_row(side).map(Row::text)
    }
}

/// A diff's rows as split rows: each change block's anchors face each other, the rest by position.
#[must_use]
pub fn split_rows(rows: Vec<Row>, similar: &[(u32, u32)]) -> Vec<Visible> {
    // Each old line pairs at most once, so one lookup per deletion.
    let similar: HashMap<u32, u32> = similar.iter().copied().collect();
    let blocks = change_blocks(&rows);
    let mut out = Vec::with_capacity(rows.len());
    let mut rows = rows.into_iter();
    let mut i = 0;
    for (dels, inss) in blocks {
        out.extend(rows.by_ref().take(dels.start - i).map(unchanged));
        let old: Vec<Row> = rows.by_ref().take(dels.len()).collect();
        let new: Vec<Row> = rows.by_ref().take(inss.len()).collect();
        face(old, new, &similar, &mut out);
        i = inss.end;
    }
    out.extend(rows.map(unchanged));
    out
}

/// A row outside any change block: a context line on both halves, or a fold as is.
fn unchanged(row: Row) -> Visible {
    match row {
        Row::Context { .. } => Visible::Pair(Pair::Same(row)),
        _ => Visible::Row(row),
    }
}

/// Push one block's pairs: anchors facing, the gaps between them filled by position.
fn face(dels: Vec<Row>, inss: Vec<Row>, similar: &HashMap<u32, u32>, out: &mut Vec<Visible>) {
    let anchors = anchors(&dels, &inss, similar);
    let (nd, ni) = (dels.len(), inss.len());
    let mut dels: Vec<Option<Row>> = dels.into_iter().map(Some).collect();
    let mut inss: Vec<Option<Row>> = inss.into_iter().map(Some).collect();
    let (mut k, mut j) = (0, 0);
    for (ak, aj) in anchors.into_iter().chain([(nd, ni)]) {
        for g in 0..(ak - k).max(aj - j) {
            let old = if k + g < ak { dels[k + g].take() } else { None };
            let new = if j + g < aj { inss[j + g].take() } else { None };
            out.push(pair(old, new, similar));
        }
        if ak < nd {
            out.push(pair(dels[ak].take(), inss[aj].take(), similar));
        }
        (k, j) = (ak + 1, aj + 1);
    }
}

/// A changed pair; emphasis paints only on a similar pair that faces.
fn pair(old: Option<Row>, new: Option<Row>, similar: &HashMap<u32, u32>) -> Visible {
    let faced = match (old.as_ref().and_then(Row::old_no), new.as_ref().and_then(Row::new_no)) {
        (Some(o), Some(n)) => similar.get(&o) == Some(&n),
        _ => false,
    };
    let plain = |mut row: Row| {
        if let Row::Deletion { emphasis, .. } | Row::Insertion { emphasis, .. } = &mut row {
            emphasis.clear();
        }
        row
    };
    let (old, new) = if faced { (old, new) } else { (old.map(plain), new.map(plain)) };
    Visible::Pair(Pair::Changed { old, new })
}

/// A block's `(deletion, insertion)` anchors: similar pairs and unique twins, the longest uncrossed.
fn anchors(dels: &[Row], inss: &[Row], similar: &HashMap<u32, u32>) -> Vec<(usize, usize)> {
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
        if let Some(&j) = d.old_no().and_then(|o| similar.get(&o)).and_then(|n| ins_at.get(n)) {
            found.push((k, j));
        }
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

    /// Each changed pair's `(old, new)` line texts, in order.
    fn changed(rows: &[Visible]) -> Vec<(Option<String>, Option<String>)> {
        rows.iter()
            .filter_map(|v| match v {
                Visible::Pair(Pair::Changed { old, new }) => {
                    Some((old.as_ref().map(Row::text), new.as_ref().map(Row::text)))
                }
                _ => None,
            })
            .collect()
    }

    /// Whether either half of a split row carries word emphasis.
    fn emphasized(v: &Visible) -> bool {
        let Visible::Pair(Pair::Changed { old, new }) = v else { return false };
        old.iter().chain(new).any(|r| !r.emphasis().is_empty())
    }

    /// Every line of `diff` appears in exactly one split row, on its own side, in order.
    fn assert_each_line_once(diff: &FileDiff, rows: &[Visible]) {
        let mut want: (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
        for row in &diff.rows {
            for line in
                if let Row::Fold { lines } = row { lines.clone() } else { vec![row.clone()] }
            {
                want.0.extend(line.old_no());
                want.1.extend(line.new_no());
            }
        }
        let mut got: (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
        for v in rows {
            match v {
                Visible::Pair(_) => {
                    got.0.extend(v.old_no());
                    got.1.extend(v.new_no());
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
        let rows = split_rows(d.rows.clone(), &d.similar);
        assert_each_line_once(&d, &rows);
        assert_eq!(
            changed(&rows),
            [
                (Some("alpha one".into()), Some("zzz".into())),
                (Some("beta two".into()), Some("yyy".into())),
                (Some("gamma three".into()), None),
            ]
        );
        assert!(matches!(&rows[0], Visible::Pair(Pair::Same(_))), "context stays");
    }

    #[test]
    fn a_similar_pair_faces_its_partner_past_an_inserted_line() {
        let d = build("let total = sum + 1;\n", "let fresh = 0;\nlet total = sum + 2;\n");
        let rows = split_rows(d.rows.clone(), &d.similar);
        assert_each_line_once(&d, &rows);
        assert_eq!(
            changed(&rows),
            [
                (None, Some("let fresh = 0;".into())),
                (Some("let total = sum + 1;".into()), Some("let total = sum + 2;".into())),
            ]
        );
        assert!(emphasized(&rows[1]), "a facing similar pair keeps emphasis");
    }

    #[test]
    fn a_unique_twin_anchors_past_an_inserted_line() {
        // The endings differ, so the twins stay in the block; position alone would stagger them.
        let d = build("a\r\nb\r\n", "x\na\nb\n");
        let rows = split_rows(d.rows.clone(), &d.similar);
        assert_each_line_once(&d, &rows);
        assert_eq!(
            changed(&rows),
            [
                (None, Some("x".into())),
                (Some("a".into()), Some("a".into())),
                (Some("b".into()), Some("b".into())),
            ]
        );
    }

    #[test]
    fn the_longest_non_crossing_anchors_win_over_the_first() {
        // A moved twin leads the old side, crossing three similar pairs; the three win.
        let old = "unique_line();\r\nlet total1 = sum + 1;\nlet total2 = sum + 1;\nlet total3 = sum + 1;\n";
        let new =
            "let total1 = sum + 2;\nlet total2 = sum + 2;\nlet total3 = sum + 2;\nunique_line();\n";
        let d = build(old, new);
        let rows = split_rows(d.rows.clone(), &d.similar);
        assert_each_line_once(&d, &rows);
        let pair = |i: usize| {
            (Some(format!("let total{i} = sum + 1;")), Some(format!("let total{i} = sum + 2;")))
        };
        assert_eq!(
            changed(&rows),
            [
                (Some("unique_line();".into()), None),
                pair(1),
                pair(2),
                pair(3),
                (None, Some("unique_line();".into())),
            ]
        );
    }

    #[test]
    fn a_crossing_twin_never_crosses_and_a_dropped_pair_loses_emphasis() {
        // The ending change keeps the twin inside the block, where it crosses the similar pair.
        let d = build(
            "let total = sum + 1;\nunique_line();\r\n",
            "unique_line();\nlet total = sum + 2;\n",
        );
        assert_eq!(d.pairs.len(), 2, "pair_homologs crosses them: {:?}", d.pairs);
        let rows = split_rows(d.rows.clone(), &d.similar);
        assert_each_line_once(&d, &rows);
        for v in &rows {
            let Visible::Pair(Pair::Changed { old, new }) = v else { continue };
            let faces_partner = old.as_ref().map(Row::text).as_deref()
                == Some("let total = sum + 1;")
                && new.as_ref().map(Row::text).as_deref() == Some("let total = sum + 2;");
            assert_eq!(emphasized(v), faces_partner, "emphasis only where the pair faces: {v:?}");
        }
    }

    #[test]
    fn a_line_ending_change_faces_its_twin_with_the_cr_marker() {
        let d = build("a\r\nb\r\n", "a\nb\n");
        let rows = split_rows(d.rows.clone(), &d.similar);
        assert_each_line_once(&d, &rows);
        assert_eq!(
            changed(&rows),
            [(Some("a".into()), Some("a".into())), (Some("b".into()), Some("b".into()))]
        );
        let Visible::Pair(Pair::Changed { old: Some(old), .. }) = &rows[0] else { panic!() };
        assert!(old.cr_marker(), "the old half keeps its CR marker");
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
        let rows = split_rows(d.rows.clone(), &d.similar);
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
