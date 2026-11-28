//! Runs the setup commands of a new worktree on this machine.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::dispatch::setup::{SetupFailure, SetupOutcome, SetupRequest, WorktreeSetupPort};
use nullradix_domain::{NetworkShares, WorktreeSetupStep};
use nullradix_os::ProcessTree;
use tokio_util::sync::CancellationToken;

/// Characters of a step's output that are kept, from the end.
const OUTPUT_KEPT: usize = 20_000;
const POLL: Duration = Duration::from_millis(25);

/// Runs setup commands as child processes of the daemon.
#[derive(Debug, Clone, Default)]
pub struct LocalSetup {
    shares: NetworkShares,
}

impl LocalSetup {
    /// Lets worktrees on these hosts be used.
    #[must_use]
    pub fn with_network_shares(mut self, shares: NetworkShares) -> Self {
        self.shares = shares;
        self
    }
}

#[async_trait]
impl WorktreeSetupPort for LocalSetup {
    async fn run(
        &self,
        request: &SetupRequest,
        cancel: &CancellationToken,
    ) -> Result<SetupOutcome, AppError> {
        crate::git::refuse_networks(&self.shares, &[&request.root])?;
        let request = request.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || run_steps(&request, &cancel))
            .await
            .map_err(AppError::internal)
    }
}

fn run_steps(request: &SetupRequest, cancel: &CancellationToken) -> SetupOutcome {
    let root = PathBuf::from(&request.root);
    for step in &request.steps {
        if cancel.is_cancelled() {
            return SetupOutcome::Cancelled;
        }
        match run_step(&root, step, request, cancel) {
            StepEnd::Done => {}
            StepEnd::Cancelled => return SetupOutcome::Cancelled,
            StepEnd::Failed(failure) => return SetupOutcome::Failed(failure),
        }
    }
    SetupOutcome::Done
}

enum StepEnd {
    Done,
    Cancelled,
    Failed(SetupFailure),
}

fn failure(
    step: &WorktreeSetupStep,
    code: Option<i32>,
    timed_out: bool,
    output: String,
) -> SetupFailure {
    SetupFailure {
        cwd: step.cwd.clone(),
        run: step.run.clone(),
        code,
        timed_out,
        output,
    }
}

/// The folder of a step when it lies inside the worktree.
fn folder_in(root: &Path, relative: &str) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let folder = root.join(relative).canonicalize().ok()?;
    (folder.starts_with(&root) && folder.is_dir()).then_some(folder)
}

/// The configured Git Bash, which must exist when one is configured.
#[cfg_attr(not(windows), allow(dead_code, reason = "only Windows runs Git Bash"))]
fn configured_bash(git_bash: Option<&str>) -> Result<Option<&str>, String> {
    match git_bash {
        Some(path) if !Path::new(path).exists() => {
            Err(format!("The configured Git Bash {path} does not exist."))
        }
        other => Ok(other),
    }
}

#[cfg(windows)]
fn shell(run: &str, git_bash: Option<&str>) -> Result<Command, String> {
    use std::os::windows::process::CommandExt;
    if let Some(bash) = configured_bash(git_bash)? {
        let mut command = Command::new(bash);
        command.arg("-c").arg(run);
        return Ok(command);
    }
    let mut command = Command::new("cmd");
    command
        .args(["/d", "/s", "/c"])
        .raw_arg(format!("\"{run}\""));
    Ok(command)
}

#[cfg(not(windows))]
#[allow(clippy::unnecessary_wraps, reason = "same shape as the Windows shell")]
fn shell(run: &str, _git_bash: Option<&str>) -> Result<Command, String> {
    let mut command = ProcessTree::command("/bin/sh");
    command.arg("-c").arg(run);
    Ok(command)
}

/// Reads a pipe to its end and keeps only the last `OUTPUT_KEPT` bytes.
fn drain(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        if let Some(mut pipe) = pipe {
            let mut chunk = [0_u8; 8192];
            while let Ok(read) = pipe.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                kept.extend_from_slice(&chunk[..read]);
                if kept.len() > OUTPUT_KEPT {
                    kept.drain(..kept.len() - OUTPUT_KEPT);
                }
            }
        }
        kept
    })
}

fn tail(bytes: &[u8]) -> String {
    // a cut in the middle of a character leaves continuation bytes first
    let start = bytes
        .iter()
        .position(|byte| byte & 0xC0 != 0x80)
        .unwrap_or(bytes.len());
    let text = String::from_utf8_lossy(&bytes[start..]);
    let count = text.chars().count();
    if count <= OUTPUT_KEPT {
        return text.into_owned();
    }
    text.chars().skip(count - OUTPUT_KEPT).collect()
}

fn run_step(
    root: &Path,
    step: &WorktreeSetupStep,
    request: &SetupRequest,
    cancel: &CancellationToken,
) -> StepEnd {
    let Some(folder) = folder_in(root, &step.cwd) else {
        let output = format!("The folder {} does not exist in the worktree.", step.cwd);
        return StepEnd::Failed(failure(step, None, false, output));
    };
    let mut command = match shell(&step.run, request.git_bash.as_deref()) {
        Ok(command) => command,
        Err(message) => return StepEnd::Failed(failure(step, None, false, message)),
    };
    command
        .current_dir(&folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tree = match ProcessTree::spawn(&mut command) {
        Ok(tree) => tree,
        Err(error) => return StepEnd::Failed(failure(step, None, false, error.to_string())),
    };
    let stdout = drain(tree.take_stdout());
    let stderr = drain(tree.take_stderr());
    let started = Instant::now();
    let (code, timed_out) = loop {
        match tree.try_wait() {
            Ok(Some(status)) => break (status.code(), false),
            Ok(None) if cancel.is_cancelled() => {
                let _ = tree.kill();
                let _ = tree.wait();
                return StepEnd::Cancelled;
            }
            Ok(None) if started.elapsed() > request.timeout => {
                let _ = tree.kill();
                let _ = tree.wait();
                break (None, true);
            }
            Ok(None) => thread::sleep(POLL),
            Err(error) => {
                return StepEnd::Failed(failure(step, None, false, error.to_string()));
            }
        }
    };
    let mut output = tail(&stdout.join().unwrap_or_default());
    let errors = tail(&stderr.join().unwrap_or_default());
    if !errors.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&errors);
    }
    if code != Some(0) || timed_out {
        return StepEnd::Failed(failure(step, code, timed_out, output));
    }
    StepEnd::Done
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(root: &Path, steps: &[(&str, &str)], timeout: Duration) -> SetupRequest {
        SetupRequest {
            root: root.to_string_lossy().into_owned(),
            steps: steps
                .iter()
                .map(|(cwd, run)| WorktreeSetupStep {
                    cwd: (*cwd).to_owned(),
                    run: (*run).to_owned(),
                })
                .collect(),
            timeout,
            git_bash: None,
        }
    }

    async fn run(request: &SetupRequest) -> SetupOutcome {
        LocalSetup::default()
            .run(request, &CancellationToken::new())
            .await
            .unwrap()
    }

    fn failing_command() -> &'static str {
        if cfg!(windows) {
            "echo broken & exit 3"
        } else {
            "echo broken; exit 3"
        }
    }

    fn sleep_command() -> &'static str {
        if cfg!(windows) {
            "ping -n 30 127.0.0.1 > nul"
        } else {
            "sleep 30"
        }
    }

    #[tokio::test]
    async fn steps_run_in_their_folder_one_after_the_other() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("web")).unwrap();
        let outcome = run(&request(
            temp.path(),
            &[
                ("web", "echo one> marker.txt"),
                (".", "echo two> second.txt"),
            ],
            Duration::from_secs(30),
        ))
        .await;
        assert_eq!(outcome, SetupOutcome::Done);
        assert!(temp.path().join("web/marker.txt").is_file());
        assert!(temp.path().join("second.txt").is_file());
    }

    #[tokio::test]
    async fn the_first_failing_step_stops_the_setup_with_its_output() {
        let temp = tempfile::tempdir().unwrap();
        let outcome = run(&request(
            temp.path(),
            &[(".", failing_command()), (".", "echo never> late.txt")],
            Duration::from_secs(30),
        ))
        .await;
        let SetupOutcome::Failed(found) = outcome else {
            panic!("the step must fail");
        };
        assert_eq!(found.code, Some(3));
        assert!(found.output.contains("broken"));
        assert!(!temp.path().join("late.txt").exists());
    }

    #[tokio::test]
    async fn a_folder_outside_the_worktree_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let outcome = run(&request(
            temp.path(),
            &[("..", "echo x")],
            Duration::from_secs(30),
        ))
        .await;
        let SetupOutcome::Failed(found) = outcome else {
            panic!("the step must fail");
        };
        assert_eq!(found.code, None);
        assert!(found.output.contains("does not exist"));
    }

    #[tokio::test]
    async fn a_step_that_takes_too_long_is_killed() {
        let temp = tempfile::tempdir().unwrap();
        let outcome = run(&request(
            temp.path(),
            &[(".", sleep_command())],
            Duration::from_millis(300),
        ))
        .await;
        let SetupOutcome::Failed(found) = outcome else {
            panic!("the step must time out");
        };
        assert!(found.timed_out);
        assert_eq!(found.cause(), "timed out");
    }

    #[tokio::test]
    async fn a_command_that_starts_with_a_quote_runs_whole() {
        let temp = tempfile::tempdir().unwrap();
        let line = if cfg!(windows) {
            "\"cmd\" /c echo hi>\"two words.txt\""
        } else {
            "\"echo\" hi>\"two words.txt\""
        };
        let outcome = run(&request(
            temp.path(),
            &[(".", line)],
            Duration::from_secs(30),
        ))
        .await;
        assert_eq!(outcome, SetupOutcome::Done);
        assert!(temp.path().join("two words.txt").is_file());
    }

    #[tokio::test]
    async fn only_the_end_of_a_long_output_is_kept() {
        let temp = tempfile::tempdir().unwrap();
        let line = if cfg!(windows) {
            "(for /L %i in (1,1,6000) do @echo line number %i padding padding padding) & exit 1"
        } else {
            "i=1; while [ $i -le 6000 ]; do echo line number $i padding padding padding; i=$((i+1)); done; exit 1"
        };
        let outcome = run(&request(
            temp.path(),
            &[(".", line)],
            Duration::from_secs(60),
        ))
        .await;
        let SetupOutcome::Failed(found) = outcome else {
            panic!("the step must fail");
        };
        assert!(found.output.len() <= OUTPUT_KEPT);
        assert!(found.output.contains("line number 6000"));
        assert!(!found.output.contains("line number 1 "));
    }

    #[test]
    fn a_configured_git_bash_that_is_missing_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let gone = temp.path().join("no-bash.exe");
        let gone = gone.to_string_lossy().into_owned();
        let error = configured_bash(Some(&gone)).unwrap_err();
        assert!(error.contains("does not exist"), "{error}");
        assert_eq!(configured_bash(None), Ok(None));
        let present = temp.path().to_string_lossy().into_owned();
        assert_eq!(configured_bash(Some(&present)), Ok(Some(present.as_str())));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_missing_git_bash_fails_the_step_instead_of_using_cmd() {
        let temp = tempfile::tempdir().unwrap();
        let mut asked = request(temp.path(), &[(".", "echo x")], Duration::from_secs(30));
        asked.git_bash = Some(
            temp.path()
                .join("no-bash.exe")
                .to_string_lossy()
                .into_owned(),
        );
        let SetupOutcome::Failed(found) = run(&asked).await else {
            panic!("the step must fail");
        };
        assert_eq!(found.code, None);
        assert!(found.output.contains("Git Bash"));
    }

    fn background_command() -> &'static str {
        if cfg!(windows) {
            "start /b ping -n 30 127.0.0.1 > nul"
        } else {
            "sleep 30 &"
        }
    }

    #[tokio::test]
    async fn a_background_child_does_not_hold_the_step_open() {
        let temp = tempfile::tempdir().unwrap();
        let asked = request(
            temp.path(),
            &[(".", background_command())],
            Duration::from_secs(120),
        );
        let outcome = tokio::time::timeout(Duration::from_secs(15), run(&asked))
            .await
            .expect("the step must end once its main process ended");
        assert_eq!(outcome, SetupOutcome::Done);
    }

    #[test]
    fn a_cut_in_the_middle_of_a_character_does_not_leave_a_replacement() {
        let mut bytes = "\u{e9}".as_bytes()[1..].to_vec();
        bytes.extend_from_slice(b"rest");
        assert_eq!(tail(&bytes), "rest");
    }

    #[test]
    fn a_missing_git_bash_error_names_the_path() {
        let error = configured_bash(Some("/no/such/bash")).unwrap_err();
        assert!(error.contains("/no/such/bash"), "{error}");
    }

    #[tokio::test]
    async fn cancelling_kills_the_running_step() {
        let temp = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let asked = request(
            temp.path(),
            &[(".", sleep_command())],
            Duration::from_secs(120),
        );
        let task = tokio::spawn({
            let cancel = cancel.clone();
            async move { LocalSetup::default().run(&asked, &cancel).await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();
        assert_eq!(task.await.unwrap(), SetupOutcome::Cancelled);
    }

    #[tokio::test]
    async fn a_worktree_on_an_unlisted_share_gets_no_setup_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut share = request(dir.path(), &[(".", "echo x")], Duration::from_secs(5));
        share.root = String::from("//srv/share/arches-worktrees/ARC-0001");
        let error = LocalSetup::default()
            .run(&share, &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code(), "network-path-refused");
    }
}
