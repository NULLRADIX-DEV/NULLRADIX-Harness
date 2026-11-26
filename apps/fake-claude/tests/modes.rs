#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn fake(mode: &str, prompt: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nullradix-fake-claude"))
        .args(["-p", "--output-format", "json"])
        .env("FAKE_CLAUDE_MODE", mode)
        .env("NULLRADIX_TICKET", "ARC-0007")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("FAKE_CLAUDE_REPLY_FILE")
        .env("FAKE_CLAUDE_REPLY", "as told")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(prompt.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn json_lines(output: &Output) -> Vec<serde_json::Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn ok_echoes_ticket_arguments_and_prompt() {
    let output = fake("ok", "fix it");
    assert!(output.status.success());
    let result = &json_lines(&output)[0];
    assert_eq!(result["type"], "result");
    assert_eq!(
        result["result"],
        "ticket=ARC-0007 key=- args=-p --output-format json prompt=fix it"
    );
}

#[test]
fn reply_and_limit_answer_as_configured() {
    assert_eq!(json_lines(&fake("reply", ""))[0]["result"], "as told");
    assert_eq!(
        json_lines(&fake("limit", ""))[0]["result"],
        "Claude usage limit reached"
    );
}

#[test]
fn fail_exits_with_3_and_complains() {
    let output = fake("fail", "");
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(output.stderr, b"broken");
}

#[test]
fn stream_and_image_write_one_event_per_line() {
    let stream = json_lines(&fake("stream", ""));
    let kinds: Vec<&str> = stream
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["system", "assistant", "user", "result"]);
    assert!(stream[1].to_string().contains("ARC-0007"));

    let image = json_lines(&fake("image", ""));
    assert_eq!(image.len(), 4);
    assert_eq!(image[3]["session_id"], "session-1");
}

#[test]
fn huge_and_split_test_readers() {
    let huge = fake("huge", "");
    assert_eq!(huge.status.code(), Some(1));
    let lines: Vec<&[u8]> = huge.stdout.split(|byte| *byte == b'\n').collect();
    assert!(lines[0].len() > 250_000);

    let split = fake("split", "");
    assert_eq!(String::from_utf8(split.stdout).unwrap(), "\u{e4}");
}

#[test]
fn cwd_reports_the_working_directory() {
    let output = fake("cwd", "");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("cwd="), "{text}");
}

#[test]
fn usage_streams_events_and_a_result_with_cost_and_tokens() {
    let output = fake("usage", "go");
    assert!(output.status.success());
    let events = json_lines(&output);
    assert_eq!(events[0]["session_id"], "fake-session-1");
    let result = events.last().unwrap();
    assert_eq!(result["type"], "result");
    assert_eq!(result["total_cost_usd"], 0.4321);
    assert_eq!(result["usage"]["output_tokens"], 340);
}

#[test]
fn a_marker_in_the_prompt_beats_the_environment() {
    let output = fake("ok", "do it [fake-claude:usage-limit] now");
    assert_eq!(output.status.code(), Some(1));
    let result = json_lines(&output).pop().unwrap();
    assert_eq!(result["is_error"], true);
    assert!(
        result["result"]
            .as_str()
            .unwrap()
            .contains("usage limit reached")
    );
}

#[test]
fn max_turns_and_denials_end_with_their_subtype_and_list() {
    let output = fake("max-turns", "");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        json_lines(&output).pop().unwrap()["subtype"],
        "error_max_turns"
    );
    let denied = json_lines(&fake("denied", "")).pop().unwrap();
    assert_eq!(denied["permission_denials"].as_array().unwrap().len(), 3);
}

#[test]
fn edit_changes_a_file_in_the_working_folder() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_nullradix-fake-claude"))
        .env("FAKE_CLAUDE_MODE", "edit")
        .env("NULLRADIX_TICKET", "ARC-0007")
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    assert!(child.wait_with_output().unwrap().status.success());
    let text = std::fs::read_to_string(dir.path().join("edited.txt")).unwrap();
    assert_eq!(text, "edited for ARC-0007\n");
}
