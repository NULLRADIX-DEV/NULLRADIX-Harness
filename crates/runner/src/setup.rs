//! Runs the setup commands of a new worktree on this machine.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::dispatch::setup::{SetupFailure, SetupOutcome, SetupRequest, WorktreeSetupPort};
use nullradix_domain::WorktreeSetupStep;
use nullradix_os::ProcessTree;

/// Characters of a step's output that are kept, from the end.
const OUTPUT_KEPT: usize = 20_000;
const POLL: Duration = Duration::from_millis(25);

/// Runs setup commands as child processes of the daemon.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalSetup;

#[async_trait]
impl WorktreeSetupPort for LocalSetup {
    async fn run(&self, request: &SetupRequest) -> Result<SetupOutcome, AppError> {
        let request = request.clone();
        tokio::task::spawn_blocking(move || run_steps(&request))
            .await
            .map_err(AppError::internal)
    }
}

fn run_steps(request: &SetupRequest) -> SetupOutcome {
    let root = PathBuf::from(&request.root);
    for step in &request.steps {
        if let Some(failure) = run_step(&root, step, request) {
            return SetupOutcome::Failed(failure);
        }
    }
    SetupOutcome::Done
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

fn shell(run: &str, git_bash: Option<&str>) -> Command {
    if cfg!(windows) {
        if let Some(bash) = git_bash.filter(|path| Path::new(path).exists()) {
            let mut command = Command::new(bash);
            command.arg("-c").arg(run);
            return command;
        }
        let mut command = Command::new("cmd");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.raw_arg("/d /s /c").raw_arg(run);
        }
        return command;
    }
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(run);
    command
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let count = text.chars().count();
    if count <= OUTPUT_KEPT {
        return text.into_owned();
    }
    text.chars().skip(count - OUTPUT_KEPT).collect()
}

fn run_step(root: &Path, step: &WorktreeSetupStep, request: &SetupRequest) -> Option<SetupFailure> {
    let Some(folder) = folder_in(root, &step.cwd) else {
        let output = format!("The folder {} does not exist in the worktree.", step.cwd);
        return Some(failure(step, None, false, output));
    };
    let mut command = shell(&step.run, request.git_bash.as_deref());
    command
        .current_dir(&folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tree = match ProcessTree::spawn(&mut command) {
        Ok(tree) => tree,
        Err(error) => return Some(failure(step, None, false, error.to_string())),
    };
    let stdout = drain(tree.take_stdout());
    let stderr = drain(tree.take_stderr());
    let started = Instant::now();
    let (code, timed_out) = loop {
        match tree.try_wait() {
            Ok(Some(status)) => break (status.code(), false),
            Ok(None) if started.elapsed() > request.timeout => {
                let _ = tree.kill();
                let _ = tree.wait();
                break (None, true);
            }
            Ok(None) => thread::sleep(POLL),
            Err(error) => return Some(failure(step, None, false, error.to_string())),
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
    (code != Some(0) || timed_out).then(|| failure(step, code, timed_out, output))
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
        let outcome = LocalSetup
            .run(&request(
                temp.path(),
                &[
                    ("web", "echo one> marker.txt"),
                    (".", "echo two> second.txt"),
                ],
                Duration::from_secs(30),
            ))
            .await
            .unwrap();
        assert_eq!(outcome, SetupOutcome::Done);
        assert!(temp.path().join("web/marker.txt").is_file());
        assert!(temp.path().join("second.txt").is_file());
    }

    #[tokio::test]
    async fn the_first_failing_step_stops_the_setup_with_its_output() {
        let temp = tempfile::tempdir().unwrap();
        let outcome = LocalSetup
            .run(&request(
                temp.path(),
                &[(".", failing_command()), (".", "echo never> late.txt")],
                Duration::from_secs(30),
            ))
            .await
            .unwrap();
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
        let outcome = LocalSetup
            .run(&request(
                temp.path(),
                &[("..", "echo x")],
                Duration::from_secs(30),
            ))
            .await
            .unwrap();
        let SetupOutcome::Failed(found) = outcome else {
            panic!("the step must fail");
        };
        assert_eq!(found.code, None);
        assert!(found.output.contains("does not exist"));
    }

    #[tokio::test]
    async fn a_step_that_takes_too_long_is_killed() {
        let temp = tempfile::tempdir().unwrap();
        let outcome = LocalSetup
            .run(&request(
                temp.path(),
                &[(".", sleep_command())],
                Duration::from_millis(300),
            ))
            .await
            .unwrap();
        let SetupOutcome::Failed(found) = outcome else {
            panic!("the step must time out");
        };
        assert!(found.timed_out);
        assert_eq!(found.cause(), "timed out");
    }
}
