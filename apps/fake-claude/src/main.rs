//! Stands in for Claude Code in tests; `FAKE_CLAUDE_MODE` picks the behaviour. Never shipped.

use std::env;
use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

/// A one-pixel PNG, base64 encoded.
const PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn main() -> ExitCode {
    let mut prompt = String::new();
    let _ = io::stdin().read_to_string(&mut prompt);
    let mode = env::var("FAKE_CLAUDE_MODE").unwrap_or_else(|_| String::from("ok"));
    match mode.as_str() {
        // runs until stopped, for timeouts and cancels
        "sleep" => sleep_forever(),
        "fail" => {
            let _ = io::stderr().write_all(b"broken");
            ExitCode::from(3)
        }
        "stream" => {
            stream(&ticket());
            ExitCode::SUCCESS
        }
        "image" => {
            image();
            ExitCode::SUCCESS
        }
        "huge" => {
            huge();
            ExitCode::FAILURE
        }
        "split" => {
            split();
            ExitCode::SUCCESS
        }
        "escape" => escape(),
        "cwd" => {
            let cwd = env::current_dir().unwrap_or_default();
            write(format!("cwd={}\n", cwd.display()).as_bytes());
            ExitCode::SUCCESS
        }
        _ => {
            write(result(&reply(&mode, &prompt)).to_string().as_bytes());
            ExitCode::SUCCESS
        }
    }
}

fn ticket() -> String {
    env::var("NULLRADIX_TICKET").unwrap_or_else(|_| String::from("undefined"))
}

fn write(bytes: &[u8]) {
    let mut stdout = io::stdout();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}

fn sleep_forever() -> ExitCode {
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

/// One JSON event per line, as Claude Code streams them.
fn lines(events: &[Value]) -> String {
    let mut text = String::new();
    for event in events {
        text.push_str(&event.to_string());
        text.push('\n');
    }
    text
}

fn result(text: &str) -> Value {
    json!({ "type": "result", "subtype": "success", "is_error": false, "result": text })
}

/// The answer text: a given reply, the usage limit message, or an echo of what came in.
fn reply(mode: &str, prompt: &str) -> String {
    match mode {
        // a file lets a test change the reply between two runs
        "reply" => match env::var("FAKE_CLAUDE_REPLY_FILE") {
            Ok(path) => std::fs::read_to_string(path).unwrap_or_default(),
            Err(_) => env::var("FAKE_CLAUDE_REPLY").unwrap_or_default(),
        },
        "limit" => String::from("Claude usage limit reached"),
        _ => {
            let key = env::var("ANTHROPIC_API_KEY").unwrap_or_else(|_| String::from("-"));
            let args: Vec<String> = env::args().skip(1).collect();
            format!(
                "ticket={} key={key} args={} prompt={prompt}",
                ticket(),
                args.join(" ")
            )
        }
    }
}

/// Like `--output-format stream-json`, cut mid-line so readers must join the halves.
fn stream(ticket: &str) {
    let events = [
        json!({ "type": "system", "subtype": "init", "model": "fake-opus" }),
        json!({
            "type": "assistant",
            "message": { "content": [
                { "type": "text", "text": "Reading the ticket." },
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": { "command": format!("nullradix ticket show {ticket}") },
                },
            ] }
        }),
        json!({
            "type": "user",
            "message": { "content": [{ "type": "tool_result", "is_error": true, "content": "denied" }] }
        }),
        json!({
            "type": "result", "subtype": "success", "is_error": false, "num_turns": 3,
            "duration_ms": 60000, "result": "done",
            "permission_denials": [{ "tool_name": "Bash", "tool_input": { "command": "git push" } }]
        }),
    ];
    let text = lines(&events);
    let half = text
        .char_indices()
        .nth(text.chars().count() / 2)
        .map_or(0, |(index, _)| index);
    write(&text.as_bytes()[..half]);
    thread::sleep(Duration::from_millis(100));
    write(&text.as_bytes()[half..]);
}

/// A stream with a session id and a screenshot inside a tool result.
fn image() {
    let events = [
        json!({ "type": "system", "subtype": "init", "model": "fake-opus", "session_id": "session-1" }),
        json!({
            "type": "assistant", "session_id": "session-1",
            "message": { "content": [
                { "type": "tool_use", "id": "toolu_image", "name": "mcp__browser__screenshot", "input": {} },
            ] }
        }),
        json!({
            "type": "user", "session_id": "session-1",
            "message": { "content": [{
                "type": "tool_result", "tool_use_id": "toolu_image",
                "content": [
                    { "type": "text", "text": "Screenshot taken" },
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": PIXEL_PNG } },
                ]
            }] }
        }),
        json!({
            "type": "result", "subtype": "success", "is_error": false, "num_turns": 2,
            "duration_ms": 1000, "result": "done", "session_id": "session-1"
        }),
    ];
    let text = lines(&events);
    write(text.as_bytes());
}

/// An overlong first line before the result; readers must keep only whole lines.
fn huge() {
    let line = format!(
        "{{\"type\":\"user\",\"x\":\"{}rate_limit{}\"}}\n",
        "a".repeat(150_000),
        "b".repeat(100_000)
    );
    write(line.as_bytes());
    write(b"{\"type\":\"result\",\"subtype\":\"error_max_turns\",\"is_error\":true,\"result\":\"\"}\n");
}

/// One character in two writes; readers must join the bytes.
fn split() {
    let bytes = "\u{e4}".as_bytes();
    write(&bytes[..1]);
    thread::sleep(Duration::from_millis(200));
    write(&bytes[1..]);
}

/// Leaves the process group of its starter, so only killing its own id stops it.
fn escape() -> ExitCode {
    #[cfg(unix)]
    {
        let _ = nix::unistd::setsid();
    }
    sleep_forever()
}
