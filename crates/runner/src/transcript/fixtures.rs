//! A short made-up transcript shaped like Claude Code's JSONL files, for the tests.

use serde_json::{Value, json};

/// Folder of the sample session, as Claude Code writes it on Windows.
pub(crate) const CWD: &str = "D:\\Projects\\demo";

/// One JSONL line with its newline.
pub(crate) fn line(value: &Value) -> String {
    format!("{value}\n")
}

fn base(kind: &str, uuid: &str, at: &str, entrypoint: &str, branch: &str) -> Value {
    json!({
        "type": kind,
        "uuid": uuid,
        "timestamp": at,
        "sessionId": "s-demo",
        "entrypoint": entrypoint,
        "cwd": CWD,
        "gitBranch": branch,
        "version": "2.1.0",
        "isSidechain": false,
    })
}

fn with(mut value: Value, fields: Value) -> Value {
    if let (Some(target), Value::Object(source)) = (value.as_object_mut(), fields) {
        target.extend(source);
    }
    value
}

pub(crate) fn user(uuid: &str, content: Value, extra: Value) -> Value {
    let mut line = base(
        "user",
        uuid,
        "2026-11-04T08:00:00.000Z",
        "claude-desktop",
        "main",
    );
    line["message"] = json!({ "role": "user" });
    line["message"]["content"] = content;
    with(line, extra)
}

/// An answer line with one block and the tokens of answer `message_id`.
pub(crate) fn assistant(uuid: &str, message_id: &str, block: Value, usage: [i64; 4]) -> Value {
    let mut line = with(
        base(
            "assistant",
            uuid,
            "2026-11-04T08:01:00.000Z",
            "cli",
            "feature/demo",
        ),
        json!({
            "message": {
                "id": message_id,
                "model": "claude-demo",
                "role": "assistant",
                "usage": {
                    "input_tokens": usage[0],
                    "output_tokens": usage[1],
                    "cache_read_input_tokens": usage[2],
                    "cache_creation_input_tokens": usage[3],
                },
            },
        }),
    );
    line["message"]["content"] = Value::Array(vec![block]);
    line
}

pub(crate) fn tool_use(
    uuid: &str,
    message_id: &str,
    id: &str,
    name: &str,
    input: Value,
    usage: [i64; 4],
) -> Value {
    let mut block = json!({ "type": "tool_use", "id": id, "name": name });
    block["input"] = input;
    assistant(uuid, message_id, block, usage)
}

pub(crate) fn tool_result(
    uuid: &str,
    id: &str,
    content: Value,
    is_error: bool,
    details: Value,
) -> Value {
    let mut part = json!({ "type": "tool_result", "tool_use_id": id, "is_error": is_error });
    part["content"] = content;
    let mut extra = json!({});
    extra["toolUseResult"] = details;
    user(uuid, Value::Array(vec![part]), extra)
}

/// A path inside the session folder.
pub(crate) fn file(relative: &str) -> String {
    format!("{CWD}\\{}", relative.replace('/', "\\"))
}

fn system(uuid: &str, fields: Value) -> Value {
    with(
        json!({ "type": "system", "uuid": uuid, "timestamp": "2026-11-04T08:02:00.000Z", "isSidechain": false }),
        fields,
    )
}

/// Every line type, one broken line, tokens, cost, todos, an edit and a new file.
pub(crate) fn transcript() -> String {
    let mut text: String = first_lines()
        .iter()
        .chain(&middle_lines())
        .map(line)
        .collect();
    text.push_str("{broken\n");
    text.extend(last_lines().iter().map(line));
    text
}

/// Rounds of a prompt, a tool call with its result and an answer, each with its own ids.
pub(crate) fn repeated(rounds: usize) -> String {
    let mut text = String::new();
    for round in 0..rounds {
        let tool = format!("toolu_{round}");
        let lines = [
            user(
                &format!("p{round}"),
                json!(format!("Question {round}")),
                json!({}),
            ),
            tool_use(
                &format!("c{round}"),
                &format!("m{round}"),
                &tool,
                "Bash",
                json!({ "command": "ls" }),
                [0; 4],
            ),
            tool_result(&format!("r{round}"), &tool, json!("ok"), false, json!({})),
            assistant(
                &format!("a{round}"),
                &format!("n{round}"),
                json!({ "type": "text", "text": format!("Answer {round}") }),
                [0; 4],
            ),
        ];
        text.extend(lines.iter().map(line));
    }
    text
}

fn first_lines() -> Vec<Value> {
    vec![
        json!({ "type": "custom-title", "customTitle": "Demo title", "sessionId": "s-demo" }),
        user(
            "u-prompt",
            json!("Please <system-reminder>hidden</system-reminder>add a function"),
            json!({ "origin": { "kind": "human" } }),
        ),
        user(
            "u-meta",
            json!("<local-command-caveat>Note</local-command-caveat>"),
            json!({ "isMeta": true }),
        ),
        user(
            "u-command",
            json!(
                "<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>opus</command-args>"
            ),
            json!({}),
        ),
        user(
            "u-stdout",
            json!("<local-command-stdout>\u{1b}[1mModel set\u{1b}[22m</local-command-stdout>"),
            json!({}),
        ),
        with(
            assistant(
                "a-think",
                "m1",
                json!({ "type": "thinking", "thinking": "", "signature": "x" }),
                [10, 5, 100, 50],
            ),
            json!({ "thinkingDurationMs": 1500 }),
        ),
        assistant(
            "a-text",
            "m1",
            json!({ "type": "text", "text": "I read the file." }),
            [10, 5, 100, 50],
        ),
        tool_use(
            "a-read",
            "m1",
            "toolu_read",
            "Read",
            json!({ "file_path": file("src/main.rs") }),
            [10, 5, 100, 50],
        ),
        tool_result(
            "u-read",
            "toolu_read",
            json!("1\tfn main() {}"),
            false,
            json!({ "type": "text", "file": { "filePath": file("src/main.rs") } }),
        ),
        tool_use(
            "a-edit",
            "m2",
            "toolu_edit",
            "Edit",
            json!({ "file_path": file("src/main.rs"), "old_string": "fn main() {}", "new_string": "fn main() {\n    hello();\n}", "replace_all": false }),
            [2, 20, 150, 10],
        ),
        tool_result(
            "u-edit",
            "toolu_edit",
            json!("The file has been updated successfully."),
            false,
            json!({
                "filePath": file("src/main.rs"),
                "oldString": "fn main() {}",
                "newString": "fn main() {\n    hello();\n}",
                "originalFile": "fn main() {}\n",
                "structuredPatch": [],
                "replaceAll": false,
            }),
        ),
    ]
}

fn middle_lines() -> Vec<Value> {
    vec![
        tool_use(
            "a-write",
            "m3",
            "toolu_write",
            "Write",
            json!({ "file_path": file("src/new.rs"), "content": "pub fn hello() {}\n" }),
            [3, 30, 160, 0],
        ),
        tool_result(
            "u-write",
            "toolu_write",
            json!("File created successfully"),
            false,
            json!({ "type": "create", "filePath": file("src/new.rs"), "content": "pub fn hello() {}\n", "structuredPatch": [], "originalFile": null }),
        ),
        tool_use(
            "a-bash",
            "m4",
            "toolu_bash",
            "Bash",
            json!({ "command": "cargo test", "description": "Tests" }),
            [1, 1, 170, 0],
        ),
        tool_result(
            "u-bash",
            "toolu_bash",
            json!([{ "type": "text", "text": "Exit code 1" }, { "type": "image", "source": {} }]),
            true,
            json!("Error: Exit code 1"),
        ),
        tool_use(
            "a-grep",
            "m5",
            "toolu_grep",
            "Grep",
            json!({ "pattern": "hello" }),
            [1, 1, 180, 0],
        ),
        tool_result(
            "u-grep",
            "toolu_grep",
            json!("x".repeat(25_000)),
            false,
            json!({}),
        ),
        with(
            assistant(
                "a-side",
                "m-side",
                json!({ "type": "text", "text": "Subagent" }),
                [100, 100, 0, 0],
            ),
            json!({ "isSidechain": true }),
        ),
        system(
            "s-hook",
            json!({ "subtype": "stop_hook_summary", "hookErrors": ["Hook failed"], "hookCount": 1 }),
        ),
        system(
            "s-api",
            json!({ "subtype": "api_error", "level": "error", "error": { "formatted": "Overloaded" } }),
        ),
        system(
            "s-compact",
            json!({ "subtype": "compact_boundary", "content": "Conversation compacted", "level": "info" }),
        ),
        system(
            "s-turn",
            json!({ "subtype": "turn_duration", "durationMs": 1000 }),
        ),
    ]
}

fn last_lines() -> Vec<Value> {
    vec![
        tool_use(
            "a-todo",
            "m6",
            "toolu_todo",
            "TodoWrite",
            json!({ "todos": [
                { "content": "Read the plan", "status": "completed", "activeForm": "Reading the plan" },
                { "content": "Build it", "status": "in_progress", "activeForm": "Building it" },
            ] }),
            [1, 2, 190, 0],
        ),
        tool_result(
            "u-todo",
            "toolu_todo",
            json!("Todos have been modified successfully."),
            false,
            json!({}),
        ),
        tool_use(
            "a-task",
            "m7",
            "toolu_task",
            "TaskCreate",
            json!({ "subject": "Write tests", "description": "x", "activeForm": "Writing tests" }),
            [1, 2, 200, 0],
        ),
        tool_result(
            "u-task",
            "toolu_task",
            json!("Task #7 created successfully: Write tests"),
            false,
            json!({ "task": { "id": "7", "subject": "Write tests" } }),
        ),
        tool_use(
            "a-update",
            "m8",
            "toolu_update",
            "TaskUpdate",
            json!({ "taskId": "7", "status": "completed" }),
            [1, 2, 210, 0],
        ),
        tool_result(
            "u-update",
            "toolu_update",
            json!("Updated task #7 status"),
            false,
            json!({ "success": true, "taskId": "7", "updatedFields": ["status"] }),
        ),
        user(
            "u-notification",
            json!(
                "<task-notification>\n<task-id>b1</task-id>\n<summary>Background task done</summary>\n</task-notification>"
            ),
            json!({ "origin": { "kind": "task-notification" } }),
        ),
        json!({
            "type": "cost-state",
            "totalCostUSD": 0.42,
            "totalDuration": 65_000,
            "totalLinesAdded": 4,
            "totalLinesRemoved": 1,
            "modelUsage": {
                "claude-demo": { "inputTokens": 100, "outputTokens": 200, "cacheReadInputTokens": 2000, "cacheCreationInputTokens": 250, "costUSD": 0.4 },
                "claude-small": { "inputTokens": 20, "outputTokens": 3, "cacheReadInputTokens": 300, "cacheCreationInputTokens": 10, "costUSD": 0.02 },
            },
        }),
        assistant(
            "a-final",
            "m9",
            json!({ "type": "text", "text": "Done." }),
            [5, 40, 1000, 200],
        ),
    ]
}

/// The sample without its `cost-state` line, so tokens come from the answers.
pub(crate) fn transcript_without_costs() -> String {
    transcript()
        .lines()
        .filter(|line| !line.contains("\"cost-state\""))
        .fold(String::new(), |mut text, line| {
            text.push_str(line);
            text.push('\n');
            text
        })
}
