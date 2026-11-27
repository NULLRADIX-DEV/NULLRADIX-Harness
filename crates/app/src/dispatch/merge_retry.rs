//! A person retries the merge of a ticket: done again, queued again, merged at once if it can.

use std::sync::Arc;

use nullradix_domain::{
    Actor, Id, MERGE_REASON, MergeJob, MergePhase, SystemMarker, TicketRecord, TicketStatus,
    Version, Worktree,
};

use super::merge_jobs::{MergeQueue, merge_session_open};
use super::pass::{DispatchPass, PassPhase};
use super::start::TicketStarter;
use crate::comments::CommentService;
use crate::runs::RunService;
use crate::tickets::{TicketPatch, TicketService, TicketView, WriteOptions};
use crate::worktrees::WorktreeRepository;
use crate::{AppError, ChangeSignal, Clock};

#[must_use]
pub fn merge_not_possible(key: &str, why: &str) -> AppError {
    AppError::invalid(
        "merge-not-possible",
        format!("{key} cannot be merged again: {why}"),
    )
}

/// What became of the merge the retry asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryOutcome {
    Merged,
    Blocked,
    Waiting,
    Session,
    Pending,
}

/// The ticket after the retry, with one line saying what happened.
#[derive(Debug, Clone)]
pub struct MergeRetried {
    pub outcome: RetryOutcome,
    pub text: String,
    pub view: TicketView,
}

/// What the retry is built from.
pub struct RetryParts {
    pub pass: Arc<DispatchPass>,
    pub queue: Arc<MergeQueue>,
    pub tickets: Arc<TicketService>,
    pub comments: Arc<CommentService>,
    pub worktrees: Arc<dyn WorktreeRepository>,
    pub runs: Arc<RunService>,
    pub starter: Arc<TicketStarter>,
    pub clock: Arc<dyn Clock>,
    pub signal: ChangeSignal,
}

/// Retries of merges, serialized with the dispatcher pass.
pub struct MergeRetry {
    parts: RetryParts,
}

impl std::fmt::Debug for MergeRetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeRetry").finish_non_exhaustive()
    }
}

fn merge_blocked(record: &TicketRecord) -> bool {
    record.status == TicketStatus::Blocked
        && record
            .blocked_reason
            .as_deref()
            .unwrap_or_default()
            .starts_with(MERGE_REASON)
}

impl MergeRetry {
    #[must_use]
    pub const fn new(parts: RetryParts) -> Self {
        Self { parts }
    }

    /// Queues the merge again and runs it on its own task, so a caller that leaves cancels nothing.
    pub async fn retry(
        self: &Arc<Self>,
        key: &str,
        actor: &Actor,
        expected: Option<Version>,
    ) -> Result<MergeRetried, AppError> {
        let this = Arc::clone(self);
        let key = key.to_owned();
        let actor = actor.clone();
        tokio::spawn(async move {
            this.parts
                .pass
                .exclusive(|| this.retry_now(&key, &actor, expected))
                .await
        })
        .await
        .map_err(AppError::internal)?
    }

    async fn retry_now(
        &self,
        key: &str,
        actor: &Actor,
        expected: Option<Version>,
    ) -> Result<MergeRetried, AppError> {
        let detail = self.parts.tickets.get(key).await?;
        let record = detail.view.ticket.record().clone();
        let text = record.key.to_string();
        let blocked = merge_blocked(&record);
        if record.branch.is_none() || !(blocked || record.status == TicketStatus::Done) {
            return Err(merge_not_possible(
                &text,
                "it waits for no merge (only done with a branch, or blocked with \"Merge: ...\")",
            ));
        }
        let Some(entry) = self.parts.worktrees.of_ticket(record.id).await? else {
            return Err(merge_not_possible(&text, "it has no worktree"));
        };
        let worktree = entry.worktree.id();
        let open = self.parts.queue.repository().open_of(worktree).await?;
        if !blocked && open.as_ref().is_some_and(MergeJob::pristine) {
            return self.run_and_report(&text, worktree).await;
        }
        let live: Vec<String> = entry
            .tickets
            .iter()
            .filter(|link| link.live())
            .map(|link| link.key.to_string())
            .collect();
        // a session that ended but is not settled yet still owns the job
        let subjects = live.iter().cloned().chain(std::iter::once(text.clone()));
        let session = merge_session_open(&self.parts.runs, subjects).await?;
        if session {
            return Err(merge_not_possible(
                &text,
                "a merge session is still running",
            ));
        }
        if expected.is_some_and(|version| version != record.version) {
            return Err(AppError::version_conflict("ticket", &text));
        }
        if blocked {
            let patch = TicketPatch {
                status: Some(TicketStatus::Done.as_str().to_owned()),
                ..TicketPatch::default()
            };
            let options = WriteOptions {
                force: true,
                ..WriteOptions::default()
            };
            self.parts
                .tickets
                .update(&text, &patch, actor, record.version, options)
                .await?;
        }
        match open {
            Some(job) => {
                let version = job.record().version;
                let at = self.parts.clock.now();
                match self
                    .parts
                    .queue
                    .repository()
                    .save(&job.retried(), version, at)
                    .await?
                {
                    Some(change) => self.parts.signal.notify(&change),
                    None => {
                        return Err(AppError::conflict(
                            "version-conflict",
                            "the merge job changed while it was retried",
                        ));
                    }
                }
            }
            None => {
                if !self.parts.queue.queue(&record).await? {
                    return Err(merge_not_possible(&text, "nothing could be queued"));
                }
            }
        }
        let author = self.parts.starter.actor().await?;
        let body = format!(
            "[dispatch] merge: retry started by {}.",
            actor.name().as_str()
        );
        self.parts
            .comments
            .add_system(&record.key, &body, Some(SystemMarker::Merge), None, author)
            .await?;
        self.run_and_report(&text, worktree).await
    }

    /// Runs the merge step and reports the errors of this worktree's job only.
    async fn run_and_report(
        &self,
        key: &str,
        worktree: Id<Worktree>,
    ) -> Result<MergeRetried, AppError> {
        let report = self.parts.pass.run_step(PassPhase::MergeAll).await;
        let own = format!("merge {worktree}:");
        let errors: Vec<String> = report
            .errors
            .into_iter()
            .filter(|error| error.starts_with(&own) || error.starts_with("merge-all:"))
            .collect();
        self.report(key, &errors).await
    }

    /// Reads what the merge of the ticket is now.
    async fn report(&self, key: &str, run: &[String]) -> Result<MergeRetried, AppError> {
        let detail = self.parts.tickets.get(key).await?;
        let record = detail.view.ticket.record();
        let jobs = self.parts.queue.of_ticket(record.id).await?;
        let newest = jobs.first().map(|stored| stored.job.record());
        let (outcome, text) = if let Some(job) =
            newest.filter(|job| job.phase == MergePhase::Merged)
        {
            (
                RetryOutcome::Merged,
                format!(
                    "{key} merged into {} ({}).",
                    job.merged_into.as_deref().unwrap_or_default(),
                    job.merged_commit.as_deref().unwrap_or_default()
                ),
            )
        } else if merge_blocked(record) {
            let reason = record.blocked_reason.as_deref().unwrap_or_default();
            (
                RetryOutcome::Blocked,
                format!(
                    "{key}: merge blocked - {}",
                    reason.trim_start_matches(MERGE_REASON).trim()
                ),
            )
        } else if newest.is_some_and(|job| job.phase == MergePhase::ConflictSession) {
            (
                RetryOutcome::Session,
                format!("{key}: merge conflict - a merge session started."),
            )
        } else if let Some(error) = run.first() {
            (
                RetryOutcome::Pending,
                format!("{key}: merge not run - {error}"),
            )
        } else if newest.is_some_and(|job| job.conflict_seen) {
            (
                RetryOutcome::Pending,
                format!("{key}: merge conflict - the merge waits for a merge session."),
            )
        } else if let Some(held) = jobs.first().and_then(|stored| stored.job.held_text()) {
            (
                RetryOutcome::Waiting,
                format!(
                    "{key}: the merge waits for {held} - it is merged automatically once that is done."
                ),
            )
        } else if newest.is_some_and(|job| job.phase == MergePhase::Waiting) {
            (
                RetryOutcome::Waiting,
                format!("{key}: the merge waits for other tickets of the worktree."),
            )
        } else {
            (RetryOutcome::Pending, format!("{key}: merge queued."))
        };
        Ok(MergeRetried {
            outcome,
            text,
            view: detail.view,
        })
    }
}
