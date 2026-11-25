//! Reads the stream-json of Claude Code: usage, denials, session and errors.

use std::sync::LazyLock;

use nullradix_domain::{Abort, AbortCategory, DeniedTool, ModelUsage, RunUsage};
use regex::Regex;
use serde_json::{Map, Value};

/// Loose on purpose; Claude Code words its limit messages differently by version and plan.
static USAGE_LIMIT: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)usage limit|limit reached|rate.?limit|hit (?:your|the)\b[^.\n]{0,30}\blimit")
        .ok()
});
static CONTEXT_OVERFLOW: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)prompt is too long|context length|context window|input is too long|exceeds the maximum context",
    )
    .ok()
});

/// Characters of error text kept; the end of the output says why a run failed.
const ERROR_TAIL_CHARS: usize = 4000;
/// Denied calls named in an abort reason.
const MAX_DENIED_EXAMPLES: usize = 5;
const DENIED_EXAMPLE_CHARS: usize = 100;

fn matches(pattern: &LazyLock<Option<Regex>>, text: &str) -> bool {
    pattern
        .as_ref()
        .is_some_and(|pattern| pattern.is_match(text))
}

#[must_use]
pub fn looks_like_usage_limit(text: &str) -> bool {
    matches(&USAGE_LIMIT, text)
}

#[must_use]
pub fn looks_like_context_overflow(text: &str) -> bool {
    matches(&CONTEXT_OVERFLOW, text)
}

/// Largest line joined from pieces; a screenshot of 20 MB is about 27 MB in base64.
pub const MAX_JOINED_BYTES: usize = 32 * 1024 * 1024;

/// A line put together from the pieces the runner cut it into.
#[derive(Debug, PartialEq, Eq)]
pub enum Joined {
    Line(String),
    /// The line exceeded the cap; none of it is kept.
    Dropped,
}

/// Joins the pieces of one pipe back into lines, up to a cap.
#[derive(Debug)]
pub struct LineJoiner {
    text: String,
    cap: usize,
    open: bool,
    overflow: bool,
}

impl Default for LineJoiner {
    fn default() -> Self {
        Self::with_cap(MAX_JOINED_BYTES)
    }
}

impl LineJoiner {
    #[must_use]
    pub fn with_cap(cap: usize) -> Self {
        Self {
            text: String::new(),
            cap,
            open: false,
            overflow: false,
        }
    }

    /// Whether no line is half read.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        !self.open
    }

    /// Takes one piece; `continues` says the line goes on in the next piece.
    pub fn push(&mut self, piece: &str, continues: bool) -> Option<Joined> {
        if !self.open && !continues {
            return Some(Joined::Line(piece.to_owned()));
        }
        self.open = true;
        if !self.overflow {
            if self.text.len() + piece.len() > self.cap {
                self.overflow = true;
                self.text = String::new();
            } else {
                self.text.push_str(piece);
            }
        }
        if continues { None } else { self.finish() }
    }

    /// Ends a half read line, as when the output stops for good.
    pub fn finish(&mut self) -> Option<Joined> {
        if !self.open {
            return None;
        }
        self.open = false;
        let text = std::mem::take(&mut self.text);
        Some(if std::mem::take(&mut self.overflow) {
            Joined::Dropped
        } else {
            Joined::Line(text)
        })
    }
}

/// Which pipe of the process a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pipe {
    Stdout,
    Stderr,
}

/// What a finished Claude run said, boiled down.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClaudeOutput {
    pub session_id: Option<String>,
    /// Whether a result event was seen at all.
    pub has_result: bool,
    pub subtype: Option<String>,
    pub is_error: bool,
    /// Stderr, lines that are no events and the result text of a failed run; the end of it.
    pub error_text: String,
    pub usage: Option<RunUsage>,
    pub denied: Vec<DeniedTool>,
    /// At most five distinct denied calls as `Tool: what`, for abort reasons.
    pub denied_examples: Vec<String>,
}

impl ClaudeOutput {
    #[must_use]
    pub fn denied_count(&self) -> Option<u32> {
        self.has_result
            .then(|| self.denied.iter().map(|tool| tool.count).sum())
    }
}

/// Takes the lines of a run one by one; memory stays bounded however long the run is.
#[derive(Debug, Default)]
pub struct ClaudeStream {
    session_id: Option<String>,
    result: Option<Map<String, Value>>,
    stray: Vec<String>,
    stray_chars: usize,
}

fn event(line: &str) -> Option<Map<String, Value>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Object(object)) if object.get("type").is_some_and(Value::is_string) => {
            Some(object)
        }
        _ => None,
    }
}

impl ClaudeStream {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, pipe: Pipe, line: &str) {
        // a result is rare and an image line is huge, so look only at lines that can matter
        let wants_event = pipe == Pipe::Stdout && line.trim_start().starts_with('{');
        match wants_event.then(|| event(line)).flatten() {
            Some(object) => self.event(object),
            None => self.stray(line),
        }
    }

    fn event(&mut self, object: Map<String, Value>) {
        if self.session_id.is_none()
            && let Some(id) = object
                .get("session_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
        {
            self.session_id = Some(id.to_owned());
        }
        if object.get("type").and_then(Value::as_str) == Some("result") {
            self.result = Some(object);
        }
    }

    fn stray(&mut self, line: &str) {
        let line = line.trim_end();
        if line.is_empty() {
            return;
        }
        // one line may be huge, only its end can matter
        let line = &tail(line);
        self.stray_chars += line.chars().count() + 1;
        self.stray.push(line.to_owned());
        while self.stray_chars > ERROR_TAIL_CHARS && self.stray.len() > 1 {
            let dropped = self.stray.remove(0);
            self.stray_chars = self.stray_chars.saturating_sub(dropped.chars().count() + 1);
        }
    }

    #[must_use]
    pub fn finish(self) -> ClaudeOutput {
        let Some(result) = &self.result else {
            return ClaudeOutput {
                session_id: self.session_id,
                error_text: tail(&self.stray.join("\n")),
                ..ClaudeOutput::default()
            };
        };
        let is_error = result.get("is_error") == Some(&Value::Bool(true));
        let mut error_text = self.stray.join("\n");
        if is_error && let Some(text) = result.get("result").and_then(Value::as_str) {
            error_text = format!("{error_text}\n{text}");
        }
        let (denied, denied_examples) = denials(result.get("permission_denials"));
        ClaudeOutput {
            session_id: self.session_id,
            has_result: true,
            subtype: result
                .get("subtype")
                .and_then(Value::as_str)
                .map(str::to_owned),
            is_error,
            error_text: tail(error_text.trim()),
            usage: Some(usage(result)),
            denied,
            denied_examples,
        }
    }
}

fn tail(text: &str) -> String {
    let count = text.chars().count();
    if count <= ERROR_TAIL_CHARS {
        return text.to_owned();
    }
    text.chars().skip(count - ERROR_TAIL_CHARS).collect()
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "token counts and costs fit i64 by far"
)]
fn whole(value: f64) -> i64 {
    value.round() as i64
}

fn count(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|number| number.is_finite())
            .map(whole)
    })
}

fn model_usage(model: &str, value: &Value) -> ModelUsage {
    ModelUsage {
        model: model.to_owned(),
        input_tokens: count(value.get("inputTokens")),
        output_tokens: count(value.get("outputTokens")),
        cache_read_tokens: count(value.get("cacheReadInputTokens")),
        cache_write_tokens: count(value.get("cacheCreationInputTokens")),
        cost_usd: value.get("costUSD").and_then(Value::as_f64),
    }
}

/// Cost, tokens and models of the result; the main model is the one that cost the most.
fn usage(result: &Map<String, Value>) -> RunUsage {
    let tokens = result.get("usage");
    let token = |name: &str| count(tokens.and_then(|usage| usage.get(name)));
    let models: Vec<ModelUsage> = result
        .get("modelUsage")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, value)| value.is_object())
        .map(|(name, value)| model_usage(name, value))
        .collect();
    let mut main: Option<&ModelUsage> = None;
    for model in &models {
        let better =
            main.is_none_or(|best| model.cost_usd.unwrap_or(-1.0) > best.cost_usd.unwrap_or(-1.0));
        if better {
            main = Some(model);
        }
    }
    RunUsage {
        cost_usd: result.get("total_cost_usd").and_then(Value::as_f64),
        input_tokens: token("input_tokens"),
        output_tokens: token("output_tokens"),
        cache_read_tokens: token("cache_read_input_tokens"),
        cache_write_tokens: token("cache_creation_input_tokens"),
        num_turns: count(result.get("num_turns")),
        model: main.map(|model| model.model.clone()),
        models,
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

/// Tools Claude Code refused with their counts, and a few examples of what was refused.
fn denials(value: Option<&Value>) -> (Vec<DeniedTool>, Vec<String>) {
    let mut tools: Vec<DeniedTool> = Vec::new();
    let mut examples: Vec<String> = Vec::new();
    for entry in value.and_then(Value::as_array).into_iter().flatten() {
        let Some(tool) = entry
            .get("tool_name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        match tools.iter_mut().find(|found| found.tool == tool) {
            Some(found) => found.count += 1,
            None => tools.push(DeniedTool {
                tool: tool.to_owned(),
                count: 1,
            }),
        }
        let input = entry.get("tool_input");
        let detail = ["command", "file_path", "path", "pattern"]
            .iter()
            .find_map(|name| {
                input
                    .and_then(|input| input.get(*name))
                    .and_then(Value::as_str)
            });
        let example = match detail {
            Some(detail) => format!("{tool}: {}", one_line(detail, DENIED_EXAMPLE_CHARS)),
            None => tool.to_owned(),
        };
        if examples.len() < MAX_DENIED_EXAMPLES && !examples.contains(&example) {
            examples.push(example);
        }
    }
    (tools, examples)
}

/// How a run ended, as far as the process and the limits tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ending {
    pub timed_out: bool,
    pub exit_code: Option<i32>,
    pub max_turns: u32,
    pub timeout_minutes: u32,
}

/// Exit code of a program that could not be started, as the runner reports it.
const NOT_FOUND_CODE: i32 = 127;

fn first_line(text: &str, max: usize) -> String {
    one_line(
        text.lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or(""),
        max,
    )
}

fn base_reason(ending: &Ending, output: &ClaudeOutput) -> Option<Abort> {
    let abort = |category, reason: String| Some(Abort { category, reason });
    if ending.timed_out {
        return abort(
            AbortCategory::Timeout,
            format!(
                "time limit of {} min reached, run stopped",
                ending.timeout_minutes
            ),
        );
    }
    let failed = output.is_error || ending.exit_code.is_some_and(|code| code != 0);
    if failed && looks_like_usage_limit(&output.error_text) {
        return abort(
            AbortCategory::UsageLimit,
            format!("usage limit: {}", first_line(&output.error_text, 200)),
        );
    }
    if output.subtype.as_deref() == Some("error_max_turns") {
        return abort(
            AbortCategory::MaxTurns,
            format!("maximum number of turns ({}) reached", ending.max_turns),
        );
    }
    if failed && looks_like_context_overflow(&output.error_text) {
        return abort(
            AbortCategory::Context,
            format!("context full: {}", first_line(&output.error_text, 200)),
        );
    }
    match ending.exit_code {
        Some(NOT_FOUND_CODE) => abort(
            AbortCategory::StartFailed,
            format!(
                "claude could not be started: {}",
                first_line(&output.error_text, 200)
            ),
        ),
        Some(code) if code != 0 => abort(
            AbortCategory::ExitCode,
            format!("claude exited with code {code}"),
        ),
        _ if output.is_error => abort(
            AbortCategory::ClaudeError,
            format!(
                "claude reported an error ({})",
                output.subtype.as_deref().unwrap_or("unknown")
            ),
        ),
        _ => None,
    }
}

/// The abort of a run that did not end well; none for a clean end.
#[must_use]
pub fn classify(ending: &Ending, output: &ClaudeOutput) -> Option<Abort> {
    let mut abort = base_reason(ending, output)?;
    if !output.denied_examples.is_empty() {
        abort.reason = format!(
            "{}; denied tool calls: {} (check the allowed tools in the dispatcher settings)",
            abort.reason,
            output.denied_examples.join(", ")
        );
    }
    Some(abort)
}

/// An image a tool returned, still base64 encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub tool_use_id: Option<String>,
    pub media_type: String,
    pub data: String,
}

fn images_of(content: &Value, tool_use_id: Option<&str>, found: &mut Vec<Image>) {
    for block in content.as_array().into_iter().flatten() {
        if block.get("type").and_then(Value::as_str) != Some("image") {
            continue;
        }
        let source = block.get("source");
        let text = |name: &str| {
            source
                .and_then(|source| source.get(name))
                .and_then(Value::as_str)
        };
        if let (Some(media_type), Some(data)) = (text("media_type"), text("data")) {
            found.push(Image {
                tool_use_id: tool_use_id.map(str::to_owned),
                media_type: media_type.to_owned(),
                data: data.to_owned(),
            });
        }
    }
}

/// Images in the tool results of one stdout line; most lines are looked at only as text.
#[must_use]
pub fn images(line: &str) -> Vec<Image> {
    if !line.contains("\"image\"") {
        return Vec::new();
    }
    let Some(object) = event(line) else {
        return Vec::new();
    };
    if object.get("type").and_then(Value::as_str) != Some("user") {
        return Vec::new();
    }
    let mut found = Vec::new();
    let blocks = object
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array);
    for block in blocks.into_iter().flatten() {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let id = block.get("tool_use_id").and_then(Value::as_str);
        if let Some(content) = block.get("content") {
            images_of(content, id, &mut found);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(lines: &[(Pipe, String)]) -> ClaudeOutput {
        let mut stream = ClaudeStream::new();
        for (pipe, line) in lines {
            stream.feed(*pipe, line);
        }
        stream.finish()
    }

    fn stdout(value: &Value) -> (Pipe, String) {
        (Pipe::Stdout, value.to_string())
    }

    fn success() -> Value {
        json!({
            "type": "result", "subtype": "success", "is_error": false, "num_turns": 7,
            "duration_ms": 61000, "total_cost_usd": 0.4321, "session_id": "s-1",
            "result": "done",
            "usage": {
                "input_tokens": 1200, "output_tokens": 340,
                "cache_read_input_tokens": 9000, "cache_creation_input_tokens": 150
            },
            "modelUsage": {
                "claude-haiku": { "inputTokens": 10, "outputTokens": 2, "costUSD": 0.01 },
                "claude-opus": {
                    "inputTokens": 1190, "outputTokens": 338, "cacheReadInputTokens": 9000,
                    "cacheCreationInputTokens": 150, "costUSD": 0.4221
                }
            },
            "permission_denials": [
                { "tool_name": "Bash", "tool_input": { "command": "git push origin main" } },
                { "tool_name": "Bash", "tool_input": { "command": "git push origin main" } },
                { "tool_name": "Write", "tool_input": { "file_path": "/etc/hosts" } }
            ]
        })
    }

    #[test]
    fn reads_cost_tokens_models_and_the_session_of_a_successful_run() {
        let output = parse(&[
            stdout(&json!({ "type": "system", "subtype": "init", "session_id": "s-1" })),
            stdout(&success()),
        ]);
        assert_eq!(output.session_id.as_deref(), Some("s-1"));
        assert!(output.has_result && !output.is_error);
        let usage = output.usage.unwrap();
        assert_eq!(usage.cost_usd, Some(0.4321));
        assert_eq!(
            (usage.input_tokens, usage.output_tokens, usage.num_turns),
            (Some(1200), Some(340), Some(7))
        );
        assert_eq!(
            (usage.cache_read_tokens, usage.cache_write_tokens),
            (Some(9000), Some(150))
        );
        assert_eq!(usage.model.as_deref(), Some("claude-opus"));
        assert_eq!(usage.models.len(), 2);
    }

    #[test]
    fn counts_denied_tools_once_per_tool_and_keeps_examples() {
        let output = parse(&[stdout(&success())]);
        assert_eq!(
            output.denied,
            vec![
                DeniedTool {
                    tool: String::from("Bash"),
                    count: 2
                },
                DeniedTool {
                    tool: String::from("Write"),
                    count: 1
                },
            ]
        );
        assert_eq!(output.denied_count(), Some(3));
        assert_eq!(
            output.denied_examples,
            ["Bash: git push origin main", "Write: /etc/hosts"]
        );
    }

    #[test]
    fn the_last_result_wins_and_a_tie_keeps_the_first_model() {
        let mut first = success();
        first["modelUsage"] = json!({
            "a": { "costUSD": 1.0 }, "b": { "costUSD": 1.0 }
        });
        let output = parse(&[
            stdout(&json!({ "type": "result", "subtype": "error_max_turns", "is_error": true })),
            stdout(&first),
        ]);
        assert_eq!(output.subtype.as_deref(), Some("success"));
        assert_eq!(output.usage.unwrap().model.as_deref(), Some("a"));
    }

    #[test]
    fn a_run_without_a_result_has_no_usage_and_keeps_its_errors() {
        let output = parse(&[
            (Pipe::Stdout, String::from("{\"type\":\"assistant\"")),
            (Pipe::Stderr, String::from("boom")),
            stdout(&json!({ "type": "system", "session_id": "s-2" })),
        ]);
        assert!(!output.has_result);
        assert_eq!(output.usage, None);
        assert_eq!(output.denied_count(), None);
        assert_eq!(output.error_text, "{\"type\":\"assistant\"\nboom");
        assert_eq!(output.session_id.as_deref(), Some("s-2"));
    }

    #[test]
    fn only_the_end_of_a_long_error_text_is_kept() {
        let mut stream = ClaudeStream::new();
        for index in 0..500 {
            stream.feed(Pipe::Stderr, &format!("line {index:04} {}", "x".repeat(50)));
        }
        let text = stream.finish().error_text;
        assert!(text.chars().count() <= ERROR_TAIL_CHARS);
        assert!(text.contains("line 0499"));
        assert!(!text.contains("line 0000"));
    }

    #[test]
    fn the_result_text_counts_as_error_text_only_for_failures() {
        let ok = parse(&[stdout(&json!({
            "type": "result", "subtype": "success", "is_error": false,
            "result": "We hit the rate limit of the API client"
        }))]);
        assert_eq!(ok.error_text, "");
        let failed = parse(&[stdout(&json!({
            "type": "result", "subtype": "success", "is_error": true,
            "result": "You've hit your session limit"
        }))]);
        assert!(failed.error_text.contains("session limit"));
    }

    fn ending(timed_out: bool, exit_code: Option<i32>) -> Ending {
        Ending {
            timed_out,
            exit_code,
            max_turns: 80,
            timeout_minutes: 90,
        }
    }

    fn category(ending: &Ending, output: &ClaudeOutput) -> Option<AbortCategory> {
        classify(ending, output).map(|abort| abort.category)
    }

    #[test]
    fn a_clean_run_has_no_abort_and_a_timeout_comes_first() {
        let clean = parse(&[stdout(&success())]);
        assert_eq!(classify(&ending(false, Some(0)), &clean), None);
        let abort = classify(&ending(true, None), &clean).unwrap();
        assert_eq!(abort.category, AbortCategory::Timeout);
        assert!(abort.reason.starts_with("time limit of 90 min reached"));
    }

    #[test]
    fn usage_limits_count_only_for_failed_runs() {
        for text in [
            "Claude AI usage limit reached|1760000000",
            "You've hit your session limit \u{b7} resets 12:10pm (Europe/Berlin)",
            "You've hit your weekly limit",
            "rate limit exceeded",
        ] {
            assert!(looks_like_usage_limit(text), "{text}");
            let failed = parse(&[(Pipe::Stderr, text.to_owned())]);
            assert_eq!(
                category(&ending(false, Some(1)), &failed),
                Some(AbortCategory::UsageLimit),
                "{text}"
            );
            // the same words in a clean run are only what Claude said
            let clean = parse(&[(Pipe::Stderr, text.to_owned())]);
            assert_eq!(category(&ending(false, Some(0)), &clean), None, "{text}");
        }
    }

    #[test]
    fn max_turns_context_exit_code_and_errors_are_told_apart() {
        let max_turns = parse(&[stdout(&json!({
            "type": "result", "subtype": "error_max_turns", "is_error": true, "result": ""
        }))]);
        let abort = classify(&ending(false, Some(1)), &max_turns).unwrap();
        assert_eq!(abort.category, AbortCategory::MaxTurns);
        assert_eq!(abort.reason, "maximum number of turns (80) reached");

        let context = parse(&[(Pipe::Stderr, String::from("Prompt is too long"))]);
        assert_eq!(
            category(&ending(false, Some(1)), &context),
            Some(AbortCategory::Context)
        );
        let plain = parse(&[]);
        let abort = classify(&ending(false, Some(3)), &plain).unwrap();
        assert_eq!(abort.category, AbortCategory::ExitCode);
        assert_eq!(abort.reason, "claude exited with code 3");
        let missing = parse(&[(Pipe::Stderr, String::from("cannot start claude"))]);
        assert_eq!(
            category(&ending(false, Some(127)), &missing),
            Some(AbortCategory::StartFailed)
        );
        let errored = parse(&[stdout(&json!({
            "type": "result", "subtype": "error_during_execution", "is_error": true
        }))]);
        let abort = classify(&ending(false, Some(0)), &errored).unwrap();
        assert_eq!(abort.category, AbortCategory::ClaudeError);
        assert_eq!(
            abort.reason,
            "claude reported an error (error_during_execution)"
        );
    }

    #[test]
    fn denied_calls_are_named_in_the_reason() {
        let output = parse(&[stdout(&json!({
            "type": "result", "subtype": "error_max_turns", "is_error": true,
            "permission_denials": [{ "tool_name": "Bash", "tool_input": { "command": "make" } }]
        }))]);
        let reason = classify(&ending(false, Some(1)), &output).unwrap().reason;
        assert!(reason.ends_with(
            "denied tool calls: Bash: make (check the allowed tools in the dispatcher settings)"
        ));
    }

    #[test]
    fn joins_pieces_and_drops_a_line_over_the_cap() {
        let mut joiner = LineJoiner::with_cap(6);
        assert_eq!(
            joiner.push("ab", false),
            Some(Joined::Line(String::from("ab")))
        );
        assert_eq!(joiner.push("abc", true), None);
        assert!(!joiner.is_idle());
        assert_eq!(
            joiner.push("def", false),
            Some(Joined::Line(String::from("abcdef")))
        );
        assert!(joiner.is_idle());
        assert_eq!(joiner.push("abcd", true), None);
        assert_eq!(joiner.push("efg", true), None);
        assert_eq!(joiner.push("h", false), Some(Joined::Dropped));
        // the next line starts clean
        assert_eq!(
            joiner.push("ok", false),
            Some(Joined::Line(String::from("ok")))
        );
        assert_eq!(joiner.push("x", true), None);
        assert_eq!(joiner.finish(), Some(Joined::Line(String::from("x"))));
        assert_eq!(joiner.finish(), None);
    }

    #[test]
    fn a_huge_stray_line_keeps_only_its_end() {
        let mut stream = ClaudeStream::new();
        stream.feed(Pipe::Stderr, &format!("{}END", "x".repeat(100_000)));
        let text = stream.finish().error_text;
        assert!(text.chars().count() <= ERROR_TAIL_CHARS && text.ends_with("END"));
    }

    #[test]
    fn finds_images_in_tool_results_only() {
        let line = json!({
            "type": "user",
            "message": { "content": [{
                "type": "tool_result", "tool_use_id": "toolu_1",
                "content": [
                    { "type": "text", "text": "shot" },
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" } }
                ]
            }] }
        })
        .to_string();
        assert_eq!(
            images(&line),
            vec![Image {
                tool_use_id: Some(String::from("toolu_1")),
                media_type: String::from("image/png"),
                data: String::from("AAAA"),
            }]
        );
        assert_eq!(images("{\"type\":\"assistant\"}"), Vec::new());
        assert_eq!(images("plain \"image\" text"), Vec::new());
    }
}
