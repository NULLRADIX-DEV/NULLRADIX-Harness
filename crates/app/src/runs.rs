//! Processes the daemon runs on a runner and watches until they finish.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::{Id, Run, RunEnd, RunError, RunLabel, RunOutcome, Version};
use strum::{EnumString, IntoStaticStr};

use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

/// What to execute; the runner owns how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub timeout: Option<Duration>,
}

impl RunSpec {
    /// Checks what holds on any runner; the runner checks its own machine.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.program.trim().is_empty() {
            return Err(AppError::invalid(
                "invalid-run-program",
                "the program is empty",
            ));
        }
        if !self.cwd.is_absolute() {
            return Err(AppError::invalid(
                "invalid-run-cwd",
                format!("{} is not an absolute path", self.cwd.display()),
            ));
        }
        if self.timeout.is_some_and(|timeout| timeout.is_zero()) {
            return Err(AppError::invalid(
                "invalid-run-timeout",
                "the timeout must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLine {
    pub seq: i64,
    pub stream: OutputStream,
    pub text: String,
}

/// The final word of the runner about a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub outcome: RunOutcome,
    pub end: RunEnd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunProgress {
    /// Output after the requested sequence number, oldest first.
    pub lines: Vec<OutputLine>,
    pub report: Option<RunReport>,
    /// Whether the process supervising the run still exists.
    pub alive: bool,
}

#[async_trait]
pub trait RunnerPort: Send + Sync {
    /// Starts the run detached and returns the id of its supervising process.
    async fn start(&self, run: Id<Run>, spec: &RunSpec) -> Result<u32, AppError>;
    /// Asks the run to stop; the report arrives later through `progress`.
    async fn cancel(&self, run: Id<Run>) -> Result<(), AppError>;
    async fn progress(&self, run: Id<Run>, after: i64, limit: u32)
    -> Result<RunProgress, AppError>;
}

#[async_trait]
pub trait RunRepository: Send + Sync {
    async fn insert(&self, run: &Run) -> Result<Change, AppError>;
    async fn update(&self, run: &Run, expected: Version) -> Result<Change, AppError>;
    async fn get(&self, id: Id<Run>) -> Result<Option<Run>, AppError>;
    /// Newest first.
    async fn list(&self, limit: u32) -> Result<Vec<Run>, AppError>;
    async fn active(&self) -> Result<Vec<Run>, AppError>;
}

#[must_use]
pub fn not_found(id: Id<Run>) -> AppError {
    AppError::not_found("run-not-found", format!("run {id} does not exist"))
}

fn transition(error: &RunError) -> AppError {
    match error {
        RunError::InvalidLabel => AppError::invalid("invalid-run-label", error.to_string()),
        RunError::InvalidTransition { .. } => AppError::conflict("run-finished", error.to_string()),
    }
}

pub struct RunService {
    repository: Arc<dyn RunRepository>,
    runner: Arc<dyn RunnerPort>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
    poll: Duration,
}

impl std::fmt::Debug for RunService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunService").finish_non_exhaustive()
    }
}

impl RunService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn RunRepository>,
        runner: Arc<dyn RunnerPort>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
        poll: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            repository,
            runner,
            clock,
            ids,
            signal,
            poll,
        })
    }

    /// Records the run first, so a crash during the start leaves a trace instead of an orphan.
    pub async fn start(self: &Arc<Self>, label: &str, spec: &RunSpec) -> Result<Run, AppError> {
        let label = RunLabel::parse(label).map_err(|error| transition(&error))?;
        spec.validate()?;
        let run = Run::new(self.ids.as_ref().next(), label, self.clock.now());
        self.save_new(&run).await?;
        match self.runner.start(run.id(), spec).await {
            Ok(pid) => {
                let running = run
                    .started(pid, self.clock.now())
                    .map_err(|error| transition(&error))?;
                self.save(&running, run.version()).await?;
                self.watch(running.id());
                Ok(running)
            }
            Err(error) => {
                let failed = run
                    .failed_to_start(error.message().to_owned(), self.clock.now())
                    .map_err(|error| transition(&error))?;
                self.save(&failed, run.version()).await?;
                Err(error)
            }
        }
    }

    pub async fn get(&self, id: Id<Run>) -> Result<Run, AppError> {
        self.repository.get(id).await?.ok_or_else(|| not_found(id))
    }

    pub async fn list(&self, limit: u32) -> Result<Vec<Run>, AppError> {
        self.repository.list(limit).await
    }

    pub async fn cancel(&self, id: Id<Run>) -> Result<Run, AppError> {
        let run = self.get(id).await?;
        if !run.status().is_active() {
            return Err(AppError::conflict(
                "run-finished",
                format!("run {id} has already finished"),
            ));
        }
        self.runner.cancel(id).await?;
        Ok(run)
    }

    pub async fn output(
        &self,
        id: Id<Run>,
        after: i64,
        limit: u32,
    ) -> Result<RunProgress, AppError> {
        self.get(id).await?;
        self.runner.progress(id, after, limit).await
    }

    /// Watches runs that were active when the daemon stopped.
    pub async fn recover(self: &Arc<Self>) -> Result<usize, AppError> {
        let active = self.repository.active().await?;
        let count = active.len();
        for run in active {
            if run.pid().is_some() {
                self.watch(run.id());
            } else {
                let lost = run
                    .lost(self.clock.now())
                    .map_err(|error| transition(&error))?;
                self.save(&lost, run.version()).await?;
            }
        }
        Ok(count)
    }

    fn watch(self: &Arc<Self>, id: Id<Run>) {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(service.poll).await;
                match service.settle(id).await {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(error) => tracing::warn!(run = %id, error = %error, "cannot check run"),
                }
            }
        });
    }

    /// Records the end of a run once the runner reports it; true when nothing is left to watch.
    async fn settle(&self, id: Id<Run>) -> Result<bool, AppError> {
        let run = self.get(id).await?;
        if !run.status().is_active() {
            return Ok(true);
        }
        let progress = self.runner.progress(id, i64::MAX, 0).await?;
        let report = match (progress.report, progress.alive) {
            (Some(report), _) => Some(report),
            (None, true) => return Ok(false),
            // the supervisor writes the report just before it exits, so look once more
            (None, false) => self.runner.progress(id, i64::MAX, 0).await?.report,
        };
        let next = match report {
            Some(report) => run.finished(report.outcome, report.end, self.clock.now()),
            None => run.lost(self.clock.now()),
        }
        .map_err(|error| transition(&error))?;
        self.save(&next, run.version()).await?;
        Ok(true)
    }

    async fn save_new(&self, run: &Run) -> Result<(), AppError> {
        let change = self.repository.insert(run).await?;
        self.signal.notify(&change);
        Ok(())
    }

    async fn save(&self, run: &Run, expected: Version) -> Result<(), AppError> {
        let change = self.repository.update(run, expected).await?;
        self.signal.notify(&change);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_domain_errors_to_stable_codes() {
        assert_eq!(
            transition(&RunError::InvalidLabel).code(),
            "invalid-run-label"
        );
        let finished = RunError::InvalidTransition {
            from: "failed",
            to: "lost",
        };
        assert_eq!(transition(&finished).code(), "run-finished");
    }

    #[test]
    fn specs_need_a_program_an_absolute_cwd_and_a_positive_timeout() {
        let valid = RunSpec {
            program: String::from("echo"),
            args: Vec::new(),
            cwd: std::env::temp_dir(),
            timeout: Some(Duration::from_secs(1)),
        };
        assert!(valid.validate().is_ok());
        let cases = [
            (
                RunSpec {
                    program: String::from(" "),
                    ..valid.clone()
                },
                "invalid-run-program",
            ),
            (
                RunSpec {
                    cwd: PathBuf::from("relative"),
                    ..valid.clone()
                },
                "invalid-run-cwd",
            ),
            (
                RunSpec {
                    timeout: Some(Duration::ZERO),
                    ..valid.clone()
                },
                "invalid-run-timeout",
            ),
        ];
        for (spec, code) in cases {
            assert_eq!(spec.validate().unwrap_err().code(), code);
        }
    }

    #[test]
    fn output_streams_have_wire_names() {
        assert_eq!(<&str>::from(OutputStream::Stderr), "stderr");
        assert_eq!("stdout".parse::<OutputStream>(), Ok(OutputStream::Stdout));
    }
}
