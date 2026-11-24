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
