use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde_json::Value;
use tempfile::TempDir;

const STARTUP: Duration = Duration::from_secs(30);

/// Variables a developer's shell may carry that would change what the daemon records.
const SCRUBBED: &[&str] = &[
    "NULLRADIX_PORT",
    "NULLRADIX_BIND",
    "NULLRADIX_DEBUG",
    "NULLRADIX_TICKET",
    "NULLRADIX_RUN",
    "NULLRADIX_HOOK_DEADLINE_MS",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_ENV_FILE",
    "ANTHROPIC_API_KEY",
    "FAKE_CLAUDE_MODE",
    "FAKE_CLAUDE_REPLY",
    "FAKE_CLAUDE_REPLY_FILE",
];

/// The programs under test, found in the target directory above the test binary.
#[derive(Debug, Clone)]
pub struct Binaries {
    pub(crate) nullradix: PathBuf,
    pub(crate) fake_claude: PathBuf,
}

impl Binaries {
    pub fn locate() -> anyhow::Result<Self> {
        let test = env::current_exe().context("cannot locate the test binary")?;
        let name = format!("nullradix{}", env::consts::EXE_SUFFIX);
        let Some(dir) = test.ancestors().find(|dir| dir.join(&name).is_file()) else {
            bail!(
                "no {name} above {}; build it with `cargo build -p nullradix-cli -p nullradix-fake-claude`",
                test.display()
            );
        };
        let fake_claude = dir.join(format!("nullradix-fake-claude{}", env::consts::EXE_SUFFIX));
        if !fake_claude.is_file() {
            bail!(
                "{} is missing; build it with `cargo build -p nullradix-fake-claude`",
                fake_claude.display()
            );
        }
        Ok(Self {
            nullradix: dir.join(name),
            fake_claude,
        })
    }
}

/// A daemon with its own home and work directory, stopped when dropped.
#[derive(Debug)]
pub(crate) struct Daemon {
    child: Child,
    pub(crate) address: SocketAddr,
    pub(crate) token: String,
    pub(crate) home: TempDir,
    pub(crate) work: PathBuf,
    env: BTreeMap<String, String>,
    nullradix: PathBuf,
}

impl Daemon {
    pub(crate) fn start(
        binaries: &Binaries,
        env: &BTreeMap<String, String>,
    ) -> anyhow::Result<Self> {
        let home = tempfile::Builder::new()
            .prefix("nullradix-acceptance")
            .tempdir()?;
        let work = home.path().join("work");
        fs::create_dir_all(&work)?;
        let child = command(&binaries.nullradix, home.path(), env)
            .args(["serve", "--port", "0"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("cannot start the daemon")?;
        let pid = child.id();
        let mut daemon = Self {
            child,
            address: SocketAddr::from(([127, 0, 0, 1], 0)),
            token: String::new(),
            home,
            work,
            env: env.clone(),
            nullradix: binaries.nullradix.clone(),
        };
        let discovery = daemon.home.path().join("data").join("daemon.json");
        let deadline = Instant::now() + STARTUP;
        loop {
            if let Some(record) = read_json(&discovery)
                && record["pid"].as_u64() == Some(u64::from(pid))
            {
                daemon.address = record["address"]
                    .as_str()
                    .context("discovery without address")?
                    .parse()?;
                break;
            }
            if Instant::now() > deadline {
                bail!("the daemon did not start in time");
            }
            thread::sleep(Duration::from_millis(50));
        }
        let token = daemon.home.path().join("config").join("cli-token");
        daemon.token = fs::read_to_string(&token)
            .with_context(|| format!("cannot read {}", token.display()))?
            .trim()
            .to_owned();
        Ok(daemon)
    }

    /// The CLI of the same profile and home as this daemon.
    #[must_use]
    pub(crate) fn cli(&self) -> Command {
        command(&self.nullradix, self.home.path(), &self.env)
    }

    #[must_use]
    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn command(program: &Path, home: &Path, env: &BTreeMap<String, String>) -> Command {
    let mut command = Command::new(program);
    for name in SCRUBBED {
        command.env_remove(name);
    }
    command
        .env("NULLRADIX_HOME", home)
        .env("NULLRADIX_PROFILE", "dev")
        .envs(env);
    command
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}
