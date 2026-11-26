//! Setup commands of a new worktree, run before Claude starts in it.

use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::WorktreeSetupStep;
use tokio_util::sync::CancellationToken;

use crate::AppError;

/// Setup steps to run in one worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupRequest {
    /// The worktree folder; the folder of a step is relative to it.
    pub root: String,
    pub steps: Vec<WorktreeSetupStep>,
    /// Limit of each step.
    pub timeout: Duration,
    pub git_bash: Option<String>,
}

/// The step that stopped the setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupFailure {
    pub cwd: String,
    pub run: String,
    /// Exit code; none when the step timed out or never started.
    pub code: Option<i32>,
    pub timed_out: bool,
    /// The end of what the step printed.
    pub output: String,
}

impl SetupFailure {
    /// The step as it reads in a block reason: folder, then the command.
    #[must_use]
    pub fn step(&self) -> String {
        format!("{}$ {}", self.cwd, self.run)
    }

    /// What went wrong in one line.
    #[must_use]
    pub fn cause(&self) -> String {
        if self.timed_out {
            return String::from("timed out");
        }
        match self.code {
            Some(code) => format!("exit code {code}"),
            None => String::from("could not run"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupOutcome {
    Done,
    Failed(SetupFailure),
    /// Stopped on request, for instance at shutdown; no step failed.
    Cancelled,
}

#[async_trait]
pub trait WorktreeSetupPort: Send + Sync {
    /// Runs the steps in order; stops at a failure, or at `cancel`, which kills the step.
    async fn run(
        &self,
        request: &SetupRequest,
        cancel: &CancellationToken,
    ) -> Result<SetupOutcome, AppError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(code: Option<i32>, timed_out: bool) -> SetupFailure {
        SetupFailure {
            cwd: String::from("web"),
            run: String::from("npm ci"),
            code,
            timed_out,
            output: String::new(),
        }
    }

    #[test]
    fn a_failure_names_its_step_and_cause() {
        assert_eq!(failure(Some(1), false).step(), "web$ npm ci");
        assert_eq!(failure(Some(1), false).cause(), "exit code 1");
        assert_eq!(failure(None, true).cause(), "timed out");
        assert_eq!(failure(None, false).cause(), "could not run");
    }
}
