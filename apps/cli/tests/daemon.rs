#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::path::Path;
use std::process::Output;
use std::time::{Duration, Instant};

mod common;

use common::{Daemon, nullradix};

fn status(home: &Path) -> Output {
    nullradix(home).arg("status").output().unwrap()
}

#[test]
fn status_follows_the_daemon_and_only_one_daemon_runs_per_profile() {
    let home = tempfile::tempdir().unwrap();
    let before = status(home.path());
    assert!(!before.status.success());
    assert!(String::from_utf8_lossy(&before.stdout).contains("not running"));

    let daemon = Daemon::start(home.path());
    let running = status(home.path());
    assert!(running.status.success());
    assert!(String::from_utf8_lossy(&running.stdout).contains("is running"));

    let second = nullradix(home.path())
        .args(["serve", "--port", "0"])
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already running"));

    drop(daemon);
    let after = status(home.path());
    assert!(!after.status.success());
    assert!(String::from_utf8_lossy(&after.stdout).contains("stale"));
}

#[test]
fn the_daemon_refuses_to_listen_beyond_loopback() {
    let home = tempfile::tempdir().unwrap();
    let output = nullradix(home.path())
        .args(["serve", "--bind", "0.0.0.0", "--port", "0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing to listen"));
}

fn run(home: &Path, args: &[&str]) -> Output {
    nullradix(home).args(args).output().unwrap()
}

#[test]
fn manages_projects_through_the_daemon() {
    let home = tempfile::tempdir().unwrap();
    let _daemon = Daemon::start(home.path());

    let created = run(home.path(), &["project", "create", "ARC", "Arches"]);
    assert!(created.status.success(), "{created:?}");

    let listed = run(home.path(), &["project", "list", "--json"]);
    let projects: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(projects[0]["key"], "ARC");
    assert_eq!(projects[0]["name"], "Arches");

    let renamed = run(home.path(), &["project", "rename", "ARC", "Arches 2"]);
    assert!(renamed.status.success(), "{renamed:?}");
    let table = run(home.path(), &["project", "list"]);
    assert!(String::from_utf8_lossy(&table.stdout).contains("Arches 2"));

    let taken = run(home.path(), &["project", "create", "arc", "Lower"]);
    assert!(!taken.status.success());
    assert!(String::from_utf8_lossy(&taken.stderr).contains("project-exists"));

    let deleted = run(home.path(), &["project", "delete", "ARC"]);
    assert!(deleted.status.success(), "{deleted:?}");
    let missing = run(home.path(), &["project", "delete", "ARC"]);
    assert!(!missing.status.success());
}

fn shell(script: &str) -> Vec<&str> {
    if cfg!(windows) {
        vec!["cmd", "/C", script]
    } else {
        vec!["sh", "-c", script]
    }
}

fn start_run(home: &Path, extra: &[&str], script: &str) -> Output {
    let mut args = vec!["run", "start"];
    args.extend_from_slice(extra);
    args.push("--");
    args.extend(shell(script));
    run(home, &args)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

const LONG: &str = if cfg!(windows) {
    "ping -n 30 127.0.0.1 > NUL"
} else {
    "sleep 30"
};

#[test]
fn runs_commands_and_exits_like_them() {
    let home = tempfile::tempdir().unwrap();
    let _daemon = Daemon::start(home.path());

    let script = if cfg!(windows) {
        "echo hello& exit 3"
    } else {
        "echo hello; exit 3"
    };
    let started = start_run(home.path(), &["--label", "greet"], script);
    assert_eq!(started.status.code(), Some(3), "{started:?}");
    assert_eq!(stdout(&started).trim(), "hello");

    let listed = run(home.path(), &["run", "list", "--json"]);
    let runs: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(runs[0]["label"], "greet");
    assert_eq!(runs[0]["status"], "failed");
    assert_eq!(runs[0]["exitCode"], 3);

    let id = runs[0]["id"].as_str().unwrap();
    let logs = run(home.path(), &["run", "logs", id]);
    assert!(logs.status.success(), "{logs:?}");
    assert_eq!(stdout(&logs).trim(), "hello");

    let missing = run(
        home.path(),
        &["run", "start", "--", "nullradix-no-such-program"],
    );
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("nullradix-no-such-program"),
        "{missing:?}"
    );
}

#[test]
fn cancels_runs_and_stops_them_at_the_timeout() {
    let home = tempfile::tempdir().unwrap();
    let _daemon = Daemon::start(home.path());

    let detached = start_run(home.path(), &["--detach"], LONG);
    assert!(detached.status.success(), "{detached:?}");
    let id = stdout(&detached).trim().to_owned();
    let cancel = run(home.path(), &["run", "cancel", &id]);
    assert!(cancel.status.success(), "{cancel:?}");
    let followed = run(home.path(), &["run", "logs", &id, "--follow"]);
    assert!(!followed.status.success());
    assert!(stderr(&followed).contains("cancelled"), "{followed:?}");
    let again = run(home.path(), &["run", "cancel", &id]);
    assert!(stderr(&again).contains("run-finished"), "{again:?}");

    let begun = Instant::now();
    let timed_out = start_run(home.path(), &["--timeout", "1"], LONG);
    assert!(begun.elapsed() < Duration::from_secs(20));
    assert!(stderr(&timed_out).contains("timed-out"), "{timed_out:?}");
}

#[test]
fn a_run_outlives_a_daemon_restart() {
    // the space checks that the supervisor gets its run dir intact
    let home = tempfile::Builder::new()
        .prefix("nullradix home")
        .tempdir()
        .unwrap();
    let daemon = Daemon::start(home.path());
    let script = if cfg!(windows) {
        "ping -n 4 127.0.0.1 > NUL & echo done"
    } else {
        "sleep 3; echo done"
    };
    let detached = start_run(home.path(), &["--detach"], script);
    assert!(detached.status.success(), "{detached:?}");
    let id = stdout(&detached).trim().to_owned();

    drop(daemon);
    let _daemon = Daemon::start(home.path());
    let followed = run(home.path(), &["run", "logs", &id, "--follow"]);
    assert!(followed.status.success(), "{followed:?}");
    assert_eq!(stdout(&followed).trim(), "done");
}
