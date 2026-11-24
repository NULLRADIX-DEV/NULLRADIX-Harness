#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const STARTUP: Duration = Duration::from_secs(30);

fn nullradix(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nullradix"));
    command
        .env("NULLRADIX_HOME", home)
        .env("NULLRADIX_PROFILE", "dev")
        .env_remove("NULLRADIX_PORT")
        .env_remove("NULLRADIX_BIND");
    command
}

fn status(home: &Path) -> Output {
    nullradix(home).arg("status").output().unwrap()
}

struct Daemon(Child);

impl Daemon {
    fn start(home: &Path) -> Self {
        let child = nullradix(home)
            .args(["serve", "--port", "0"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let daemon = Self(child);
        let discovery = home.join("data").join("daemon.json");
        let deadline = Instant::now() + STARTUP;
        while !discovery.exists() {
            assert!(Instant::now() < deadline, "daemon did not start in time");
            thread::sleep(Duration::from_millis(50));
        }
        daemon
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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

    let invalid = run(home.path(), &["project", "create", "arc", "Lower"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid-project-key"));

    let deleted = run(home.path(), &["project", "delete", "ARC"]);
    assert!(deleted.status.success(), "{deleted:?}");
    let missing = run(home.path(), &["project", "delete", "ARC"]);
    assert!(!missing.status.success());
}
