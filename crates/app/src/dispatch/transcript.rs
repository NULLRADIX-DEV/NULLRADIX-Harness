//! Turns the stream-json output of a Claude run into readable log lines.

use std::collections::HashMap;

use serde_json::Value;
use strum::IntoStaticStr;

use super::stream::Pipe;

/// Characters of the exact call or message kept in `detail`.
const DETAIL_MAX: usize = 8000;
const SAY_MAX: usize = 400;
const FAIL_MAX: usize = 300;

/// What a log line is about; the web UI colors by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
pub enum LineKind {
    Start,
    Say,
    Read,
    Edit,
    Search,
    Test,
    Run,
    Todo,
    Tool,
    Ok,
    Fail,
    Done,
    Abort,
    /// A line that is no event: stderr or text Claude printed itself.
    Raw,
}

impl LineKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// One line of the run log: a short text and, when there is more, the exact call or message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptLine {
    pub kind: LineKind,
    pub text: String,
    /// Id of the tool call, on a call and on its result.
    pub reference: Option<String>,
    pub detail: Option<String>,
}

impl TranscriptLine {
    pub(super) fn plain(kind: LineKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            reference: None,
            detail: None,
        }
    }

    fn with_detail(kind: LineKind, text: String, detail: String) -> Self {
        Self {
            kind,
            text,
            reference: None,
            detail: Some(detail),
        }
    }
}

fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(3)).collect();
    format!("{cut}...")
}

fn clip(text: &str) -> String {
    if text.chars().count() <= DETAIL_MAX {
        return text.to_owned();
    }
    let cut: String = text.chars().take(DETAIL_MAX).collect();
    format!("{cut}\n... cut")
}

fn string<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or("")
}

/// Text of a tool result, which is a string or a list of blocks.
fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|block| match block.get("type").and_then(Value::as_str) {
                Some("image") => String::from("[image]"),
                _ => string(block, "text").to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn is_test_command(command: &str) -> bool {
    ["test", "vitest", "jest", "pytest", "nextest"]
        .iter()
        .any(|word| command.split_whitespace().any(|part| part.contains(word)))
}

/// A rough classification of a shell command; the exact command stays in `detail`.
fn shell_kind(command: &str) -> LineKind {
    let first = command.split_whitespace().next().unwrap_or("");
    if is_test_command(command) {
        LineKind::Test
    } else if matches!(
        first,
        "cat" | "head" | "tail" | "ls" | "wc" | "pwd" | "less"
    ) {
        LineKind::Read
    } else if matches!(first, "grep" | "rg" | "find" | "fd") {
        LineKind::Search
    } else if matches!(first, "mv" | "cp" | "rm" | "mkdir" | "touch" | "tee") {
        LineKind::Edit
    } else {
        LineKind::Run
    }
}

/// Reads output line by line and keeps which tool call a result belongs to.
#[derive(Debug, Default)]
pub struct TranscriptFormatter {
    tools: HashMap<String, LineKind>,
    /// Working folder with forward slashes; paths below it show relative.
    root: Option<String>,
}

impl TranscriptFormatter {
    #[must_use]
    pub fn new(root: Option<&str>) -> Self {
        Self {
            tools: HashMap::new(),
            root: root.map(|root| root.replace('\\', "/").trim_end_matches('/').to_owned()),
        }
    }

    fn relative(&self, path: &str) -> String {
        let slashed = path.replace('\\', "/");
        match &self.root {
            Some(root) if !root.is_empty() => slashed
                .strip_prefix(root.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .map_or(slashed.clone(), str::to_owned),
            _ => slashed,
        }
    }

    /// The log lines of one output line; unknown events give none.
    pub fn format(&mut self, pipe: Pipe, line: &str) -> Vec<TranscriptLine> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let event = (pipe == Pipe::Stdout && trimmed.starts_with('{'))
            .then(|| serde_json::from_str::<Value>(trimmed).ok())
            .flatten()
            .filter(|event| event.get("type").is_some_and(Value::is_string));
        match event {
            Some(event) => self.event(&event),
            None => vec![TranscriptLine::plain(LineKind::Raw, clip(trimmed))],
        }
    }

    fn event(&mut self, event: &Value) -> Vec<TranscriptLine> {
        let content = event
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match string(event, "type") {
            "system" if string(event, "subtype") == "init" => {
                let model = event
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown model");
                vec![TranscriptLine::plain(
                    LineKind::Start,
                    format!("Claude starts ({model})"),
                )]
            }
            "assistant" => content
                .iter()
                .flat_map(|block| self.assistant_block(block))
                .collect(),
            "user" => content
                .iter()
                .filter(|block| string(block, "type") == "tool_result")
                .map(Self::tool_result)
                .collect(),
            "result" => vec![result_line(event)],
            _ => Vec::new(),
        }
    }

    fn assistant_block(&mut self, block: &Value) -> Vec<TranscriptLine> {
        match string(block, "type") {
            "text" => {
                let text = string(block, "text");
                if text.trim().is_empty() {
                    return Vec::new();
                }
                let short = one_line(text, SAY_MAX);
                let detail = (short.chars().count() < text.trim().chars().count())
                    .then(|| clip(text.trim()));
                vec![TranscriptLine {
                    kind: LineKind::Say,
                    text: short,
                    reference: None,
                    detail,
                }]
            }
            "tool_use" => {
                let name = block.get("name").and_then(Value::as_str).unwrap_or("?");
                let empty = Value::Object(serde_json::Map::new());
                let input = block.get("input").filter(|input| input.is_object());
                let mut line = self.tool(name, input.unwrap_or(&empty));
                if let Some(id) = block.get("id").and_then(Value::as_str) {
                    self.tools.insert(id.to_owned(), line.kind);
                    line.reference = Some(id.to_owned());
                }
                vec![line]
            }
            _ => Vec::new(),
        }
    }

    fn tool(&self, name: &str, input: &Value) -> TranscriptLine {
        let path = [string(input, "file_path"), string(input, "path")]
            .into_iter()
            .find(|path| !path.is_empty())
            .unwrap_or("");
        let file = self.relative(path);
        match name {
            "Read" => TranscriptLine::with_detail(
                LineKind::Read,
                format!("reads {file}"),
                format!("Read {path}"),
            ),
            "Edit" | "MultiEdit" | "NotebookEdit" => TranscriptLine::with_detail(
                LineKind::Edit,
                format!("edits {file}"),
                format!("{name} {path}"),
            ),
            "Write" => TranscriptLine::with_detail(
                LineKind::Edit,
                format!("writes {file}"),
                format!("Write {path}"),
            ),
            "Bash" | "PowerShell" => {
                let command = string(input, "command");
                let description = string(input, "description");
                let text = one_line(
                    if description.is_empty() {
                        command
                    } else {
                        description
                    },
                    120,
                );
                let prompt = if name == "Bash" { "$" } else { "PS>" };
                TranscriptLine::with_detail(
                    shell_kind(command),
                    text,
                    format!("{prompt} {}", clip(command)),
                )
            }
            "Grep" => {
                let pattern = string(input, "pattern");
                TranscriptLine::with_detail(
                    LineKind::Search,
                    format!("searches \"{}\"", one_line(pattern, 80)),
                    format!("Grep \"{pattern}\" {path}").trim_end().to_owned(),
                )
            }
            "Glob" => {
                let pattern = string(input, "pattern");
                TranscriptLine::with_detail(
                    LineKind::Search,
                    format!("finds files {}", one_line(pattern, 80)),
                    format!("Glob {pattern} {path}").trim_end().to_owned(),
                )
            }
            "TodoWrite" => todo_line(input),
            _ => TranscriptLine::with_detail(
                LineKind::Tool,
                if file.is_empty() {
                    name.to_owned()
                } else {
                    format!("{name} {file}")
                },
                format!("{name} {}", clip(&input.to_string())),
            ),
        }
    }

    fn tool_result(block: &Value) -> TranscriptLine {
        let reference = block
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let output = result_text(block.get("content"));
        if block.get("is_error") == Some(&Value::Bool(true)) {
            return TranscriptLine {
                kind: LineKind::Fail,
                text: one_line(&output, FAIL_MAX),
                reference,
                detail: Some(clip(output.trim())),
            };
        }
        TranscriptLine {
            kind: LineKind::Ok,
            text: String::new(),
            reference,
            detail: None,
        }
    }

    /// The kind of the call a result belongs to, when it was seen.
    #[must_use]
    pub fn kind_of(&self, reference: &str) -> Option<LineKind> {
        self.tools.get(reference).copied()
    }
}

fn status(todo: &Value) -> &str {
    string(todo, "status")
}

fn todo_line(input: &Value) -> TranscriptLine {
    let todos = input
        .get("todos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let done = todos
        .iter()
        .filter(|todo| status(todo) == "completed")
        .count();
    let now = todos
        .iter()
        .find(|todo| status(todo) == "in_progress")
        .map(|todo| string(todo, "content"))
        .filter(|content| !content.is_empty())
        .map_or(String::new(), |content| {
            format!(" - now: {}", one_line(content, 120))
        });
    let detail = todos
        .iter()
        .map(|todo| {
            let mark = match status(todo) {
                "completed" => "[x]",
                "in_progress" => "[>]",
                _ => "[ ]",
            };
            format!("{mark} {}", string(todo, "content"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    TranscriptLine::with_detail(
        LineKind::Todo,
        format!("todo list: {done}/{} done{now}", todos.len()),
        detail,
    )
}

fn result_line(event: &Value) -> TranscriptLine {
    let turns = event
        .get("num_turns")
        .and_then(Value::as_i64)
        .map_or(String::new(), |turns| format!(", {turns} turns"));
    let minutes = event
        .get("duration_ms")
        .and_then(Value::as_i64)
        .map_or(String::new(), |millis| {
            format!(", {} min", (millis + 30_000) / 60_000)
        });
    let subtype = string(event, "subtype");
    if subtype == "success" {
        TranscriptLine::plain(LineKind::Done, format!("done{turns}{minutes}"))
    } else {
        let subtype = if subtype.is_empty() { "?" } else { subtype };
        TranscriptLine::plain(
            LineKind::Abort,
            format!("ended ({subtype}){turns}{minutes}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn lines(formatter: &mut TranscriptFormatter, events: &[Value]) -> Vec<TranscriptLine> {
        events
            .iter()
            .flat_map(|event| formatter.format(Pipe::Stdout, &event.to_string()))
            .collect()
    }

    #[test]
    fn formats_a_session_from_start_to_result() {
        let mut formatter = TranscriptFormatter::new(Some("C:\\work\\repo"));
        let out = lines(
            &mut formatter,
            &[
                json!({ "type": "system", "subtype": "init", "model": "opus" }),
                json!({ "type": "assistant", "message": { "content": [
                    { "type": "text", "text": "Reading the ticket." },
                    { "type": "tool_use", "id": "t1", "name": "Read", "input": { "file_path": "C:/work/repo/src/lib.rs" } },
                    { "type": "tool_use", "id": "t2", "name": "Bash", "input": { "command": "cargo nextest run", "description": "Run the tests" } },
                ] } }),
                json!({ "type": "user", "message": { "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": "fn main() {}" },
                    { "type": "tool_result", "tool_use_id": "t2", "is_error": true, "content": [{ "type": "text", "text": "1 test failed" }] },
                ] } }),
                json!({ "type": "result", "subtype": "success", "num_turns": 3, "duration_ms": 90_000 }),
            ],
        );
        let kinds: Vec<&str> = out.iter().map(|line| line.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["start", "say", "read", "test", "ok", "fail", "done"]
        );
        assert_eq!(out[0].text, "Claude starts (opus)");
        assert_eq!(out[2].text, "reads src/lib.rs");
        assert_eq!(out[2].reference.as_deref(), Some("t1"));
        assert_eq!(out[3].text, "Run the tests");
        assert_eq!(out[3].detail.as_deref(), Some("$ cargo nextest run"));
        assert_eq!(out[5].text, "1 test failed");
        assert_eq!(out[5].reference.as_deref(), Some("t2"));
        assert_eq!(out[6].text, "done, 3 turns, 2 min");
        assert_eq!(formatter.kind_of("t2"), Some(LineKind::Test));
    }

    #[test]
    fn long_text_keeps_its_full_form_in_the_detail() {
        let mut formatter = TranscriptFormatter::new(None);
        let long = "word ".repeat(200);
        let out = lines(
            &mut formatter,
            &[
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": long }] } }),
            ],
        );
        assert!(out[0].text.ends_with("..."));
        assert_eq!(out[0].detail.as_deref(), Some(long.trim()));
    }

    #[test]
    fn non_events_pass_through_and_unknown_events_vanish() {
        let mut formatter = TranscriptFormatter::new(None);
        assert_eq!(
            formatter.format(Pipe::Stderr, "boom"),
            vec![TranscriptLine::plain(LineKind::Raw, "boom")]
        );
        assert_eq!(
            formatter.format(Pipe::Stdout, "plain text"),
            vec![TranscriptLine::plain(LineKind::Raw, "plain text")]
        );
        assert_eq!(
            formatter.format(Pipe::Stdout, "{\"type\":\"rate_limit_event\"}"),
            Vec::new()
        );
        assert_eq!(formatter.format(Pipe::Stdout, "  "), Vec::new());
    }

    #[test]
    fn a_huge_raw_line_is_clipped() {
        let mut formatter = TranscriptFormatter::new(None);
        let out = formatter.format(Pipe::Stderr, &"x".repeat(100_000));
        assert_eq!(out.len(), 1);
        assert!(out[0].text.chars().count() < 2 * DETAIL_MAX);
        assert!(out[0].text.ends_with("... cut"));
    }

    #[test]
    fn shell_commands_are_sorted_by_what_they_do() {
        assert_eq!(shell_kind("cat README.md"), LineKind::Read);
        assert_eq!(shell_kind("rg foo src"), LineKind::Search);
        assert_eq!(shell_kind("npm test -- --watch=false"), LineKind::Test);
        assert_eq!(shell_kind("rm -rf dist"), LineKind::Edit);
        assert_eq!(shell_kind("git status"), LineKind::Run);
    }

    #[test]
    fn todo_lists_show_progress_and_the_current_item() {
        let mut formatter = TranscriptFormatter::new(None);
        let out = lines(
            &mut formatter,
            &[json!({ "type": "assistant", "message": { "content": [{
                "type": "tool_use", "id": "t", "name": "TodoWrite",
                "input": { "todos": [
                    { "content": "a", "status": "completed" },
                    { "content": "b", "status": "in_progress" },
                    { "content": "c", "status": "pending" },
                ] }
            }] } })],
        );
        assert_eq!(out[0].text, "todo list: 1/3 done - now: b");
        assert_eq!(out[0].detail.as_deref(), Some("[x] a\n[>] b\n[ ] c"));
    }

    #[test]
    fn a_failed_result_is_an_abort_line() {
        let mut formatter = TranscriptFormatter::new(None);
        let out = formatter.format(
            Pipe::Stdout,
            &json!({ "type": "result", "subtype": "error_max_turns" }).to_string(),
        );
        assert_eq!(out[0].kind, LineKind::Abort);
        assert_eq!(out[0].text, "ended (error_max_turns)");
    }
}
