#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nullradix_app::auth::Secret;
use nullradix_client::{Client, DiscoveryFile};

mod common;

use common::{Daemon, nullradix};

/// A debug build on a slow CI runner must still answer well inside Claude Code's 5 s.
const BUDGET: Duration = Duration::from_millis(1000);
/// No single hook may come near Claude Code's limit.
const CEILING: Duration = Duration::from_millis(2500);
const REPORTS: usize = 5;

fn cwd(home: &Path) -> String {
    home.to_string_lossy().into_owned()
}

fn hook_command(home: &Path, args: &[&str]) -> Command {
    let mut command = nullradix(home);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Runs the command with `input` on stdin, or with stdin left open when there is none.
fn finish(mut command: Command, input: Option<&[u8]>) -> (Output, Duration) {
    let started = Instant::now();
    let mut child = command.spawn().unwrap();
    let open = match (input, child.stdin.take()) {
        (Some(input), Some(mut pipe)) => {
            pipe.write_all(input).unwrap();
            None
        }
        (_, pipe) => pipe,
    };
    let output = child.wait_with_output().unwrap();
    drop(open);
    (output, started.elapsed())
}

fn hook(home: &Path, event: &str, input: &[u8]) -> (Output, Duration) {
    finish(hook_command(home, &["hook", event]), Some(input))
}

fn assert_in_time(durations: &[Duration]) {
    let mut sorted = durations.to_vec();
    sorted.sort();
    let typical = sorted[sorted.len() / 2];
    let slowest = sorted[sorted.len() - 1];
    assert!(typical < BUDGET, "a typical hook took {typical:?}");
    assert!(slowest < CEILING, "the slowest hook took {slowest:?}");
}

fn payload(home: &Path, session: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "session_id": session,
        "cwd": cwd(home),
        "prompt": "fix the login",
        "hook_event_name": "UserPromptSubmit",
    }))
    .unwrap()
}

fn assert_silent_success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

fn spooled(home: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(home.join("data").join("spool").join("hooks")) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn client(home: &Path) -> Client {
    let discovery = DiscoveryFile::in_dir(&home.join("data"))
        .read()
        .unwrap()
        .unwrap();
    let token = std::fs::read_to_string(home.join("config").join("cli-token")).unwrap();
    Client::new(discovery.address, BUDGET, BUDGET).with_token(Secret::parse(token.trim()).unwrap())
}

fn sessions_seen(home: &Path) -> Vec<String> {
    client(home)
        .hook_events(100)
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.event.session_id)
        .collect()
}

#[test]
fn hooks_reach_a_running_daemon_within_the_budget() {
    let home = tempfile::tempdir().unwrap();
    let _daemon = Daemon::start(home.path());
    let mut durations = Vec::new();
    for n in 0..REPORTS {
        let (output, took) = hook(
            home.path(),
            "user-prompt",
            &payload(home.path(), &format!("s-{n}")),
        );
        // the first prompt names each new session, as legacy answered
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "{\"hookSpecificOutput\":{\"hookEventName\":\"UserPromptSubmit\",\"sessionTitle\":\"fix the login\"}}\n"
        );
        assert!(output.stderr.is_empty(), "{output:?}");
        durations.push(took);
    }
    assert_in_time(&durations);
    assert_eq!(sessions_seen(home.path()).len(), REPORTS);
    assert_eq!(spooled(home.path()), Vec::<PathBuf>::new());
}

#[test]
fn hooks_spool_while_the_daemon_is_down_and_arrive_after_its_start() {
    let home = tempfile::tempdir().unwrap();
    let mut durations = Vec::new();
    for n in 0..REPORTS {
        let (output, took) = hook(
            home.path(),
            "stop",
            &payload(home.path(), &format!("s-{n}")),
        );
        assert_silent_success(&output);
        durations.push(took);
    }
    assert_in_time(&durations);
    assert_eq!(spooled(home.path()).len(), REPORTS);

    let _daemon = Daemon::start(home.path());
    let deadline = Instant::now() + Duration::from_secs(20);
    while !spooled(home.path()).is_empty() {
        assert!(Instant::now() < deadline, "the spool was not taken in");
        thread::sleep(Duration::from_millis(100));
    }
    let mut seen = sessions_seen(home.path());
    seen.sort();
    assert_eq!(
        seen,
        (0..REPORTS).map(|n| format!("s-{n}")).collect::<Vec<_>>()
    );
}

/// A daemon as discovery sees it that accepts connections into its backlog but never answers.
fn silent_daemon(home: &Path) -> TcpListener {
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let discovery = serde_json::json!({
        "pid": std::process::id(),
        "address": silent.local_addr().unwrap(),
        "version": "0.0.0",
        "apiVersion": 1,
        "startedAt": 0,
    });
    std::fs::create_dir_all(home.join("data")).unwrap();
    std::fs::write(home.join("data").join("daemon.json"), discovery.to_string()).unwrap();
    std::fs::create_dir_all(home.join("config")).unwrap();
    std::fs::write(home.join("config").join("cli-token"), "a".repeat(64)).unwrap();
    silent
}

#[test]
fn hooks_give_up_on_a_daemon_that_does_not_answer() {
    let home = tempfile::tempdir().unwrap();
    let silent = silent_daemon(home.path());
    for (n, event) in ["stop", "user-prompt"].into_iter().enumerate() {
        let (output, took) = hook(home.path(), event, &payload(home.path(), &format!("s-{n}")));
        assert_silent_success(&output);
        assert!(took < CEILING, "the {event} hook waited {took:?}");
    }
    assert_eq!(spooled(home.path()).len(), 2);
    drop(silent);
}

#[test]
fn a_hook_short_of_time_spools_without_asking() {
    let home = tempfile::tempdir().unwrap();
    let silent = silent_daemon(home.path());
    let mut short = hook_command(home.path(), &["hook", "user-prompt"]);
    short.env("NULLRADIX_HOOK_DEADLINE_MS", "900");
    let (output, took) = finish(short, Some(&payload(home.path(), "s-1")));
    assert_silent_success(&output);
    assert!(took < Duration::from_millis(900), "took {took:?}");
    assert_eq!(spooled(home.path()).len(), 1);
    drop(silent);
}

#[test]
fn hooks_stay_silent_whatever_they_are_given() {
    let home = tempfile::tempdir().unwrap();
    for (event, input) in [
        ("stop", b"not json".as_slice()),
        ("stop", b"{}".as_slice()),
        ("stop", br#"{"session_id":"s-1"}"#.as_slice()),
        ("no-such-hook", b"{}".as_slice()),
    ] {
        let (output, _) = hook(home.path(), event, input);
        assert_silent_success(&output);
    }
    assert_eq!(spooled(home.path()), Vec::<PathBuf>::new());

    // whatever confuses the regular parser must not fail the hook either
    let home_text = cwd(home.path());
    let flagged = [
        "--profile",
        "no-such-profile",
        "--home",
        &home_text,
        "hook",
        "stop",
        "--no-such-flag",
    ];
    let mut confused = hook_command(home.path(), &flagged);
    confused.env("NULLRADIX_PROFILE", "no-such-profile");
    let (output, _) = finish(confused, Some(&payload(home.path(), "s-1")));
    assert_silent_success(&output);
    assert_eq!(spooled(home.path()).len(), 1);
    let mut bare = hook_command(home.path(), &["hook"]);
    bare.env("NULLRADIX_PROFILE", "no-such-profile");
    let (output, _) = finish(bare, Some(b"{}"));
    assert_silent_success(&output);
}

#[test]
fn a_hook_whose_input_never_ends_still_ends_in_time() {
    let home = tempfile::tempdir().unwrap();
    let (output, took) = finish(hook_command(home.path(), &["hook", "stop"]), None);
    assert_silent_success(&output);
    assert!(took < CEILING, "took {took:?}");

    let mut short = hook_command(home.path(), &["hook", "stop"]);
    short.env("NULLRADIX_HOOK_DEADLINE_MS", "300");
    let (output, took) = finish(short, None);
    assert_silent_success(&output);
    assert!(
        took < Duration::from_millis(900),
        "the watchdog took {took:?}"
    );
}
