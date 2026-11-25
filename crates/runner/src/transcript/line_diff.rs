//! Line diff of two texts (common ends, longest common subsequence between) and its hunks.

use nullradix_app::session_detail::{DiffHunk, DiffLine, DiffLineKind};

/// Largest subsequence table (old lines times new lines); beyond, all goes and all comes.
const MAX_CELLS: usize = 4_000_000;
/// Context lines around each change.
pub(crate) const CONTEXT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op<'a> {
    Same(&'a str),
    Remove(&'a str),
    Add(&'a str),
}

/// Lines of a text without their newline; `\r\n` counts as `\n`.
pub(crate) fn split(text: &str) -> Vec<&str> {
    text.lines().collect()
}

/// Diff of the lines from `old` to `new`.
pub(crate) fn diff<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Op<'a>> {
    let prefix = old
        .iter()
        .zip(new.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let rest_old = &old[prefix..];
    let rest_new = &new[prefix..];
    let suffix = rest_old
        .iter()
        .rev()
        .zip(rest_new.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let middle_old = &rest_old[..rest_old.len() - suffix];
    let middle_new = &rest_new[..rest_new.len() - suffix];
    let mut ops: Vec<Op<'a>> = old[..prefix].iter().map(|line| Op::Same(line)).collect();
    ops.extend(middle(middle_old, middle_new));
    ops.extend(
        rest_old[rest_old.len() - suffix..]
            .iter()
            .map(|line| Op::Same(line)),
    );
    ops
}

/// Added and removed lines.
pub(crate) fn counts(ops: &[Op<'_>]) -> (i64, i64) {
    ops.iter().fold((0, 0), |(added, removed), op| match op {
        Op::Add(_) => (added + 1, removed),
        Op::Remove(_) => (added, removed + 1),
        Op::Same(_) => (added, removed),
    })
}

/// Added and removed lines from `old` to `new`.
pub(crate) fn text_counts(old: &str, new: &str) -> (i64, i64) {
    counts(&diff(&split(old), &split(new)))
}

/// Hunks with [`CONTEXT`] lines around each change; none without a change.
pub(crate) fn hunks(ops: &[Op<'_>]) -> Vec<DiffHunk> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (index, _) in ops
        .iter()
        .enumerate()
        .filter(|(_, op)| !matches!(op, Op::Same(_)))
    {
        let start = index.saturating_sub(CONTEXT);
        let end = (index + CONTEXT + 1).min(ops.len());
        match ranges.last_mut() {
            Some(last) if start <= last.1 => last.1 = end,
            _ => ranges.push((start, end)),
        }
    }
    ranges
        .into_iter()
        .map(|(start, end)| hunk(ops, start, end))
        .collect()
}

fn hunk(ops: &[Op<'_>], start: usize, end: usize) -> DiffHunk {
    let (mut old_line, mut new_line) = (1usize, 1usize);
    for op in &ops[..start] {
        match op {
            Op::Same(_) => {
                old_line += 1;
                new_line += 1;
            }
            Op::Remove(_) => old_line += 1,
            Op::Add(_) => new_line += 1,
        }
    }
    let lines: Vec<DiffLine> = ops[start..end].iter().map(|op| line(*op)).collect();
    let old_count = lines
        .iter()
        .filter(|line| line.kind != DiffLineKind::Add)
        .count();
    let new_count = lines
        .iter()
        .filter(|line| line.kind != DiffLineKind::Remove)
        .count();
    DiffHunk {
        header: header(old_line, old_count, new_line, new_count),
        lines,
    }
}

/// `@@ -a,b +c,d @@`; an empty range starts one line earlier, as in git.
pub(crate) fn header(
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
) -> String {
    let start = |line: usize, count: usize| if count == 0 { line - 1 } else { line };
    format!(
        "@@ -{},{} +{},{} @@",
        start(old_start, old_count),
        old_count,
        start(new_start, new_count),
        new_count
    )
}

pub(crate) fn line(op: Op<'_>) -> DiffLine {
    let (kind, text) = match op {
        Op::Same(text) => (DiffLineKind::Context, text),
        Op::Remove(text) => (DiffLineKind::Remove, text),
        Op::Add(text) => (DiffLineKind::Add, text),
    };
    DiffLine {
        kind,
        text: text.to_owned(),
    }
}

/// The middle part: longest common subsequence, or all removed and added when too large.
fn middle<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Op<'a>> {
    if old.is_empty() || new.is_empty() || old.len().saturating_mul(new.len()) > MAX_CELLS {
        let mut ops: Vec<Op<'a>> = old.iter().map(|line| Op::Remove(line)).collect();
        ops.extend(new.iter().map(|line| Op::Add(line)));
        return ops;
    }
    let (n, m) = (old.len(), new.len());
    // table[i][j] is the subsequence length of old[i..] and new[j..]
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[at(i, j)] = if old[i] == new[j] {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ops = Vec::with_capacity(n + m);
    while i < n && j < m {
        if old[i] == new[j] {
            ops.push(Op::Same(old[i]));
            i += 1;
            j += 1;
        } else if table[at(i + 1, j)] >= table[at(i, j + 1)] {
            ops.push(Op::Remove(old[i]));
            i += 1;
        } else {
            ops.push(Op::Add(new[j]));
            j += 1;
        }
    }
    ops.extend(old[i..].iter().map(|line| Op::Remove(line)));
    ops.extend(new[j..].iter().map(|line| Op::Add(line)));
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(hunk: &DiffHunk) -> String {
        hunk.lines
            .iter()
            .map(|line| match line.kind {
                DiffLineKind::Add => '+',
                DiffLineKind::Remove => '-',
                DiffLineKind::Context => ' ',
            })
            .collect()
    }

    #[test]
    fn finds_a_changed_line_with_context() {
        let old = split("a\nb\nc\nd\ne\nf\ng\nh\n");
        let new = split("a\nb\nc\nd\nE\nf\ng\nh\n");
        let ops = diff(&old, &new);
        assert_eq!(counts(&ops), (1, 1));
        let hunks = hunks(&ops);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].header, "@@ -2,7 +2,7 @@");
        assert_eq!(kinds(&hunks[0]), "   -+   ");
        assert_eq!(hunks[0].lines[3].text, "e");
        assert_eq!(hunks[0].lines[4].text, "E");
    }

    #[test]
    fn splits_changes_far_apart() {
        let old: Vec<String> = (1..=20).map(|n| n.to_string()).collect();
        let mut new = old.clone();
        new[1] = "two".into();
        new[17] = "eighteen".into();
        let old: Vec<&str> = old.iter().map(String::as_str).collect();
        let new: Vec<&str> = new.iter().map(String::as_str).collect();
        let hunks = hunks(&diff(&old, &new));
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].header, "@@ -1,5 +1,5 @@");
        assert_eq!(hunks[1].header, "@@ -15,6 +15,6 @@");
    }

    #[test]
    fn a_new_file_starts_at_zero() {
        let ops = diff(&[], &split("x\ny"));
        assert_eq!(counts(&ops), (2, 0));
        assert_eq!(hunks(&ops)[0].header, "@@ -0,0 +1,2 @@");
    }

    #[test]
    fn no_change_no_hunks() {
        let lines = split("a\nb");
        assert_eq!(hunks(&diff(&lines, &lines)), Vec::new());
    }

    #[test]
    fn finds_lines_inserted_in_the_middle() {
        assert_eq!(text_counts("a\nb\nc\nd", "a\nx\nb\nc\ny\nd"), (2, 0));
    }
}
