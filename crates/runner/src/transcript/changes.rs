//! Changes of a session from its transcript: successful edits and writes folded per file.

use std::collections::HashMap;

use nullradix_app::session_detail::{ChangedFile, DiffHunk, FileDiff, FileStatus};
use serde_json::Value;

use super::jsonl::{count, flag, text};
use super::line_diff;
use super::summary::relative;
use super::unified_diff;

/// Note on hunks whose base is unknown.
const UNKNOWN_BASE: &str = "base unknown";
/// Header of a fragment without line numbers.
const FRAGMENT_HEADER: &str = "@@ fragment @@";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Base {
    Unknown,
    Missing,
    Text(String),
}

#[derive(Debug, Clone)]
struct FileState {
    base: Base,
    current: Option<String>,
    /// Replacements that applied to no known state.
    fragments: Vec<(String, String)>,
}

/// Collects the changes line by line.
#[derive(Debug, Clone)]
pub(crate) struct TranscriptChanges {
    cwd: String,
    files: HashMap<String, FileState>,
    /// Calls of changing tools without result: id to name and input.
    pending: HashMap<String, (String, Value)>,
}

impl TranscriptChanges {
    pub(crate) fn new(cwd: &str) -> Self {
        Self {
            cwd: cwd.replace('\\', "/"),
            files: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    /// Takes a line; subagent lines count too, their changes are the session's.
    pub(crate) fn feed(&mut self, value: &Value) {
        match text(value, "type") {
            Some("assistant") => self.calls(value),
            Some("user") => self.results(value),
            _ => {}
        }
    }

    /// The changed files by path.
    pub(crate) fn files(&self) -> Vec<ChangedFile> {
        let mut list: Vec<ChangedFile> = self
            .files
            .iter()
            .filter_map(|(path, state)| changed(path, state))
            .collect();
        list.sort_by(|a, b| a.path.cmp(&b.path));
        list
    }

    /// Diff of a changed file; `None` for a path that did not change.
    pub(crate) fn diff(&self, path: &str) -> Option<FileDiff> {
        let state = self.files.get(path)?;
        changed(path, state)?;
        let mut hunks = match (&state.base, &state.current) {
            (Base::Text(base), Some(current)) => line_diff::hunks(&line_diff::diff(
                &line_diff::split(base),
                &line_diff::split(current),
            )),
            (Base::Missing, Some(current)) => unified_diff::all_added(path, current, None).hunks,
            (Base::Unknown, Some(current)) => {
                unified_diff::all_added(path, current, Some(UNKNOWN_BASE)).hunks
            }
            _ => Vec::new(),
        };
        hunks.extend(state.fragments.iter().map(|(old, new)| fragment(old, new)));
        Some(unified_diff::limit(path, hunks))
    }

    fn calls(&mut self, value: &Value) {
        let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for block in blocks {
            let name = text(block, "name").unwrap_or("");
            if text(block, "type") == Some("tool_use")
                && matches!(name, "Edit" | "MultiEdit" | "Write" | "NotebookEdit")
                && let Some(id) = text(block, "id")
            {
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                self.pending.insert(id.to_owned(), (name.to_owned(), input));
            }
        }
    }

    /// Applies successful results; `toolUseResult` counts only beside exactly one result.
    fn results(&mut self, value: &Value) {
        let Some(parts) = value.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        let results: Vec<&Value> = parts
            .iter()
            .filter(|part| text(part, "type") == Some("tool_result"))
            .collect();
        let details = if results.len() == 1 {
            value.get("toolUseResult").unwrap_or(&Value::Null)
        } else {
            &Value::Null
        };
        for part in results {
            let Some(id) = text(part, "tool_use_id") else {
                continue;
            };
            let Some((name, input)) = self.pending.remove(id) else {
                continue;
            };
            if !flag(part, "is_error") {
                self.apply(&name, &input, details);
            }
        }
    }

    fn apply(&mut self, name: &str, input: &Value, details: &Value) {
        let key = if name == "NotebookEdit" {
            "notebook_path"
        } else {
            "file_path"
        };
        let Some(path) = text(input, key).and_then(|path| self.inside(path)) else {
            return;
        };
        let original = details.get("originalFile").and_then(Value::as_str);
        let created = text(details, "type") == Some("create");
        let state = self.files.entry(path).or_insert_with(|| FileState {
            base: match (created, original) {
                (true, _) => Base::Missing,
                (false, Some(text)) => Base::Text(text.to_owned()),
                (false, None) => Base::Unknown,
            },
            current: if created {
                Some(String::new())
            } else {
                original.map(str::to_owned)
            },
            fragments: Vec::new(),
        });
        match name {
            "Write" => {
                state.current = Some(
                    input
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                );
                state.fragments.clear();
            }
            "Edit" => {
                if let Some(original) = original {
                    state.current = Some(original.to_owned());
                }
                replace(state, input);
            }
            "MultiEdit" => {
                if let Some(original) = original {
                    state.current = Some(original.to_owned());
                }
                for edit in input
                    .get("edits")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    replace(state, edit);
                }
            }
            _ => {
                let source = text(input, "new_source").unwrap_or("").to_owned();
                state.fragments.push((String::new(), source));
            }
        }
    }

    /// The path relative to the session folder when the file lies inside it.
    fn inside(&self, path: &str) -> Option<String> {
        let normalized = path.replace('\\', "/");
        let rel = relative(&normalized, Some(&self.cwd));
        let absolute = normalized.starts_with('/') || normalized.contains(':');
        if absolute && rel == normalized {
            return None;
        }
        (!rel.split('/').any(|part| part == "..")).then_some(rel)
    }
}

/// Applies `old_string` to `new_string` on the current state; else it stays a fragment.
fn replace(state: &mut FileState, edit: &Value) {
    let old = edit.get("old_string").and_then(Value::as_str).unwrap_or("");
    let new = edit.get("new_string").and_then(Value::as_str).unwrap_or("");
    let all = flag(edit, "replace_all");
    match state.current.as_mut() {
        Some(current) if old.is_empty() && current.is_empty() => new.clone_into(current),
        Some(current) if !old.is_empty() && current.contains(old) => {
            *current = if all {
                current.replace(old, new)
            } else {
                current.replacen(old, new, 1)
            };
        }
        _ => state.fragments.push((old.to_owned(), new.to_owned())),
    }
}

/// The file with its counts when it changed.
fn changed(path: &str, state: &FileState) -> Option<ChangedFile> {
    let (mut added, mut removed) = (0, 0);
    let mut status = FileStatus::Modified;
    match (&state.base, &state.current) {
        (Base::Text(base), Some(current)) => {
            if base != current || !state.fragments.is_empty() {
                (added, removed) = line_diff::text_counts(base, current);
            }
        }
        (Base::Missing, Some(current)) => {
            status = FileStatus::Added;
            added = count(current.lines().count());
        }
        (Base::Unknown, Some(current)) => added = count(current.lines().count()),
        _ => {}
    }
    for (old, new) in &state.fragments {
        let counts = line_diff::text_counts(old, new);
        added += counts.0;
        removed += counts.1;
    }
    let reverted = matches!(
        (&state.base, &state.current),
        (Base::Text(base), Some(current)) if base == current
    ) && state.fragments.is_empty();
    if reverted {
        return None;
    }
    Some(ChangedFile {
        path: path.to_owned(),
        status,
        added,
        removed,
    })
}

/// A fragment old to new as a hunk without line numbers.
fn fragment(old: &str, new: &str) -> DiffHunk {
    let ops = line_diff::diff(&line_diff::split(old), &line_diff::split(new));
    DiffHunk {
        header: FRAGMENT_HEADER.to_owned(),
        lines: ops.into_iter().map(line_diff::line).collect(),
    }
}

#[cfg(test)]
mod tests {
    use nullradix_app::session_detail::DiffLineKind;
    use serde_json::json;

    use super::super::fixtures::{self, file, tool_result, tool_use};
    use super::*;

    fn collect(content: &str) -> TranscriptChanges {
        let mut changes = TranscriptChanges::new(fixtures::CWD);
        for line in content.lines() {
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                changes.feed(&value);
            }
        }
        changes
    }

    fn calls(pairs: Vec<(Value, Value)>) -> String {
        pairs
            .into_iter()
            .flat_map(|(call, result)| [fixtures::line(&call), fixtures::line(&result)])
            .collect()
    }

    fn signs(diff: &FileDiff) -> String {
        diff.hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .map(|line| match line.kind {
                DiffLineKind::Add => '+',
                DiffLineKind::Remove => '-',
                DiffLineKind::Context => ' ',
            })
            .collect()
    }

    fn edit(id: &str, path: &str, old: &str, new: &str) -> Value {
        tool_use(
            id,
            id,
            id,
            "Edit",
            json!({ "file_path": file(path), "old_string": old, "new_string": new }),
            [0; 4],
        )
    }

    #[test]
    fn folds_the_edit_and_the_new_file_of_the_sample() {
        let changes = collect(&fixtures::transcript());
        assert_eq!(
            changes.files(),
            vec![
                ChangedFile {
                    path: "src/main.rs".into(),
                    status: FileStatus::Modified,
                    added: 3,
                    removed: 1
                },
                ChangedFile {
                    path: "src/new.rs".into(),
                    status: FileStatus::Added,
                    added: 1,
                    removed: 0
                },
            ]
        );
        let diff = changes.diff("src/main.rs").unwrap();
        assert_eq!(diff.hunks[0].header, "@@ -1,1 +1,3 @@");
        assert_eq!(signs(&diff), "-+++");
        assert_eq!(
            changes.diff("src/new.rs").unwrap().hunks[0].header,
            "@@ -0,0 +1,1 @@"
        );
        assert!(changes.diff("src/other.rs").is_none());
    }

    #[test]
    fn several_edits_and_a_multiedit_on_a_known_base() {
        let content = calls(vec![
            (
                edit("t1", "x.txt", "b", "B"),
                tool_result(
                    "r1",
                    "t1",
                    json!("ok"),
                    false,
                    json!({ "originalFile": "a\nb\nc\nd\n" }),
                ),
            ),
            (
                tool_use(
                    "c2",
                    "m2",
                    "t2",
                    "MultiEdit",
                    json!({ "file_path": file("x.txt"), "edits": [
                        { "old_string": "c", "new_string": "C" },
                        { "old_string": "d", "new_string": "d\ne" },
                    ] }),
                    [0; 4],
                ),
                tool_result(
                    "r2",
                    "t2",
                    json!("ok"),
                    false,
                    json!({ "originalFile": "a\nB\nc\nd\n" }),
                ),
            ),
        ]);
        let changes = collect(&content);
        let files = changes.files();
        assert_eq!((files[0].added, files[0].removed), (3, 2));
        assert_eq!(signs(&changes.diff("x.txt").unwrap()), " --++ +");
    }

    #[test]
    fn a_write_on_an_existing_file_with_and_without_base() {
        let content = calls(vec![
            (
                tool_use(
                    "c1",
                    "m1",
                    "t1",
                    "Write",
                    json!({ "file_path": file("known.txt"), "content": "one\ntwo\n" }),
                    [0; 4],
                ),
                tool_result(
                    "r1",
                    "t1",
                    json!("ok"),
                    false,
                    json!({ "type": "update", "originalFile": "one\n" }),
                ),
            ),
            (
                tool_use(
                    "c2",
                    "m2",
                    "t2",
                    "Write",
                    json!({ "file_path": file("unknown.txt"), "content": "new\n" }),
                    [0; 4],
                ),
                tool_result("r2", "t2", json!("ok"), false, json!({ "type": "update" })),
            ),
        ]);
        let changes = collect(&content);
        assert_eq!(signs(&changes.diff("known.txt").unwrap()), " +");
        let unknown = changes.diff("unknown.txt").unwrap();
        assert_eq!(unknown.hunks[0].header, "@@ -0,0 +1,1 @@ base unknown");
        assert_eq!(signs(&unknown), "+");
        assert_eq!(changes.files()[1].status, FileStatus::Modified);
    }

    #[test]
    fn an_edit_without_base_stays_a_fragment() {
        let content = calls(vec![(
            edit("t1", "a.rs", "x\ny", "x\nz"),
            tool_result("r1", "t1", json!("ok"), false, json!({})),
        )]);
        let changes = collect(&content);
        let diff = changes.diff("a.rs").unwrap();
        assert_eq!(diff.hunks[0].header, FRAGMENT_HEADER);
        assert_eq!(signs(&diff), " -+");
        assert_eq!(
            (changes.files()[0].added, changes.files()[0].removed),
            (1, 1)
        );
    }

    #[test]
    fn skips_errors_outside_paths_and_reverted_changes() {
        let content = calls(vec![
            (
                edit("t1", "f.rs", "a", "b"),
                tool_result("r1", "t1", json!("String not found"), true, json!("Error")),
            ),
            (
                tool_use(
                    "c2",
                    "m2",
                    "t2",
                    "Write",
                    json!({ "file_path": "C:\\elsewhere\\x.rs", "content": "x" }),
                    [0; 4],
                ),
                tool_result("r2", "t2", json!("ok"), false, json!({ "type": "create" })),
            ),
            (
                edit("t3", "z.rs", "a", "b"),
                tool_result(
                    "r3",
                    "t3",
                    json!("ok"),
                    false,
                    json!({ "originalFile": "a\n" }),
                ),
            ),
            (
                edit("t4", "z.rs", "b", "a"),
                tool_result(
                    "r4",
                    "t4",
                    json!("ok"),
                    false,
                    json!({ "originalFile": "b\n" }),
                ),
            ),
            (
                edit("t5", "../up.rs", "a", "b"),
                tool_result(
                    "r5",
                    "t5",
                    json!("ok"),
                    false,
                    json!({ "originalFile": "a\n" }),
                ),
            ),
        ]);
        assert_eq!(collect(&content).files(), Vec::new());
    }
}
