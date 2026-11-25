//! Reads a page of the transcript from a byte offset into entries, as legacy.

use std::collections::HashMap;
use std::io::{self, BufRead};
use std::path::Path;
use std::sync::LazyLock;

use nullradix_app::session_detail::{
    MAX_RESULT_CHARS, SystemLevel, ToolResult, TranscriptChunk, TranscriptEntry,
};
use regex::Regex;
use serde_json::Value;

use super::jsonl::{self, Line, LineReader, flag, number, text};
use super::summary::summarize;

const IMAGE: &str = "[image]";
/// How far behind a full page the file is searched for results of the calls it leaves open.
const LOOKAHEAD_BYTES: u64 = 8 * 1024 * 1024;

/// Reads the file at `path` from offset `base`, about `limit` entries at most.
pub(crate) fn parse_file(path: &Path, base: u64, limit: usize) -> io::Result<TranscriptChunk> {
    parse_lines(&mut jsonl::open_lines(path, base)?, limit)
}

/// Reads `bytes` that start at offset `base` of the file, about `limit` entries at most.
#[cfg(test)]
pub(crate) fn parse(bytes: &[u8], base: u64, limit: usize) -> TranscriptChunk {
    let mut lines = LineReader::new(io::Cursor::new(bytes), base);
    parse_lines(&mut lines, limit).unwrap()
}

/// Reads lines until `limit` entries are found, then on only while calls still lack results.
pub(crate) fn parse_lines<R: BufRead>(
    lines: &mut LineReader<R>,
    limit: usize,
) -> io::Result<TranscriptChunk> {
    let mut page = Page::default();
    let mut cut = false;
    let mut cursor = lines.complete();
    let base = cursor;
    let mut search_until = u64::MAX;
    while let Some(line) = lines.next_line()? {
        if !cut && page.entries.len() >= limit.max(1) {
            cut = true;
            search_until = line.end.saturating_add(LOOKAHEAD_BYTES);
        }
        if !cut {
            page.feed(&line);
            cursor = line.end;
            continue;
        }
        // results of calls still open, in the rest of the file
        if page.pending.is_empty() || line.end > search_until {
            break;
        }
        if line.text.contains("tool_use_id")
            && let Ok(value) = serde_json::from_str::<Value>(line.text)
        {
            page.results(&value);
        }
    }
    if !cut {
        cursor = lines.complete();
    }
    if let Some(start) = page.pending.values().map(|(_, start)| *start).min()
        && (!cut || start > base)
    {
        cursor = cursor.min(start);
    }
    Ok(TranscriptChunk {
        entries: page.entries,
        cursor,
        skipped: page.skipped,
    })
}

#[derive(Default)]
struct Page {
    entries: Vec<TranscriptEntry>,
    skipped: u64,
    /// Open tool calls: id to entry index and line start.
    pending: HashMap<String, (usize, u64)>,
}

impl Page {
    fn feed(&mut self, line: &Line<'_>) {
        let value = match serde_json::from_str::<Value>(line.text) {
            Ok(value) if value.is_object() => value,
            _ => {
                self.skipped += 1;
                return;
            }
        };
        if flag(&value, "isSidechain") {
            return;
        }
        let at = text(&value, "timestamp").unwrap_or("").to_owned();
        let id = text(&value, "uuid").map_or_else(|| format!("line-{}", line.start), str::to_owned);
        match text(&value, "type") {
            Some("user") => self.user(&value, id, at),
            Some("assistant") => self.assistant(&value, line.start, &id, &at),
            Some("system") => self.system(&value, id, at),
            _ => {}
        }
    }

    /// Prompts, tool results and output of local commands.
    fn user(&mut self, value: &Value, id: String, at: String) {
        if flag(value, "isMeta") {
            return;
        }
        let mut texts: Vec<String> = Vec::new();
        match value.pointer("/message/content") {
            Some(Value::String(content)) => texts.push(content.clone()),
            Some(Value::Array(parts)) => {
                for part in parts {
                    match text(part, "type") {
                        Some("tool_result") => self.result(part),
                        Some("text") => texts.push(text(part, "text").unwrap_or("").to_owned()),
                        Some("image") => texts.push(IMAGE.to_owned()),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        let joined = texts.join("\n");
        if joined.trim().is_empty() {
            return;
        }
        let origin = value.pointer("/origin/kind").and_then(Value::as_str);
        if let Some(entry) = prompt(id, at, &joined, origin) {
            self.entries.push(entry);
        }
    }

    /// Answer text, thinking and tool calls.
    fn assistant(&mut self, value: &Value, start: u64, id: &str, at: &str) {
        let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        let cwd = text(value, "cwd");
        for (index, block) in blocks.iter().enumerate() {
            let block_id = format!("{id}:{index}");
            match text(block, "type") {
                Some("text") => {
                    if let Some(content) = text(block, "text") {
                        self.entries.push(TranscriptEntry::Text {
                            id: block_id,
                            at: at.to_owned(),
                            text: content.to_owned(),
                        });
                    }
                }
                Some("thinking" | "redacted_thinking") => {
                    self.entries.push(TranscriptEntry::Thinking {
                        id: block_id,
                        at: at.to_owned(),
                        text: block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        duration_ms: number(value, "thinkingDurationMs"),
                    });
                }
                Some("tool_use" | "server_tool_use") => {
                    let name = text(block, "name").unwrap_or("Tool").to_owned();
                    let input = block.get("input").unwrap_or(&Value::Null);
                    let tool_id = text(block, "id").map_or(block_id, str::to_owned);
                    self.pending
                        .insert(tool_id.clone(), (self.entries.len(), start));
                    self.entries.push(TranscriptEntry::Tool {
                        id: tool_id,
                        at: at.to_owned(),
                        summary: summarize(&name, input, cwd),
                        name,
                        input: input.to_string(),
                        result: None,
                    });
                }
                _ => {}
            }
        }
    }

    /// System notes; turn durations are left out.
    fn system(&mut self, value: &Value, id: String, at: String) {
        let content = text(value, "content");
        let (message, level) = match text(value, "subtype") {
            Some("turn_duration") => return,
            Some("stop_hook_summary") => {
                let errors: Vec<String> = value
                    .get("hookErrors")
                    .and_then(Value::as_array)
                    .map_or_default(|list| {
                        list.iter()
                            .map(|error| match error {
                                Value::String(text) => text.clone(),
                                other => other.to_string(),
                            })
                            .collect()
                    });
                if errors.is_empty() {
                    return;
                }
                (errors.join("\n"), SystemLevel::Warning)
            }
            Some("api_error") => (
                value
                    .pointer("/error/formatted")
                    .and_then(Value::as_str)
                    .or(content)
                    .unwrap_or("API error")
                    .to_owned(),
                SystemLevel::Error,
            ),
            Some("compact_boundary") => (
                content.unwrap_or("Conversation compacted").to_owned(),
                SystemLevel::Info,
            ),
            _ => {
                let Some(content) = content else {
                    return;
                };
                (clean(content), level(text(value, "level")))
            }
        };
        if message.trim().is_empty() {
            return;
        }
        self.entries.push(TranscriptEntry::System {
            id,
            at,
            text: message,
            level,
        });
    }

    /// Tool results of a user line, for the search in the rest of the file.
    fn results(&mut self, value: &Value) {
        if flag(value, "isSidechain") {
            return;
        }
        if let Some(parts) = value.pointer("/message/content").and_then(Value::as_array) {
            for part in parts {
                if text(part, "type") == Some("tool_result") {
                    self.result(part);
                }
            }
        }
    }

    fn result(&mut self, part: &Value) {
        let Some(tool_id) = text(part, "tool_use_id") else {
            return;
        };
        let Some((index, _)) = self.pending.remove(tool_id) else {
            return;
        };
        if let Some(TranscriptEntry::Tool { result, .. }) = self.entries.get_mut(index) {
            *result = Some(tool_result(part));
        }
    }
}

/// The result of a `tool_result` part, cut to [`MAX_RESULT_CHARS`].
pub(crate) fn tool_result(part: &Value) -> ToolResult {
    let full = match part.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match text(item, "type") {
                Some("text") => item.get("text").and_then(Value::as_str).map(str::to_owned),
                Some("image") => Some(IMAGE.to_owned()),
                Some("tool_reference") => text(item, "tool_name")
                    .or_else(|| text(item, "name"))
                    .map(str::to_owned),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    let truncated = full.chars().count() > MAX_RESULT_CHARS;
    ToolResult {
        text: if truncated {
            full.chars().take(MAX_RESULT_CHARS).collect()
        } else {
            full
        },
        is_error: flag(part, "is_error"),
        truncated,
    }
}

/// A prompt or system entry from the text of a user line; nothing for empty text.
fn prompt(id: String, at: String, raw: &str, origin: Option<&str>) -> Option<TranscriptEntry> {
    let content = strip(&REMINDERS, raw);
    let content = content.trim();
    if content.is_empty() || content.contains("<local-command-caveat>") {
        return None;
    }
    if let Some(output) =
        inner(content, "local-command-stdout").or_else(|| inner(content, "local-command-stderr"))
    {
        let output = clean(&output);
        return (!output.is_empty()).then_some(TranscriptEntry::System {
            id,
            at,
            text: output,
            level: SystemLevel::Info,
        });
    }
    if let Some(name) = inner(content, "command-name") {
        let name = name.trim();
        let slash = if name.starts_with('/') { "" } else { "/" };
        let args = inner(content, "command-args").unwrap_or_default();
        let text = format!("{slash}{name} {}", args.trim()).trim().to_owned();
        return Some(TranscriptEntry::Prompt { id, at, text });
    }
    if origin.is_some_and(|kind| kind != "human") {
        let text = inner(content, "summary").unwrap_or_else(|| clean(content));
        return (!text.trim().is_empty()).then(|| TranscriptEntry::System {
            id,
            at,
            text: text.trim().to_owned(),
            level: SystemLevel::Info,
        });
    }
    Some(TranscriptEntry::Prompt {
        id,
        at,
        text: content.to_owned(),
    })
}

fn level(name: Option<&str>) -> SystemLevel {
    match name {
        Some("error") => SystemLevel::Error,
        Some("warning" | "warn") => SystemLevel::Warning,
        _ => SystemLevel::Info,
    }
}

/// Content of the first `<tag>...</tag>`.
fn inner(content: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = content.find(&open)? + open.len();
    let end = content[start..]
        .find(&close)
        .map_or(content.len(), |end| start + end);
    Some(content[start..end].to_owned())
}

/// Without tags and terminal colors, trimmed.
fn clean(content: &str) -> String {
    strip(&TAGS, &strip(&ANSI, content)).trim().to_owned()
}

/// `text` without the matches of `pattern`.
fn strip(pattern: &LazyLock<Option<Regex>>, text: &str) -> String {
    match pattern.as_ref() {
        Some(pattern) => pattern.replace_all(text, "").into_owned(),
        None => text.to_owned(),
    }
}

// constant patterns, so `None` never happens
static REMINDERS: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?s)<system-reminder>.*?</system-reminder>").ok());
static TAGS: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"</?[A-Za-z][A-Za-z0-9_-]*>").ok());
static ANSI: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").ok());

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::fixtures;
    use super::*;

    fn kinds(entries: &[TranscriptEntry]) -> Vec<&'static str> {
        entries
            .iter()
            .map(|entry| match entry {
                TranscriptEntry::Prompt { .. } => "prompt",
                TranscriptEntry::Text { .. } => "text",
                TranscriptEntry::Thinking { .. } => "thinking",
                TranscriptEntry::Tool { .. } => "tool",
                TranscriptEntry::System { .. } => "system",
            })
            .collect()
    }

    fn find<'a>(entries: &'a [TranscriptEntry], id: &str) -> &'a TranscriptEntry {
        entries
            .iter()
            .find(|entry| entry.id() == id)
            .unwrap_or_else(|| panic!("entry {id} missing"))
    }

    fn len(text: &str) -> u64 {
        u64::try_from(text.len()).unwrap()
    }

    fn open_call(uuid: &str) -> String {
        fixtures::line(&json!({
            "type": "assistant", "uuid": uuid, "timestamp": "t", "cwd": fixtures::CWD,
            "message": { "id": "m-open", "content": [
                { "type": "tool_use", "id": "toolu_open", "name": "Bash", "input": { "command": "sleep 5" } }
            ] }
        }))
    }

    fn open_result() -> String {
        fixtures::line(&json!({
            "type": "user", "uuid": "u-open", "timestamp": "t",
            "message": { "content": [
                { "type": "tool_result", "tool_use_id": "toolu_open", "content": "done" }
            ] }
        }))
    }

    #[test]
    fn reads_every_line_type_in_order() {
        let content = fixtures::transcript();
        let page = parse(content.as_bytes(), 0, 500);
        assert_eq!(page.skipped, 1, "the broken line counts");
        assert_eq!(page.cursor, len(&content));
        assert_eq!(
            kinds(&page.entries),
            vec![
                "prompt", "prompt", "system", "thinking", "text", "tool", "tool", "tool", "tool",
                "tool", "system", "system", "system", "tool", "tool", "tool", "system", "text"
            ]
        );
    }

    #[test]
    fn cleans_prompts_and_commands() {
        let page = parse(fixtures::transcript().as_bytes(), 0, 500);
        match find(&page.entries, "u-prompt") {
            TranscriptEntry::Prompt { text, at, .. } => {
                assert_eq!(text, "Please add a function");
                assert_eq!(at, "2026-11-04T08:00:00.000Z");
            }
            other => panic!("no prompt: {other:?}"),
        }
        match find(&page.entries, "u-command") {
            TranscriptEntry::Prompt { text, .. } => assert_eq!(text, "/model opus"),
            other => panic!("no prompt: {other:?}"),
        }
        match find(&page.entries, "u-stdout") {
            TranscriptEntry::System { text, level, .. } => {
                assert_eq!(text, "Model set");
                assert_eq!(*level, SystemLevel::Info);
            }
            other => panic!("no system entry: {other:?}"),
        }
        match find(&page.entries, "u-notification") {
            TranscriptEntry::System { text, .. } => assert_eq!(text, "Background task done"),
            other => panic!("no system entry: {other:?}"),
        }
    }

    #[test]
    fn attaches_results_to_their_tools_and_cuts_long_ones() {
        let page = parse(fixtures::transcript().as_bytes(), 0, 500);
        match find(&page.entries, "toolu_read") {
            TranscriptEntry::Tool {
                summary,
                result,
                input,
                ..
            } => {
                assert_eq!(summary, "Read src/main.rs");
                assert!(input.contains("file_path"));
                let result = result.as_ref().unwrap();
                assert_eq!(result.text, "1\tfn main() {}");
                assert!(!result.is_error);
            }
            other => panic!("no tool: {other:?}"),
        }
        match find(&page.entries, "toolu_bash") {
            TranscriptEntry::Tool { result, .. } => {
                let result = result.as_ref().unwrap();
                assert!(result.is_error);
                assert_eq!(result.text, "Exit code 1\n[image]");
            }
            other => panic!("no tool: {other:?}"),
        }
        match find(&page.entries, "toolu_grep") {
            TranscriptEntry::Tool { result, .. } => {
                let result = result.as_ref().unwrap();
                assert!(result.truncated);
                assert_eq!(result.text.chars().count(), MAX_RESULT_CHARS);
            }
            other => panic!("no tool: {other:?}"),
        }
    }

    #[test]
    fn thinking_system_notes_and_subagents() {
        let page = parse(fixtures::transcript().as_bytes(), 0, 500);
        match find(&page.entries, "a-think:0") {
            TranscriptEntry::Thinking {
                text, duration_ms, ..
            } => {
                assert_eq!(text, "");
                assert_eq!(*duration_ms, Some(1500));
            }
            other => panic!("no thinking: {other:?}"),
        }
        let levels: Vec<(String, SystemLevel)> = page
            .entries
            .iter()
            .filter_map(|entry| match entry {
                TranscriptEntry::System { text, level, .. } => Some((text.clone(), *level)),
                _ => None,
            })
            .collect();
        assert!(levels.contains(&("Hook failed".into(), SystemLevel::Warning)));
        assert!(levels.contains(&("Overloaded".into(), SystemLevel::Error)));
        assert!(levels.contains(&("Conversation compacted".into(), SystemLevel::Info)));
        assert!(
            page.entries.iter().all(|entry| entry.id() != "a-side:0"),
            "subagent lines are hidden"
        );
    }

    #[test]
    fn a_half_last_line_is_not_used() {
        let content = fixtures::transcript();
        let bytes = format!("{content}{{\"type\":\"assistant\",\"uuid\":\"a-new\"");
        let page = parse(bytes.as_bytes(), 0, 500);
        assert_eq!(page.cursor, len(&content));
        assert_eq!(page.skipped, 1);
    }

    #[test]
    fn reads_on_in_pages_from_the_offset() {
        let content = fixtures::transcript();
        let all = parse(content.as_bytes(), 0, 500).entries;
        let first = parse(content.as_bytes(), 0, 4);
        assert_eq!(first.entries.len(), 4);
        let offset = usize::try_from(first.cursor).unwrap();
        let second = parse(&content.as_bytes()[offset..], first.cursor, 500);
        let mut joined = first.entries.clone();
        joined.extend(second.entries);
        assert_eq!(joined, all);
    }

    #[test]
    fn keeps_the_cursor_before_a_tool_without_result() {
        let start = fixtures::transcript();
        let content = format!("{start}{}", open_call("a-open"));
        let page = parse(content.as_bytes(), 0, 500);
        assert_eq!(page.cursor, len(&start));
        let content = format!("{content}{}", open_result());
        let at = usize::try_from(page.cursor).unwrap();
        let next = parse(&content.as_bytes()[at..], page.cursor, 500);
        match find(&next.entries, "toolu_open") {
            TranscriptEntry::Tool { result, .. } => {
                assert_eq!(result.as_ref().unwrap().text, "done");
            }
            other => panic!("no tool: {other:?}"),
        }
        assert_eq!(next.cursor, len(&content));
    }

    #[test]
    fn a_cut_page_repeats_an_open_call_later_with_its_result() {
        let start = fixtures::transcript();
        let more = fixtures::line(&fixtures::assistant(
            "a-more",
            "m-more",
            json!({ "type": "text", "text": "Waiting." }),
            [0; 4],
        ));
        let content = format!("{start}{}{more}", open_call("a-open"));
        let first = parse(content.as_bytes(), 0, 19);
        assert_eq!(first.entries.last().unwrap().id(), "toolu_open");
        assert_eq!(
            first.cursor,
            len(&start),
            "the cursor stays before the open call"
        );
        let content = format!("{content}{}", open_result());
        let at = usize::try_from(first.cursor).unwrap();
        let next = parse(&content.as_bytes()[at..], first.cursor, 500);
        let ids: Vec<&str> = next.entries.iter().map(TranscriptEntry::id).collect();
        assert_eq!(ids, vec!["toolu_open", "a-more:0"]);
        assert_eq!(next.cursor, len(&content));
    }

    #[test]
    fn an_open_call_starting_a_cut_page_does_not_hold_it() {
        let open = open_call("a-open");
        let more = fixtures::line(&fixtures::assistant(
            "a-more",
            "m-more",
            json!({ "type": "text", "text": "Waiting." }),
            [0; 4],
        ));
        let content = format!("{open}{more}");
        let page = parse(content.as_bytes(), 0, 1);
        assert_eq!(page.cursor, len(&open));
    }

    #[test]
    fn finds_results_behind_the_limit() {
        let content = fixtures::transcript();
        let page = parse(content.as_bytes(), 0, 6);
        match page.entries.last().unwrap() {
            TranscriptEntry::Tool { id, result, .. } => {
                assert_eq!(id, "toolu_read");
                assert!(result.is_some());
            }
            other => panic!("no tool: {other:?}"),
        }
    }

    #[test]
    fn a_page_of_a_large_file_reads_only_its_part_and_a_short_look_ahead() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.jsonl");
        let content = fixtures::repeated(5000);
        std::fs::write(&path, &content).unwrap();
        let mut lines = jsonl::open_lines(&path, 0).unwrap();
        let page = parse_lines(&mut lines, 50).unwrap();
        assert_eq!(page.entries.len(), 50);
        let file = len(&content);
        assert!(
            lines.complete() * 100 < file,
            "{} of {file}",
            lines.complete()
        );
        assert!(page.cursor <= lines.complete());
        let later = parse_file(&path, page.cursor, 50).unwrap();
        assert_eq!(later.entries.len(), 50);
        assert_ne!(later.entries[0].id(), page.entries[0].id());
    }

    #[test]
    fn the_search_for_a_missing_result_stops_after_a_bound() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("open.jsonl");
        let filler = fixtures::line(&json!({ "type": "queue-operation", "pad": "x".repeat(1000) }));
        let first = fixtures::line(&fixtures::user("u-first", json!("Hi"), json!({})));
        let mut content = format!("{first}{}", open_call("a-open"));
        let rest = usize::try_from(LOOKAHEAD_BYTES).unwrap() / filler.len() + 100;
        content.push_str(&filler.repeat(rest));
        content.push_str(&open_result());
        std::fs::write(&path, &content).unwrap();
        let mut lines = jsonl::open_lines(&path, 0).unwrap();
        let page = parse_lines(&mut lines, 2).unwrap();
        assert_eq!(
            page.cursor,
            len(&first),
            "the open call is read again later"
        );
        assert!(
            lines.complete() < len(&content),
            "the result is out of reach"
        );
        assert!(lines.complete() < LOOKAHEAD_BYTES + 100_000);
        match &page.entries[1] {
            TranscriptEntry::Tool { result, .. } => assert_eq!(*result, None),
            other => panic!("no tool: {other:?}"),
        }
    }
}
