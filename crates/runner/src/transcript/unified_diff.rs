//! Hunks and lines from the output of `git diff` for one file.

use nullradix_app::session_detail::{DiffHunk, DiffLine, DiffLineKind, FileDiff, MAX_DIFF_LINES};

use super::line_diff;

/// Hunks of the patch, cut after [`MAX_DIFF_LINES`] lines.
pub(crate) fn parse(path: &str, output: &str) -> FileDiff {
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut count = 0usize;
    let mut truncated = false;
    for raw in output.lines() {
        if raw.starts_with("@@") {
            if count >= MAX_DIFF_LINES {
                truncated = true;
                break;
            }
            hunks.push(DiffHunk {
                header: raw.to_owned(),
                lines: Vec::new(),
            });
            continue;
        }
        // the patch head before the first hunk
        let Some(hunk) = hunks.last_mut() else {
            continue;
        };
        let kind = match raw.chars().next() {
            Some('+') => DiffLineKind::Add,
            Some('-') => DiffLineKind::Remove,
            Some(' ') | None => DiffLineKind::Context,
            // "\ No newline at end of file" and anything else
            _ => continue,
        };
        if count >= MAX_DIFF_LINES {
            truncated = true;
            break;
        }
        hunk.lines.push(DiffLine {
            kind,
            text: raw.get(1..).unwrap_or("").to_owned(),
        });
        count += 1;
    }
    FileDiff {
        path: path.to_owned(),
        hunks,
        truncated,
    }
}

/// Diff of a new file: every line added; `note` follows the header.
pub(crate) fn all_added(path: &str, content: &str, note: Option<&str>) -> FileDiff {
    let mut lines: Vec<DiffLine> = content
        .lines()
        .map(|text| DiffLine {
            kind: DiffLineKind::Add,
            text: text.to_owned(),
        })
        .collect();
    let count = lines.len();
    let truncated = count > MAX_DIFF_LINES;
    lines.truncate(MAX_DIFF_LINES);
    let mut header = line_diff::header(1, 0, 1, count);
    if let Some(note) = note {
        header.push(' ');
        header.push_str(note);
    }
    FileDiff {
        path: path.to_owned(),
        hunks: if lines.is_empty() {
            Vec::new()
        } else {
            vec![DiffHunk { header, lines }]
        },
        truncated,
    }
}

/// The hunks cut to [`MAX_DIFF_LINES`] lines.
pub(crate) fn limit(path: &str, hunks: Vec<DiffHunk>) -> FileDiff {
    let mut left = MAX_DIFF_LINES;
    let mut kept = Vec::new();
    let mut truncated = false;
    for mut hunk in hunks {
        if left == 0 {
            truncated = true;
            break;
        }
        if hunk.lines.len() > left {
            hunk.lines.truncate(left);
            truncated = true;
        }
        left -= hunk.lines.len();
        kept.push(hunk);
        if truncated {
            break;
        }
    }
    FileDiff {
        path: path.to_owned(),
        hunks: kept,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use super::*;

    const PATCH: &str = "diff --git a/src/a.txt b/src/a.txt\n\
index 1111111..2222222 100644\n\
--- a/src/a.txt\n\
+++ b/src/a.txt\n\
@@ -1,3 +1,3 @@ fn head\n \
one\n\
-two\n\
+TWO\n \
three\n\
@@ -10,2 +10,3 @@\n \
ten\n\
+new\n \
eleven\n\
\\ No newline at end of file\n";

    #[test]
    fn reads_hunks_and_lines_without_prefix() {
        let diff = parse("src/a.txt", PATCH);
        assert_eq!(diff.path, "src/a.txt");
        assert!(!diff.truncated);
        assert_eq!(diff.hunks.len(), 2);
        assert_eq!(diff.hunks[0].header, "@@ -1,3 +1,3 @@ fn head");
        assert_eq!(diff.hunks[0].lines.len(), 4);
        assert_eq!(diff.hunks[0].lines[1].kind, DiffLineKind::Remove);
        assert_eq!(diff.hunks[0].lines[1].text, "two");
        assert_eq!(diff.hunks[0].lines[2].kind, DiffLineKind::Add);
        assert_eq!(diff.hunks[1].lines.len(), 3);
    }

    #[test]
    fn binary_files_have_no_hunks() {
        let diff = parse(
            "b.bin",
            "diff --git a/b.bin b/b.bin\nBinary files a/b.bin and b/b.bin differ\n",
        );
        assert_eq!(diff.hunks, Vec::new());
    }

    #[test]
    fn cuts_very_long_diffs() {
        let mut patch = String::from("@@ -0,0 +1,6000 @@\n");
        for n in 0..6000 {
            writeln!(patch, "+{n}").unwrap();
        }
        let diff = parse("x", &patch);
        assert!(diff.truncated);
        assert_eq!(diff.hunks[0].lines.len(), MAX_DIFF_LINES);
        let cut = limit("x", diff.hunks.clone());
        assert!(!cut.truncated);
        let mut more = diff.hunks.clone();
        more.extend(diff.hunks);
        assert!(limit("x", more).truncated);
    }

    #[test]
    fn a_new_file_is_all_added() {
        let diff = all_added("n.txt", "a\nb\n", None);
        assert_eq!(diff.hunks[0].header, "@@ -0,0 +1,2 @@");
        assert!(
            diff.hunks[0]
                .lines
                .iter()
                .all(|line| line.kind == DiffLineKind::Add)
        );
        assert_eq!(all_added("e.txt", "", None).hunks, Vec::new());
    }
}
