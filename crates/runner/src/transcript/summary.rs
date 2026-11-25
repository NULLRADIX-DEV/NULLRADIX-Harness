//! One-line summary of a tool call, such as `Edit src/x.ts (+12 -3)` or `Bash: npm test`.

use serde_json::Value;

use super::jsonl::text;
use super::line_diff;

/// Longest command or description in a summary.
const MAX_PART: usize = 120;
const MINUS: char = '\u{2212}';
const ELLIPSIS: char = '\u{2026}';
const ARROW: char = '\u{2192}';

/// Summary of tool `name` with `input`; paths relative to `cwd` when inside it.
pub(crate) fn summarize(name: &str, input: &Value, cwd: Option<&str>) -> String {
    let field = |key: &str| text(input, key).map(one_line);
    let path = |key: &str| text(input, key).map(|path| relative(path, cwd));
    match name {
        "Edit" => match path("file_path") {
            Some(file) => {
                let (added, removed) = edit_counts(input);
                format!("Edit {file} (+{added} {MINUS}{removed})")
            }
            None => name.to_owned(),
        },
        "MultiEdit" => match path("file_path") {
            Some(file) => {
                let (added, removed) =
                    input
                        .get("edits")
                        .and_then(Value::as_array)
                        .map_or((0, 0), |edits| {
                            edits
                                .iter()
                                .map(edit_counts)
                                .fold((0, 0), |sum, next| (sum.0 + next.0, sum.1 + next.1))
                        });
                format!("MultiEdit {file} (+{added} {MINUS}{removed})")
            }
            None => name.to_owned(),
        },
        "Write" => match path("file_path") {
            Some(file) => {
                let lines = input
                    .get("content")
                    .and_then(Value::as_str)
                    .map_or(0, |content| content.lines().count());
                format!("Write {file} (+{lines})")
            }
            None => name.to_owned(),
        },
        "Read" | "NotebookEdit" | "NotebookRead" => {
            match path("file_path").or_else(|| path("notebook_path")) {
                Some(file) => format!("{name} {file}"),
                None => name.to_owned(),
            }
        }
        "Bash" | "PowerShell" => with(name, ": ", field("command")),
        "Grep" => with(name, " ", field("pattern").map(|pattern| quoted(&pattern))),
        "Glob" => with(name, " ", field("pattern")),
        "Task" | "Agent" => {
            let description = field("description").or_else(|| field("prompt"));
            match (field("subagent_type"), description) {
                (Some(kind), Some(description)) => format!("{name} {kind}: {description}"),
                (Some(kind), None) => format!("{name} {kind}"),
                (None, description) => with(name, ": ", description),
            }
        }
        "WebFetch" => with(name, " ", field("url")),
        "WebSearch" | "ToolSearch" => with(name, " ", field("query").map(|query| quoted(&query))),
        "TodoWrite" => {
            let count = input
                .get("todos")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("TodoWrite ({count} tasks)")
        }
        "TaskCreate" => with(name, ": ", field("subject")),
        "TaskUpdate" => match (task_id(input), field("status")) {
            (Some(id), Some(status)) => format!("TaskUpdate #{id} {ARROW} {status}"),
            (Some(id), None) => format!("TaskUpdate #{id}"),
            _ => name.to_owned(),
        },
        "Skill" => with(name, " ", field("skill").or_else(|| field("command"))),
        _ => mcp_name(name).unwrap_or_else(|| name.to_owned()),
    }
}

fn with(name: &str, separator: &str, part: Option<String>) -> String {
    match part {
        Some(part) => format!("{name}{separator}{part}"),
        None => name.to_owned(),
    }
}

/// The first line, cut to [`MAX_PART`] characters.
fn one_line(value: &str) -> String {
    let mut lines = value.trim().lines();
    let first = lines.next().unwrap_or("").trim();
    let more = lines.next().is_some();
    let mut cut: String = first.chars().take(MAX_PART).collect();
    if more || first.chars().count() > MAX_PART {
        cut.push(ELLIPSIS);
    }
    cut
}

fn quoted(value: &str) -> String {
    format!("\"{value}\"")
}

/// A task number given as text or number.
pub(crate) fn task_id(input: &Value) -> Option<String> {
    id_text(input.get("taskId")?)
}

pub(crate) fn id_text(value: &Value) -> Option<String> {
    match value {
        Value::String(id) if !id.is_empty() => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

/// Added and removed lines of a replacement `old_string` to `new_string`.
fn edit_counts(edit: &Value) -> (i64, i64) {
    let old = edit.get("old_string").and_then(Value::as_str).unwrap_or("");
    let new = edit.get("new_string").and_then(Value::as_str).unwrap_or("");
    line_diff::text_counts(old, new)
}

/// `mcp__server__tool` reads `tool (server)`.
fn mcp_name(name: &str) -> Option<String> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.rsplit_once("__")?;
    Some(format!("{tool} ({server})"))
}

/// The path relative to `cwd` with `/` when inside it, else unchanged with `/`.
pub(crate) fn relative(path: &str, cwd: Option<&str>) -> String {
    let path = path.replace('\\', "/");
    let Some(cwd) = cwd else {
        return path;
    };
    let base = cwd.replace('\\', "/");
    let base = base.trim_end_matches('/');
    if base.is_empty() || path.len() <= base.len() + 1 || !path.is_char_boundary(base.len()) {
        return path;
    }
    let (head, tail) = path.split_at(base.len());
    let same = if cfg!(windows) {
        head.eq_ignore_ascii_case(base)
    } else {
        head == base
    };
    match tail.strip_prefix('/') {
        Some(rest) if same => rest.to_owned(),
        _ => path,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const CWD: Option<&str> = Some("D:/Projects/demo");

    #[test]
    fn edit_shows_the_relative_path_and_line_counts() {
        let input = json!({
            "file_path": "D:\\Projects\\demo\\src\\x.ts",
            "old_string": "a\nb\nc",
            "new_string": "a\nB\nc\nd",
        });
        assert_eq!(
            summarize("Edit", &input, CWD),
            "Edit src/x.ts (+2 \u{2212}1)"
        );
    }

    #[test]
    fn multiedit_adds_up_its_edits() {
        let input = json!({
            "file_path": "D:/Projects/demo/a.rs",
            "edits": [
                { "old_string": "x", "new_string": "y" },
                { "old_string": "", "new_string": "new\nnew" },
            ],
        });
        assert_eq!(
            summarize("MultiEdit", &input, CWD),
            "MultiEdit a.rs (+3 \u{2212}1)"
        );
    }

    #[test]
    fn write_read_and_paths_outside_the_folder() {
        assert_eq!(
            summarize(
                "Write",
                &json!({ "file_path": "D:/Projects/demo/n.md", "content": "a\nb\n" }),
                CWD
            ),
            "Write n.md (+2)"
        );
        assert_eq!(
            summarize("Read", &json!({ "file_path": "C:/elsewhere/a.rs" }), CWD),
            "Read C:/elsewhere/a.rs"
        );
    }

    #[test]
    fn commands_show_their_first_line_cut() {
        assert_eq!(
            summarize("Bash", &json!({ "command": "npm test" }), CWD),
            "Bash: npm test"
        );
        assert_eq!(
            summarize(
                "PowerShell",
                &json!({ "command": "Get-Item a\nGet-Item b" }),
                CWD
            ),
            "PowerShell: Get-Item a\u{2026}"
        );
        let long = "x".repeat(200);
        let summary = summarize("Bash", &json!({ "command": long }), CWD);
        assert_eq!(summary.chars().count(), "Bash: ".len() + MAX_PART + 1);
    }

    #[test]
    fn search_agents_and_tasks() {
        assert_eq!(
            summarize("Grep", &json!({ "pattern": "fn main" }), CWD),
            "Grep \"fn main\""
        );
        assert_eq!(
            summarize("Glob", &json!({ "pattern": "**/*.rs" }), CWD),
            "Glob **/*.rs"
        );
        assert_eq!(
            summarize(
                "Task",
                &json!({ "subagent_type": "Explore", "description": "Find router", "prompt": "x" }),
                CWD
            ),
            "Task Explore: Find router"
        );
        assert_eq!(
            summarize("Agent", &json!({ "description": "Check" }), CWD),
            "Agent: Check"
        );
        assert_eq!(
            summarize("TodoWrite", &json!({ "todos": [{}, {}, {}] }), CWD),
            "TodoWrite (3 tasks)"
        );
        assert_eq!(
            summarize("TaskCreate", &json!({ "subject": "Write tests" }), CWD),
            "TaskCreate: Write tests"
        );
        assert_eq!(
            summarize(
                "TaskUpdate",
                &json!({ "taskId": "2", "status": "completed" }),
                CWD
            ),
            "TaskUpdate #2 \u{2192} completed"
        );
    }

    #[test]
    fn web_skills_and_mcp_tools_with_short_names() {
        assert_eq!(
            summarize("WebSearch", &json!({ "query": "rust axum" }), CWD),
            "WebSearch \"rust axum\""
        );
        assert_eq!(
            summarize("WebFetch", &json!({ "url": "https://example.org" }), CWD),
            "WebFetch https://example.org"
        );
        assert_eq!(
            summarize("Skill", &json!({ "skill": "board" }), CWD),
            "Skill board"
        );
        assert_eq!(
            summarize("mcp__claude-in-chrome__navigate", &json!({}), CWD),
            "navigate (claude-in-chrome)"
        );
        assert_eq!(summarize("Unknown", &json!({}), CWD), "Unknown");
    }
}
