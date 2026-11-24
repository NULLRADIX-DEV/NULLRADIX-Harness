use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::runs::{RunProgress, RunSpec, RunnerPort};
use nullradix_domain::{Id, Run};
use nullradix_os::{spawn_detached, write_atomic};

use crate::protocol::{RunFiles, SpecFile, read_output, read_report, supervisor_alive};

/// How the runner starts the supervisor: a program plus leading arguments, the run dir follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Runs processes on this machine, each under a detached supervisor.
#[derive(Debug, Clone)]
pub struct LocalRunner {
    runs: PathBuf,
    supervisor: SupervisorCommand,
}

fn unavailable(error: &io::Error) -> AppError {
    AppError::unavailable("runner-failed", format!("the runner failed: {error}"))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(AppError::internal)?
        .map_err(|error| unavailable(&error))
}

impl LocalRunner {
    #[must_use]
    pub fn new(runs: PathBuf, supervisor: SupervisorCommand) -> Self {
        Self { runs, supervisor }
    }

    fn files(&self, run: Id<Run>) -> RunFiles {
        RunFiles::at(&self.runs.join(run.to_string()))
    }
}

fn validate(spec: &RunSpec) -> Result<(), AppError> {
    spec.validate()?;
    if !spec.cwd.is_dir() {
        return Err(AppError::invalid(
            "invalid-run-cwd",
            format!("{} is not a directory", spec.cwd.display()),
        ));
    }
    Ok(())
}

#[async_trait]
impl RunnerPort for LocalRunner {
    async fn check(&self, spec: &RunSpec) -> Result<(), AppError> {
        validate(spec)
    }

    async fn start(&self, run: Id<Run>, spec: &RunSpec) -> Result<u32, AppError> {
        validate(spec)?;
        let files = self.files(run);
        let file = SpecFile {
            program: spec.program.clone(),
            args: spec.args.clone(),
            cwd: spec.cwd.clone(),
            timeout_ms: spec
                .timeout
                .map(|timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)),
        };
        let supervisor = self.supervisor.clone();
        blocking(move || {
            fs::create_dir_all(files.dir())?;
            let json = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
            write_atomic(&files.spec(), &json)?;
            let mut args: Vec<OsString> = supervisor.args.iter().map(OsString::from).collect();
            args.push(files.dir().as_os_str().to_owned());
            let pid = spawn_detached(&supervisor.program, &args, files.dir())?;
            if let Err(error) = write_atomic(&files.supervisor(), pid.to_string().as_bytes()) {
                // the start counts as failed, so the command must not run unseen
                let _ = write_atomic(&files.cancel(), b"");
                return Err(error);
            }
            Ok(pid)
        })
        .await
    }

    async fn cancel(&self, run: Id<Run>) -> Result<(), AppError> {
        let files = self.files(run);
        blocking(move || write_atomic(&files.cancel(), b"")).await
    }

    async fn progress(
        &self,
        run: Id<Run>,
        after: i64,
        limit: u32,
    ) -> Result<RunProgress, AppError> {
        let files = self.files(run);
        blocking(move || {
            // the report comes first: once it exists, all output is already written
            let report = read_report(&files)?;
            let lines = if limit == 0 {
                Vec::new()
            } else {
                read_output(&files, after, limit)?
            };
            let alive = supervisor_alive(&files)?;
            Ok(RunProgress {
                lines,
                report,
                alive,
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use nullradix_app::ErrorKind;

    use super::*;

    fn runner(dir: &std::path::Path) -> LocalRunner {
        LocalRunner::new(
            dir.join("runs"),
            SupervisorCommand {
                program: PathBuf::from("nullradix-missing-supervisor"),
                args: Vec::new(),
            },
        )
    }

    fn spec(cwd: PathBuf) -> RunSpec {
        RunSpec {
            program: String::from("echo"),
            args: Vec::new(),
            cwd,
            timeout: Some(Duration::from_secs(1)),
        }
    }

    #[tokio::test]
    async fn rejects_bad_specs_before_touching_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let runner = runner(dir.path());
        let id = Id::from_uuid(uuid::Uuid::from_u128(1));
        let mut empty = spec(dir.path().to_path_buf());
        empty.program = String::from(" ");
        assert_eq!(
            runner.start(id, &empty).await.unwrap_err().code(),
            "invalid-run-program"
        );
        let missing = spec(dir.path().join("nope"));
        assert_eq!(
            runner.check(&missing).await.unwrap_err().code(),
            "invalid-run-cwd"
        );
        assert_eq!(
            runner.start(id, &missing).await.unwrap_err().code(),
            "invalid-run-cwd"
        );
        assert!(!dir.path().join("runs").exists());
    }

    #[tokio::test]
    async fn a_supervisor_that_cannot_start_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let runner = runner(dir.path());
        let error = runner
            .start(
                Id::from_uuid(uuid::Uuid::from_u128(2)),
                &spec(dir.path().to_path_buf()),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Unavailable);
    }

    #[tokio::test]
    async fn progress_of_an_unknown_run_is_empty_and_dead() {
        let dir = tempfile::tempdir().unwrap();
        let progress = runner(dir.path())
            .progress(Id::from_uuid(uuid::Uuid::from_u128(3)), 0, 10)
            .await
            .unwrap();
        assert_eq!(progress.lines, Vec::new());
        assert_eq!(progress.report, None);
        assert!(!progress.alive);
    }
}
