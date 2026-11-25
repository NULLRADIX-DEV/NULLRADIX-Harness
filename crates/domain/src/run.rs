use strum::{EnumString, IntoStaticStr};

use crate::dispatch_run::{Abort, AbortCategory, DispatchRun, RunResult};
use crate::{Id, Timestamp, Version};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum RunStatus {
    Starting,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Lost,
}

impl RunStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

/// How the runner reports a finished run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum RunOutcome {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl RunOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    const fn status(self) -> RunStatus {
        match self {
            Self::Succeeded => RunStatus::Succeeded,
            Self::Failed => RunStatus::Failed,
            Self::Cancelled => RunStatus::Cancelled,
            Self::TimedOut => RunStatus::TimedOut,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    #[error("a run label has 1 to 200 characters")]
    InvalidLabel,
    #[error("a run cannot go from {from} to {to}")]
    InvalidTransition {
        from: &'static str,
        to: &'static str,
    },
}

/// Short human description of what a run does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLabel(String);

impl RunLabel {
    pub fn parse(text: &str) -> Result<Self, RunError> {
        let trimmed = text.trim();
        (1..=200)
            .contains(&trimmed.chars().count())
            .then(|| Self(trimmed.to_owned()))
            .ok_or(RunError::InvalidLabel)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Everything a finished run reports besides its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEnd {
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    id: Id<Run>,
    label: RunLabel,
    status: RunStatus,
    pid: Option<u32>,
    exit_code: Option<i32>,
    error: Option<String>,
    created_at: Timestamp,
    started_at: Option<Timestamp>,
    finished_at: Option<Timestamp>,
    dispatch: Option<DispatchRun>,
    result: Option<RunResult>,
    version: Version,
}

/// Stored fields of a run, for persistence adapters only.
#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub id: Id<Run>,
    pub label: RunLabel,
    pub status: RunStatus,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub dispatch: Option<DispatchRun>,
    pub result: Option<RunResult>,
    pub version: Version,
}

impl Run {
    #[must_use]
    pub fn new(id: Id<Self>, label: RunLabel, now: Timestamp) -> Self {
        Self {
            id,
            label,
            status: RunStatus::Starting,
            pid: None,
            exit_code: None,
            error: None,
            created_at: now,
            started_at: None,
            finished_at: None,
            dispatch: None,
            result: None,
            version: Version::INITIAL,
        }
    }

    /// Marks the run as one Claude run of the dispatcher.
    #[must_use]
    pub fn with_dispatch(mut self, dispatch: DispatchRun) -> Self {
        self.dispatch = Some(dispatch);
        self
    }

    #[must_use]
    pub fn restore(record: RunRecord) -> Self {
        Self {
            id: record.id,
            label: record.label,
            status: record.status,
            pid: record.pid,
            exit_code: record.exit_code,
            error: record.error,
            created_at: record.created_at,
            started_at: record.started_at,
            finished_at: record.finished_at,
            dispatch: record.dispatch,
            result: record.result,
            version: record.version,
        }
    }

    fn transition(&self, to: RunStatus, allowed: bool) -> Result<Self, RunError> {
        if allowed {
            Ok(Self {
                status: to,
                version: self.version.next(),
                ..self.clone()
            })
        } else {
            Err(RunError::InvalidTransition {
                from: self.status.as_str(),
                to: to.as_str(),
            })
        }
    }

    /// The supervisor process is up.
    pub fn started(&self, pid: u32, now: Timestamp) -> Result<Self, RunError> {
        let mut next = self.transition(RunStatus::Running, self.status == RunStatus::Starting)?;
        next.pid = Some(pid);
        next.started_at = Some(now);
        Ok(next)
    }

    /// The process could not even be started.
    pub fn failed_to_start(&self, error: String, now: Timestamp) -> Result<Self, RunError> {
        let mut next = self.transition(RunStatus::Failed, self.status == RunStatus::Starting)?;
        next.result = self.dispatch.as_ref().map(|_| RunResult {
            abort: Some(Abort {
                category: AbortCategory::StartFailed,
                reason: error.clone(),
            }),
            ..RunResult::default()
        });
        next.error = Some(error);
        next.finished_at = Some(now);
        Ok(next)
    }

    pub fn finished(
        &self,
        outcome: RunOutcome,
        end: RunEnd,
        now: Timestamp,
    ) -> Result<Self, RunError> {
        self.finished_with(outcome, end, None, now)
    }

    /// Like `finished`, with what the dispatcher learned from the output of a Claude run.
    pub fn finished_with(
        &self,
        outcome: RunOutcome,
        end: RunEnd,
        result: Option<RunResult>,
        now: Timestamp,
    ) -> Result<Self, RunError> {
        let mut next = self.transition(outcome.status(), self.status.is_active())?;
        next.exit_code = end.exit_code;
        next.error = end.error;
        next.result = result;
        next.finished_at = Some(now.max(self.created_at));
        Ok(next)
    }

    /// The supervisor vanished without reporting a result.
    pub fn lost(&self, now: Timestamp) -> Result<Self, RunError> {
        let mut next = self.transition(RunStatus::Lost, self.status.is_active())?;
        next.result = self.dispatch.as_ref().map(|_| RunResult {
            abort: Some(Abort {
                category: AbortCategory::Vanished,
                reason: String::from("the run vanished without a result"),
            }),
            ..RunResult::default()
        });
        next.finished_at = Some(now.max(self.created_at));
        Ok(next)
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.id
    }

    #[must_use]
    pub const fn label(&self) -> &RunLabel {
        &self.label
    }

    #[must_use]
    pub const fn status(&self) -> RunStatus {
        self.status
    }

    #[must_use]
    pub const fn pid(&self) -> Option<u32> {
        self.pid
    }

    #[must_use]
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }

    #[must_use]
    pub const fn started_at(&self) -> Option<Timestamp> {
        self.started_at
    }

    #[must_use]
    pub const fn finished_at(&self) -> Option<Timestamp> {
        self.finished_at
    }

    #[must_use]
    pub const fn dispatch(&self) -> Option<&DispatchRun> {
        self.dispatch.as_ref()
    }

    #[must_use]
    pub const fn result(&self) -> Option<&RunResult> {
        self.result.as_ref()
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> Run {
        Run::new(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            RunLabel::parse("echo").unwrap(),
            Timestamp::from_millis(10),
        )
    }

    fn end(code: i32) -> RunEnd {
        RunEnd {
            exit_code: Some(code),
            error: None,
        }
    }

    #[test]
    fn follows_the_happy_path() {
        let started = run().started(42, Timestamp::from_millis(11)).unwrap();
        assert_eq!(started.status(), RunStatus::Running);
        assert_eq!(started.pid(), Some(42));
        let done = started
            .finished(RunOutcome::Succeeded, end(0), Timestamp::from_millis(20))
            .unwrap();
        assert_eq!(done.status(), RunStatus::Succeeded);
        assert_eq!(done.exit_code(), Some(0));
        assert_eq!(done.version(), Version::from_stored(3));
        assert!(!done.status().is_active());
    }

    #[test]
    fn rejects_transitions_out_of_final_states() {
        let done = run()
            .started(1, Timestamp::from_millis(11))
            .unwrap()
            .finished(RunOutcome::Failed, end(3), Timestamp::from_millis(12))
            .unwrap();
        assert_eq!(
            done.lost(Timestamp::from_millis(13)),
            Err(RunError::InvalidTransition {
                from: "failed",
                to: "lost"
            })
        );
        assert!(done.started(2, Timestamp::from_millis(13)).is_err());
    }

    #[test]
    fn only_starting_runs_can_fail_to_start() {
        let failed = run()
            .failed_to_start(String::from("no such program"), Timestamp::from_millis(11))
            .unwrap();
        assert_eq!(failed.status(), RunStatus::Failed);
        assert_eq!(failed.error(), Some("no such program"));
        let running = run().started(1, Timestamp::from_millis(11)).unwrap();
        assert!(
            running
                .failed_to_start(String::new(), Timestamp::from_millis(12))
                .is_err()
        );
    }

    #[test]
    fn active_runs_can_get_lost() {
        let lost = run().lost(Timestamp::from_millis(5)).unwrap();
        assert_eq!(lost.status(), RunStatus::Lost);
        assert_eq!(lost.finished_at(), Some(Timestamp::from_millis(10)));
    }

    #[test]
    fn statuses_have_kebab_case_wire_names() {
        assert_eq!(RunStatus::TimedOut.as_str(), "timed-out");
        assert_eq!("timed-out".parse::<RunStatus>(), Ok(RunStatus::TimedOut));
        assert_eq!(RunOutcome::Cancelled.as_str(), "cancelled");
    }

    #[test]
    fn labels_are_trimmed_and_bounded() {
        assert_eq!(RunLabel::parse(" build ").unwrap().as_str(), "build");
        assert_eq!(RunLabel::parse(""), Err(RunError::InvalidLabel));
    }

    fn dispatch_run() -> Run {
        run().with_dispatch(DispatchRun {
            kind: crate::RunKind::Work,
            trigger: crate::RunTrigger::Manual,
            subject_key: Some(String::from("ARC-0001")),
            profile: crate::RunProfile {
                id: None,
                name: String::from("medium"),
                model: Some(String::from("opus")),
                effort: None,
                max_turns: 80,
                timeout_minutes: 90,
            },
            account: None,
            config_hash: String::from("0123456789abcdef"),
            cwd: String::from("/work"),
            args: vec![String::from("-p")],
            prompt: String::from("go"),
        })
    }

    #[test]
    fn plain_runs_have_no_dispatch_data_and_no_abort() {
        let lost = run().lost(Timestamp::from_millis(20)).unwrap();
        assert_eq!((lost.dispatch(), lost.result()), (None, None));
        let failed = run()
            .failed_to_start(String::from("x"), Timestamp::from_millis(20))
            .unwrap();
        assert_eq!(failed.result(), None);
    }

    #[test]
    fn a_dispatch_run_that_never_started_or_vanished_says_so() {
        let category = |run: &Run| {
            run.result()
                .and_then(|result| result.abort.as_ref())
                .map(|abort| abort.category)
        };
        let failed = dispatch_run()
            .failed_to_start(String::from("no claude"), Timestamp::from_millis(20))
            .unwrap();
        assert_eq!(category(&failed), Some(AbortCategory::StartFailed));
        let lost = dispatch_run()
            .started(1, Timestamp::from_millis(11))
            .unwrap()
            .lost(Timestamp::from_millis(20))
            .unwrap();
        assert_eq!(category(&lost), Some(AbortCategory::Vanished));
        assert_eq!(lost.dispatch().unwrap().kind, crate::RunKind::Work);
    }

    #[test]
    fn a_finished_dispatch_run_keeps_what_the_output_said() {
        let result = RunResult {
            session_id: Some(String::from("s-1")),
            usage: Some(crate::RunUsage {
                cost_usd: Some(0.5),
                ..crate::RunUsage::default()
            }),
            denied_count: Some(2),
            abort: None,
        };
        let done = dispatch_run()
            .started(1, Timestamp::from_millis(11))
            .unwrap()
            .finished_with(
                RunOutcome::Succeeded,
                end(0),
                Some(result.clone()),
                Timestamp::from_millis(30),
            )
            .unwrap();
        assert_eq!(done.result(), Some(&result));
        assert_eq!(done.version(), Version::from_stored(3));
    }

    #[test]
    fn dispatch_kinds_triggers_and_categories_have_kebab_case_names() {
        assert_eq!(crate::RunKind::Research.as_str(), "research");
        assert!(crate::RunKind::Plan.is_restricted() && !crate::RunKind::Merge.is_restricted());
        assert!(crate::RunKind::Research.is_about_question());
        assert_eq!(crate::RunTrigger::Dispatcher.as_str(), "dispatcher");
        assert_eq!(AbortCategory::MaxTurns.as_str(), "max-turns");
        assert_eq!(
            "usage-limit".parse::<AbortCategory>(),
            Ok(AbortCategory::UsageLimit)
        );
    }
}
