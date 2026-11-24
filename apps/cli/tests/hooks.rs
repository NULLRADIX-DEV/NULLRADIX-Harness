#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nullradix_app::auth::Secret;
use nullradix_client::{Client, DiscoveryFile};

mod common;

use common::{Daemon, nullradix};

/// A debug build on a slow CI runner must still answer well inside Claude Code's 5 s.
const BUDGET: Duration = Duration::from_millis(1000);
const REPORTS: usize = 5;

fn cwd(home: &Path) -> String {
    home.to_string_lossy().into_owned()
}

/// Runs `nullradix hook <event>` with `input` on stdin and returns how long it took.
fn hook(home: &Path, event: &str, input: &[u8]) -> (Output, Duration) {
    let started = Instant::now();
    let mut child = nullradix(home)
        .args(["hook", event])
        .env_remove("NULLRADIX_DEBUG")
        .env_remove("CLAUDE_ENV_FILE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    (output, started.elapsed())
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

fn median(mut durations: Vec<Duration>) -> Duration {
    durations.sort();
    durations[durations.len() / 2]
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
        assert_silent_success(&output);
        durations.push(took);
    }
    let typical = median(durations);
    assert!(typical < BUDGET, "a hook took {typical:?}");
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
    let typical = median(durations);
    assert!(typical < BUDGET, "a spooling hook took {typical:?}");
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

#[test]
fn hooks_give_up_on_a_daemon_that_does_not_answer() {
    let home = tempfile::tempdir().unwrap();
    // accepts connections into its backlog but never answers
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let discovery = serde_json::json!({
        "pid": std::process::id(),
        "address": silent.local_addr().unwrap(),
        "version": "0.0.0",
        "apiVersion": 1,
        "startedAt": 0,
    });
    std::fs::create_dir_all(home.path().join("data")).unwrap();
    std::fs::write(
        home.path().join("data").join("daemon.json"),
        discovery.to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(home.path().join("config").join("cli-token"), "a".repeat(64)).unwrap();

    let (output, took) = hook(home.path(), "stop", &payload(home.path(), "s-1"));
    assert_silent_success(&output);
    assert!(
        took < Duration::from_millis(2500),
        "the hook waited {took:?}"
    );
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
}

#[test]
fn a_hook_whose_input_never_ends_still_ends_in_time() {
    let home = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let mut child = nullradix(home.path())
        .args(["hook", "stop"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _open = child.stdin.take();
    let status = child.wait().unwrap();
    assert!(status.success());
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "took {:?}",
        started.elapsed()
    );
}
