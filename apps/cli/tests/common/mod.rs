#![allow(
    dead_code,
    clippy::unwrap_used,
    reason = "shared by test files that each use a part; tests fail loudly"
)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const STARTUP: Duration = Duration::from_secs(30);

pub(crate) fn nullradix(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nullradix"));
    command
        .env("NULLRADIX_HOME", home)
        .env("NULLRADIX_PROFILE", "dev")
        .env_remove("NULLRADIX_PORT")
        .env_remove("NULLRADIX_BIND")
        .env_remove("NULLRADIX_DEBUG")
        .env_remove("NULLRADIX_HOOK_DEADLINE_MS")
        .env_remove("NULLRADIX_TICKET")
        .env_remove("NULLRADIX_RUN")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .env_remove("CLAUDE_ENV_FILE");
    command
}

pub(crate) struct Daemon(Child);

impl Daemon {
    pub(crate) fn start(home: &Path) -> Self {
        let child = nullradix(home)
            .args(["serve", "--port", "0"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let daemon = Self(child);
        let discovery = home.join("data").join("daemon.json");
        let deadline = Instant::now() + STARTUP;
        // a killed daemon leaves its file behind, so wait for ours
        while discovered_pid(&discovery) != Some(pid) {
            assert!(Instant::now() < deadline, "daemon did not start in time");
            thread::sleep(Duration::from_millis(50));
        }
        daemon
    }
}

fn discovered_pid(path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    let record: serde_json::Value = serde_json::from_str(&text).ok()?;
    record["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
