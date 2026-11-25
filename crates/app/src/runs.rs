//! Processes the daemon runs on a runner and watches until they finish.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::{
    DeniedTool, DispatchRun, Id, Run, RunEnd, RunError, RunKind, RunLabel, RunOutcome, RunResult,
    Screenshot, Timestamp, Version,
};
use strum::{EnumString, IntoStaticStr};
use uuid::Uuid;

use crate::dispatch::stream::{ClaudeOutput, Ending, classify};

use crate::repositories::Located;
use crate::session_detail::{
    ChangeRequest, DiffRequest, FileDiff, SessionChanges, TranscriptChunk, TranscriptRead,
    TranscriptSource, TranscriptSummary,
};
use crate::worktrees::{
    CreatedWorktree, EnsureWorktree, EnsuredWorktree, NewWorktree, RemoveWorktree, WorktreeRemoval,
};
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

/// Changes to the environment a run inherits from the runner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunEnv {
    pub set: BTreeMap<String, String>,
    pub remove: Vec<String>,
    /// Set only when the runner's environment has no value of its own.
    pub defaults: BTreeMap<String, String>,
}

/// What to execute; the runner owns how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub timeout: Option<Duration>,
    pub env: RunEnv,
    /// Written to the program's input, which is closed afterwards.
    pub stdin: Option<String>,
    /// The program speaks Claude's stream-json; the runner reads usage and screenshots from it.
    pub claude: bool,
}

impl RunSpec {
    /// Checks what holds on any runner; the runner checks its own machine.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.program.trim().is_empty() {
            return Err(AppError::invalid(
                "run-program-invalid",
                "the program is empty",
            ));
        }
        if !self.cwd.is_absolute() {
            return Err(AppError::invalid(
                "run-cwd-invalid",
                format!("{} is not an absolute path", self.cwd.display()),
            ));
        }
        if self.timeout.is_some_and(|timeout| timeout.is_zero()) {
            return Err(AppError::invalid(
                "run-timeout-invalid",
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
    /// The line goes on in the next piece, as it was too long for one.
    pub continues: bool,
}

/// What the runner read from the output of a Claude run.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeReport {
    pub output: ClaudeOutput,
    pub screenshots: Vec<Screenshot>,
}

/// The final word of the runner about a run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunReport {
    pub outcome: RunOutcome,
    pub end: RunEnd,
    /// Present when the spec said the program speaks Claude's stream-json.
    pub claude: Option<ClaudeReport>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunProgress {
    /// Output after the requested sequence number, oldest first.
    pub lines: Vec<OutputLine>,
    pub report: Option<RunReport>,
    /// Whether the process supervising the run still exists.
    pub alive: bool,
}

#[async_trait]
pub trait RunnerPort: Send + Sync {
    /// Refuses what this runner cannot run, before anything is recorded.
    async fn check(&self, spec: &RunSpec) -> Result<(), AppError>;
    /// Starts the run detached and returns the id of its supervising process.
    async fn start(&self, run: Id<Run>, spec: &RunSpec) -> Result<u32, AppError>;
    /// Asks the run to stop; the report arrives later through `progress`.
    async fn cancel(&self, run: Id<Run>) -> Result<(), AppError>;
    async fn progress(&self, run: Id<Run>, after: i64, limit: u32)
    -> Result<RunProgress, AppError>;
    /// Finds the repository an absolute folder on this runner belongs to.
    async fn locate(&self, path: &str) -> Result<Located, AppError>;
    /// Adds a worktree on a new branch, creating its target branch first when missing.
    async fn create_worktree(&self, request: &NewWorktree) -> Result<CreatedWorktree, AppError>;
    /// Whether each absolute folder exists, in the order given.
    async fn folders_exist(&self, paths: &[String]) -> Result<Vec<bool>, AppError>;
    /// Adds a ticket's worktree again on its existing branch, moving leftovers aside.
    async fn ensure_worktree(&self, request: &EnsureWorktree) -> Result<EnsuredWorktree, AppError>;
    /// Removes a clean worktree folder after its links; refusals are answers, not errors.
    async fn remove_worktree(&self, request: &RemoveWorktree) -> Result<WorktreeRemoval, AppError>;
    /// A page of a session transcript; `None` when the file is gone.
    async fn read_transcript(
        &self,
        request: &TranscriptRead,
    ) -> Result<Option<TranscriptChunk>, AppError>;
    /// Figures of a session transcript; `None` when the file is gone.
    async fn transcript_facts(
        &self,
        source: &TranscriptSource,
    ) -> Result<Option<TranscriptSummary>, AppError>;
    /// Changed files from git when the session has its own branch, else from the transcript.
    async fn session_changes(&self, request: &ChangeRequest) -> Result<SessionChanges, AppError>;
    /// Diff of one changed file from the same source as `session_changes`.
    async fn session_diff(&self, request: &DiffRequest) -> Result<FileDiff, AppError>;
}

/// A file a run left on its runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArtifact {
    pub id: Uuid,
    pub screenshot: Screenshot,
    pub created_at: Timestamp,
}

/// Rows that belong to a finished run and are written with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunDetails {
    pub denied: Vec<DeniedTool>,
    pub artifacts: Vec<RunArtifact>,
}

/// What a run list is narrowed to; every set field must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunFilter {
    /// Ticket or question key, upper case.
    pub subject_key: Option<String>,
    pub kind: Option<RunKind>,
    /// Runs of this Claude session or with this run id.
    pub link: Option<RunLink>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLink {
    pub claude_session: String,
    pub run: Option<Id<Run>>,
}

#[async_trait]
pub trait RunRepository: Send + Sync {
    async fn insert(&self, run: &Run) -> Result<Change, AppError>;
    async fn update(&self, run: &Run, expected: Version) -> Result<Change, AppError> {
        self.update_with(run, expected, &RunDetails::default())
            .await
    }
    /// Like `update`, also writing the denied tools and artifacts of the run in one step.
    async fn update_with(
        &self,
        run: &Run,
        expected: Version,
        details: &RunDetails,
    ) -> Result<Change, AppError>;
    async fn get(&self, id: Id<Run>) -> Result<Option<Run>, AppError>;
    /// Newest first.
    async fn list(&self, limit: u32) -> Result<Vec<Run>, AppError>;
    /// Newest first, narrowed by the filter.
    async fn find(&self, filter: &RunFilter, limit: u32) -> Result<Vec<Run>, AppError>;
    async fn active(&self) -> Result<Vec<Run>, AppError>;
    async fn denied_tools(&self, id: Id<Run>) -> Result<Vec<DeniedTool>, AppError>;
    async fn artifacts(&self, id: Id<Run>) -> Result<Vec<RunArtifact>, AppError>;
}

#[must_use]
pub fn not_found(id: Id<Run>) -> AppError {
    AppError::not_found("run-not-found", format!("run {id} does not exist"))
}

fn transition(error: &RunError) -> AppError {
    match error {
        RunError::InvalidLabel => AppError::invalid("run-label-invalid", error.to_string()),
        RunError::InvalidTransition { .. } => AppError::conflict("run-finished", error.to_string()),
    }
}

/// Longest pause between checks of a run whose checks keep failing.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

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

    /// The id a run will get, for settings that name it before it starts.
    #[must_use]
    pub fn next_id(&self) -> Id<Run> {
        self.ids.as_ref().next()
    }

    pub async fn start(self: &Arc<Self>, label: &str, spec: &RunSpec) -> Result<Run, AppError> {
        self.start_planned(self.next_id(), label, spec, None).await
    }

    /// Records the run first, so a crash during the start leaves a trace instead of an orphan.
    pub async fn start_planned(
        self: &Arc<Self>,
        id: Id<Run>,
        label: &str,
        spec: &RunSpec,
        dispatch: Option<DispatchRun>,
    ) -> Result<Run, AppError> {
        let label = RunLabel::parse(label).map_err(|error| transition(&error))?;
        spec.validate()?;
        self.runner.check(spec).await?;
        let mut run = Run::new(id, label, self.clock.now());
        if let Some(dispatch) = dispatch {
            run = run.with_dispatch(dispatch);
        }
        self.save_new(&run).await?;
        match self.runner.start(run.id(), spec).await {
            Ok(pid) => {
                let running = run
                    .started(pid, self.clock.now())
                    .map_err(|error| transition(&error))?;
                let saved = self.save(&running, run.version()).await;
                self.watch(run.id());
                if let Err(error) = saved {
                    // the caller hears of a failure, so the run must not go on unseen
                    if let Err(cancel) = self.runner.cancel(run.id()).await {
                        tracing::warn!(run = %run.id(), error = %cancel, "cannot cancel run");
                    }
                    return Err(error);
                }
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

    pub async fn find(&self, filter: &RunFilter, limit: u32) -> Result<Vec<Run>, AppError> {
        self.repository.find(filter, limit).await
    }

    pub async fn denied_tools(&self, id: Id<Run>) -> Result<Vec<DeniedTool>, AppError> {
        self.repository.denied_tools(id).await
    }

    pub async fn artifacts(&self, id: Id<Run>) -> Result<Vec<RunArtifact>, AppError> {
        self.repository.artifacts(id).await
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

    /// Output after `after` and the run as it was before, whose status says whether more follows.
    pub async fn output(
        &self,
        id: Id<Run>,
        after: i64,
        limit: u32,
    ) -> Result<(Run, RunProgress), AppError> {
        let run = self.get(id).await?;
        let progress = self.runner.progress(id, after, limit).await?;
        Ok((run, progress))
    }

    /// Watches runs that were active when the daemon stopped; the runner knows how they ended.
    pub async fn recover(self: &Arc<Self>) -> Result<usize, AppError> {
        let active = self.repository.active().await?;
        for run in &active {
            self.watch(run.id());
        }
        Ok(active.len())
    }

    fn watch(self: &Arc<Self>, id: Id<Run>) {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let mut delay = service.poll;
            loop {
                tokio::time::sleep(delay).await;
                match service.settle(id).await {
                    Ok(true) => break,
                    Ok(false) => delay = service.poll,
                    Err(error) if error.code() == "run-not-found" => break,
                    Err(error) => {
                        tracing::warn!(run = %id, error = %error, "cannot check run");
                        delay = (delay * 2).min(MAX_BACKOFF);
                    }
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
        let (next, details) = match report {
            Some(report) => self.finish(&run, report),
            None => (run.lost(self.clock.now()), RunDetails::default()),
        };
        let next = next.map_err(|error| transition(&error))?;
        let change = self
            .repository
            .update_with(&next, run.version(), &details)
            .await?;
        self.signal.notify(&change);
        Ok(true)
    }

    /// The finished run with what the dispatcher learned from Claude's output.
    fn finish(&self, run: &Run, report: RunReport) -> (Result<Run, RunError>, RunDetails) {
        let now = self.clock.now();
        let (Some(dispatch), Some(claude)) = (run.dispatch(), report.claude) else {
            return (
                run.finished(report.outcome, report.end, now),
                RunDetails::default(),
            );
        };
        let mut output = claude.output;
        if output.error_text.is_empty()
            && let Some(error) = report.end.error.as_deref()
        {
            error.clone_into(&mut output.error_text);
        }
        let abort = (report.outcome != RunOutcome::Cancelled)
            .then(|| {
                classify(
                    &Ending {
                        timed_out: report.outcome == RunOutcome::TimedOut,
                        exit_code: report.end.exit_code,
                        max_turns: dispatch.profile.max_turns,
                        timeout_minutes: dispatch.profile.timeout_minutes,
                    },
                    &output,
                )
            })
            .flatten();
        let result = RunResult {
            session_id: output.session_id.clone(),
            denied_count: output.denied_count(),
            usage: output.usage.clone(),
            abort,
        };
        let details = RunDetails {
            denied: output.denied,
            artifacts: claude
                .screenshots
                .into_iter()
                .map(|screenshot| RunArtifact {
                    id: self.ids.next_uuid(),
                    screenshot,
                    created_at: now,
                })
                .collect(),
        };
        (
            run.finished_with(report.outcome, report.end, Some(result), now),
            details,
        )
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
            "run-label-invalid"
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
            env: RunEnv::default(),
            stdin: None,
            claude: false,
        };
        assert!(valid.validate().is_ok());
        let cases = [
            (
                RunSpec {
                    program: String::from(" "),
                    ..valid.clone()
                },
                "run-program-invalid",
            ),
            (
                RunSpec {
                    cwd: PathBuf::from("relative"),
                    ..valid.clone()
                },
                "run-cwd-invalid",
            ),
            (
                RunSpec {
                    timeout: Some(Duration::ZERO),
                    ..valid.clone()
                },
                "run-timeout-invalid",
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
