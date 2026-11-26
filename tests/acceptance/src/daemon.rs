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
use ureq::Agent;

/// Generous for a loaded machine; the daemon is waited for, not guessed at.
const STARTUP: Duration = Duration::from_secs(60);
/// Time active runs get to end before the daemon is stopped.
const WIND_DOWN: Duration = Duration::from_secs(30);
/// Sources each binary is built from; a newer file there means it is stale.
const NULLRADIX_SOURCES: &[&str] = &["apps/cli", "crates"];
const FAKE_CLAUDE_SOURCES: &[&str] = &["apps/fake-claude"];
/// Directories that do not go into a binary.
const NOT_BUILT: &[&str] = &["target", "tests", "benches"];

/// Variables a developer's shell may carry that would change what the daemon records.
const SCRUBBED: &[&str] = &[
    "NULLRADIX_PORT",
    "NULLRADIX_BIND",
    "NULLRADIX_LOG",
    "NULLRADIX_WEB_DIR",
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
    "NULLRADIX_CLAUDE",
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
        let found = Self {
            nullradix: dir.join(name),
            fake_claude,
        };
        found.ensure_fresh()?;
        Ok(found)
    }

    /// Refuses binaries older than their sources, which would test or record old behaviour.
    fn ensure_fresh(&self) -> anyhow::Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .context("the acceptance crate is not inside the repository")?;
        for (binary, sources) in [
            (&self.nullradix, NULLRADIX_SOURCES),
            (&self.fake_claude, FAKE_CLAUDE_SOURCES),
        ] {
            let mut newest = None;
            for source in sources {
                newest = newest.max(newest_change(&root.join(source))?);
            }
            let built = fs::metadata(binary)?.modified()?;
            if newest.is_some_and(|changed| changed > built) {
                bail!(
                    "{} is older than its sources; build it with `cargo build -p nullradix-cli -p nullradix-fake-claude`",
                    binary.display()
                );
            }
        }
        Ok(())
    }
}

/// When a file below `dir` last changed, skipping build output.
fn newest_change(dir: &Path) -> anyhow::Result<Option<std::time::SystemTime>> {
    let mut newest = None;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let skipped = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| NOT_BUILT.contains(&name));
        if skipped && entry.file_type()?.is_dir() {
            continue;
        }
        let changed = if entry.file_type()?.is_dir() {
            newest_change(&path)?
        } else {
            Some(entry.metadata()?.modified()?)
        };
        newest = newest.max(changed);
    }
    Ok(newest)
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
        // logins of accounts run the fake Claude unless a scenario names another command
        let mut env = env.clone();
        env.entry(String::from("NULLRADIX_CLAUDE"))
            .or_insert_with(|| binaries.fake_claude.to_string_lossy().into_owned());
        let env = &env;
        let home = tempfile::Builder::new()
            .prefix("nullradix-acceptance")
            .tempdir()?;
        let work = home.path().join("work");
        fs::create_dir_all(&work)?;
        let log = fs::File::create(home.path().join("daemon.stderr"))?;
        let child = command(&binaries.nullradix, home.path(), env)
            .args(["serve", "--port", "0"])
            .stdout(Stdio::null())
            .stderr(log)
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
            if let Some(status) = daemon.child.try_wait()? {
                let stderr = fs::read_to_string(daemon.home.path().join("daemon.stderr"))
                    .unwrap_or_default();
                bail!("the daemon stopped with {status}: {}", stderr.trim());
            }
            if Instant::now() > deadline {
                bail!("the daemon did not start in time");
            }
            thread::sleep(Duration::from_millis(50));
        }
        daemon.wait_until_serving(deadline)?;
        let token = daemon.home.path().join("config").join("cli-token");
        daemon.token = fs::read_to_string(&token)
            .with_context(|| format!("cannot read {}", token.display()))?
            .trim()
            .to_owned();
        Ok(daemon)
    }

    /// Waits until the daemon answers its health check, trying again while the connection fails.
    fn wait_until_serving(&mut self, deadline: Instant) -> anyhow::Result<()> {
        let agent: Agent = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(10)))
            .proxy(None)
            .build()
            .into();
        loop {
            if agent.get(&self.url("/api/v1/health")).call().is_ok() {
                return Ok(());
            }
            if let Some(status) = self.child.try_wait()? {
                bail!("the daemon stopped with {status}");
            }
            if Instant::now() > deadline {
                bail!("the daemon did not answer in time");
            }
            thread::sleep(Duration::from_millis(50));
        }
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

impl Daemon {
    /// Cancels what still runs, since run supervisors are detached and outlive the daemon.
    fn wind_down(&self) {
        let agent: Agent = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .into();
        let bearer = format!("Bearer {}", self.token);
        let active = || -> Vec<String> {
            let Ok(mut response) = agent
                .get(&self.url("/api/v1/runs?limit=500"))
                .header("Authorization", &bearer)
                .call()
            else {
                return Vec::new();
            };
            let list: Value = response.body_mut().read_json().unwrap_or_default();
            list["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|run| matches!(run["status"].as_str(), Some("starting" | "running")))
                .filter_map(|run| run["id"].as_str().map(str::to_owned))
                .collect()
        };
        for id in active() {
            let _ = agent
                .post(&self.url(&format!("/api/v1/runs/{id}/cancel")))
                .header("Authorization", &bearer)
                .send_empty();
        }
        let deadline = Instant::now() + WIND_DOWN;
        while Instant::now() < deadline && !active().is_empty() {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if !self.token.is_empty() {
            self.wind_down();
        }
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
        .env("NULLRADIX_PERSON", "tester")
        // transcripts are read below `projects` of this folder only
        .env("CLAUDE_CONFIG_DIR", home.join("work").join("claude-config"))
        .envs(env);
    command
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}
