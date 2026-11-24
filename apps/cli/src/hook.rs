use std::collections::BTreeMap;
use std::env;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use nullradix_api::{HookEventDto, HookKindDto, HookReplyDto};
use nullradix_app::{Clock, SystemClock};
use nullradix_client::{Client, ClientError, DiscoveryFile, Spool, SpooledHook};
use nullradix_os::Paths;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::config::Profile;
use crate::token;

/// Claude Code waits 5 s for a hook; the watchdog ends it well before that.
const DEADLINE: Duration = Duration::from_millis(3000);
const STDIN_TIMEOUT: Duration = Duration::from_millis(1500);
/// Larger input is no hook payload.
const STDIN_LIMIT: u64 = 4 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
/// Hooks whose answer reaches the conversation may wait longer than mere notices.
const ANSWER_TIMEOUT: Duration = Duration::from_millis(1000);
const NOTICE_TIMEOUT: Duration = Duration::from_millis(300);

/// Reports a hook to the daemon; it always succeeds and never starts the daemon.
pub(crate) fn run(event: &str, profile: Profile, home: Option<&Path>) -> ExitCode {
    let deadline = env::var("NULLRADIX_HOOK_DEADLINE_MS")
        .ok()
        .and_then(|millis| millis.parse().ok())
        .map_or(DEADLINE, Duration::from_millis);
    thread::spawn(move || {
        thread::sleep(deadline);
        debug("deadline reached");
        std::process::exit(0);
    });
    let Ok(kind) = serde_json::from_value::<HookKindDto>(Value::from(event)) else {
        debug(&format!("unknown hook {event}"));
        return ExitCode::SUCCESS;
    };
    if let Some(reply) = report(kind, profile, home) {
        answer(kind, &reply);
    }
    ExitCode::SUCCESS
}

fn debug(message: &str) {
    if env::var_os("NULLRADIX_DEBUG").is_some_and(|value| value == "1") {
        eprintln!("nullradix hook: {message}");
    }
}

fn report(kind: HookKindDto, profile: Profile, home: Option<&Path>) -> Option<HookReplyDto> {
    let input = read_stdin()?;
    let Some(event) = event(&input) else {
        debug("input without session_id and cwd");
        return None;
    };
    let paths = Paths::resolve(profile.app_name(), home).ok()?;
    match deliver(&paths, kind, &event) {
        Delivery::Answered(reply) => Some(reply),
        Delivery::Refused(error) => {
            debug(&format!("refused: {error}"));
            None
        }
        Delivery::Unreachable(reason) => {
            debug(&format!("spooled: {reason}"));
            if let Err(error) = Spool::in_dir(&paths.data).push(&SpooledHook { kind, event }) {
                debug(&format!("cannot spool: {error}"));
            }
            None
        }
    }
}

fn read_stdin() -> Option<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut input = Vec::new();
        let read = io::stdin().take(STDIN_LIMIT).read_to_end(&mut input);
        let _ = sender.send(read.map(|_| input));
    });
    match receiver.recv_timeout(STDIN_TIMEOUT) {
        Ok(Ok(input)) => Some(input),
        Ok(Err(error)) => {
            debug(&format!("cannot read input: {error}"));
            None
        }
        Err(_) => {
            debug("no input in time");
            None
        }
    }
}

/// The event in Claude Code's hook input; text fields of other types are left out.
fn event(input: &[u8]) -> Option<HookEventDto> {
    let payload: Map<String, Value> = serde_json::from_slice(input).ok()?;
    let text = |key: &str| payload.get(key).and_then(Value::as_str).map(str::to_owned);
    let variable = |key: &str| env::var(key).ok().filter(|value| !value.is_empty());
    Some(HookEventDto {
        id: Uuid::now_v7(),
        session_id: text("session_id")?,
        cwd: text("cwd")?,
        occurred_at: SystemClock.now().as_millis(),
        transcript_path: text("transcript_path"),
        prompt: text("prompt"),
        source: text("source"),
        model: text("model"),
        reason: text("reason"),
        message: text("message"),
        notification_type: text("notification_type"),
        agent_id: text("agent_id"),
        agent_type: text("agent_type"),
        ticket: variable("NULLRADIX_TICKET"),
        run: variable("NULLRADIX_RUN"),
        entrypoint: variable("CLAUDE_CODE_ENTRYPOINT"),
    })
}

enum Delivery {
    Answered(HookReplyDto),
    Refused(ClientError),
    Unreachable(String),
}

fn deliver(paths: &Paths, kind: HookKindDto, event: &HookEventDto) -> Delivery {
    let discovery = match DiscoveryFile::in_dir(&paths.data).read() {
        Ok(Some(discovery)) if discovery.process_alive() => discovery,
        Ok(_) => return Delivery::Unreachable(String::from("the daemon is not running")),
        Err(error) => return Delivery::Unreachable(error.to_string()),
    };
    let token = match token::read(&token::path(paths)) {
        Ok(Some(token)) => token,
        Ok(None) => return Delivery::Unreachable(String::from("no local token")),
        Err(error) => return Delivery::Unreachable(error.to_string()),
    };
    let timeout = if kind.answers() {
        ANSWER_TIMEOUT
    } else {
        NOTICE_TIMEOUT
    };
    let client = Client::new(discovery.address, CONNECT_TIMEOUT, timeout).with_token(token);
    match client.post_hook(kind, event) {
        Ok(reply) => Delivery::Answered(reply),
        // the daemon judged the event itself, sending it again would not change that
        Err(
            error @ ClientError::Refused {
                status: 400 | 403 | 413 | 422,
                ..
            },
        ) => Delivery::Refused(error),
        Err(error) => Delivery::Unreachable(error.to_string()),
    }
}

/// Hands the reply to Claude Code: variables to its env file, context on stdout.
fn answer(kind: HookKindDto, reply: &HookReplyDto) {
    if kind == HookKindDto::SessionStart
        && let Some(file) = env::var_os("CLAUDE_ENV_FILE")
        && let Err(error) = append_env(Path::new(&file), &reply.env, |name| env::var(name).ok())
    {
        debug(&format!("cannot write the env file: {error}"));
    }
    let name = match kind {
        HookKindDto::SessionStart => "SessionStart",
        HookKindDto::UserPrompt => "UserPromptSubmit",
        _ => return,
    };
    let Some(context) = reply
        .additional_context
        .as_deref()
        .filter(|text| !text.is_empty())
    else {
        return;
    };
    let output = json!({
        "hookSpecificOutput": { "hookEventName": name, "additionalContext": context }
    });
    let mut stdout = io::stdout().lock();
    if let Err(error) = writeln!(stdout, "{output}").and_then(|()| stdout.flush()) {
        debug(&format!("cannot answer: {error}"));
    }
}

/// Appends `export` lines for variables the session does not have yet.
fn append_env(
    file: &Path,
    variables: &BTreeMap<String, String>,
    current: impl Fn(&str) -> Option<String>,
) -> io::Result<()> {
    let mut lines = String::new();
    let missing = variables
        .iter()
        .filter(|(name, _)| valid_name(name))
        .filter(|(name, _)| current(name).is_none_or(|value| value.is_empty()));
    for (name, value) in missing {
        lines.push_str("export ");
        lines.push_str(name);
        lines.push('=');
        lines.push_str(&quote(value));
        lines.push('\n');
    }
    if lines.is_empty() {
        return Ok(());
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?
        .write_all(lines.as_bytes())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|first: char| first.is_ascii_digit())
        && name
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || letter == '_')
}

/// Single quotes for the shell; an embedded quote closes, escapes and reopens.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claude_input_and_skips_what_is_not_text() {
        let input = br#"{"session_id":"s-1","cwd":"/repo","prompt":"hi","model":{"id":"x"}}"#;
        let event = event(input).unwrap();
        assert_eq!(event.session_id, "s-1");
        assert_eq!(event.cwd, "/repo");
        assert_eq!(event.prompt.as_deref(), Some("hi"));
        assert_eq!(event.model, None);
        assert!(event.occurred_at > 0);

        assert!(super::event(br#"{"cwd":"/repo"}"#).is_none());
        assert!(super::event(b"not json").is_none());
        assert!(super::event(b"[1,2]").is_none());
    }

    #[test]
    fn env_lines_are_quoted_and_never_override() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("env");
        let variables = BTreeMap::from([
            (String::from("NULLRADIX_SESSION"), String::from("it's 1")),
            (String::from("NULLRADIX_TICKET"), String::from("ARC-0001")),
            (String::from("BAD NAME"), String::from("x")),
            (String::from("1ST"), String::from("x")),
        ]);
        let current = |name: &str| (name == "NULLRADIX_TICKET").then(|| String::from("ARC-0009"));
        append_env(&file, &variables, current).unwrap();
        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(written, "export NULLRADIX_SESSION='it'\\''s 1'\n");

        let nothing = dir.path().join("untouched");
        append_env(&nothing, &BTreeMap::new(), |_| None).unwrap();
        assert!(!nothing.exists());
    }
}
