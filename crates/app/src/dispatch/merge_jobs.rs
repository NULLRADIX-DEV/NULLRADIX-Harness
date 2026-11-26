//! Merge jobs: queued when a ticket is accepted or finishes without review, one per worktree.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    DispatchOutcome, DispatcherConfig, Id, MergeJob, Run, RunKind, Ticket, TicketRecord, Timestamp,
    Version, Worktree,
};

use super::evaluate::DoneGate;
use super::review_decide::AcceptHook;
use crate::dispatcher_config::DispatcherConfigService;
use crate::runs::{RunFilter, RunService};
use crate::worktrees::WorktreeRepository;
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

/// Jobs a pass looks at; one worktree has at most one open job.
pub const MAX_OPEN_JOBS: u32 = 1000;
/// Jobs the history of one ticket lists.
pub const MAX_TICKET_JOBS: u32 = 50;
/// Merge runs of one ticket looked at to find one that is not settled.
pub const SESSION_RUNS_LOOKED_AT: u32 = 20;

/// Whether a merge session of one of the tickets has not been settled yet.
pub async fn merge_session_open(
    runs: &RunService,
    subjects: impl IntoIterator<Item = String>,
) -> Result<bool, AppError> {
    for subject in subjects {
        let filter = RunFilter {
            subject_key: Some(subject),
            kind: Some(RunKind::Merge),
            ..RunFilter::default()
        };
        let found = runs.find(&filter, SESSION_RUNS_LOOKED_AT).await?;
        if found.iter().any(|run| run.outcome().is_none()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A job with the branch of its worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMergeJob {
    pub job: MergeJob,
    pub branch: String,
}

#[async_trait]
pub trait MergeJobRepository: Send + Sync {
    /// Stores the job; `None` when its worktree has an open job already.
    async fn queue(&self, job: &MergeJob) -> Result<Option<Change>, AppError>;
    /// Jobs that still have to be processed, oldest first, at most [`MAX_OPEN_JOBS`].
    async fn open(&self) -> Result<Vec<MergeJob>, AppError>;
    /// Writes the job when it still has `expected` as version; `None` when it changed meanwhile.
    async fn save(
        &self,
        job: &MergeJob,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    /// The jobs of every worktree the ticket was ever linked to, newest first.
    async fn of_ticket(&self, ticket: Id<Ticket>) -> Result<Vec<StoredMergeJob>, AppError>;
    /// The open job of the worktree.
    async fn open_of(&self, worktree: Id<Worktree>) -> Result<Option<MergeJob>, AppError>;
}

pub struct MergeQueue {
    jobs: Arc<dyn MergeJobRepository>,
    worktrees: Arc<dyn WorktreeRepository>,
    config: Arc<DispatcherConfigService>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for MergeQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeQueue").finish_non_exhaustive()
    }
}

impl MergeQueue {
    #[must_use]
    pub fn new(
        jobs: Arc<dyn MergeJobRepository>,
        worktrees: Arc<dyn WorktreeRepository>,
        config: Arc<DispatcherConfigService>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            jobs,
            worktrees,
            config,
            clock,
            ids,
            signal,
        }
    }

    pub(crate) fn repository(&self) -> &Arc<dyn MergeJobRepository> {
        &self.jobs
    }

    /// Queues the merge of the ticket branch; false when there is nothing to merge or one waits.
    pub async fn queue(&self, ticket: &TicketRecord) -> Result<bool, AppError> {
        if ticket.branch.is_none() {
            return Ok(false);
        }
        let Some(entry) = self.worktrees.of_ticket(ticket.id).await? else {
            return Ok(false);
        };
        let record = entry.worktree.record();
        let target = ticket
            .target_branch
            .as_ref()
            .or(record.target_branch.as_ref())
            .map(|target| target.as_str().to_owned());
        let strategy = self.config.get().await?.config.merge_strategy;
        let job = MergeJob::pending(
            self.ids.as_ref().next(),
            entry.worktree.id(),
            target,
            strategy,
            self.clock.now(),
        );
        match self.jobs.queue(&job).await? {
            Some(change) => {
                self.signal.notify(&change);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// The merge history of a ticket, newest first.
    pub async fn of_ticket(&self, ticket: Id<Ticket>) -> Result<Vec<StoredMergeJob>, AppError> {
        self.jobs.of_ticket(ticket).await
    }
}

/// An accepted ticket queues its merge.
#[derive(Debug)]
pub struct QueueOnAccept(pub Arc<MergeQueue>);

#[async_trait]
impl AcceptHook for QueueOnAccept {
    async fn accepted(&self, ticket: &TicketRecord) -> Result<(), AppError> {
        self.0.queue(ticket).await.map(drop)
    }
}

/// Queues the merge of a ticket that finished without waiting for a review.
pub struct MergeGate {
    inner: Arc<dyn DoneGate>,
    queue: Arc<MergeQueue>,
}

impl std::fmt::Debug for MergeGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeGate").finish_non_exhaustive()
    }
}

impl MergeGate {
    #[must_use]
    pub fn new(inner: Arc<dyn DoneGate>, queue: Arc<MergeQueue>) -> Self {
        Self { inner, queue }
    }
}

#[async_trait]
impl DoneGate for MergeGate {
    async fn finished(
        &self,
        ticket: &TicketRecord,
        run: &Run,
        config: &DispatcherConfig,
    ) -> Result<DispatchOutcome, AppError> {
        let outcome = self.inner.finished(ticket, run, config).await?;
        // a held review queues its merge when a person accepts
        if outcome == DispatchOutcome::Done {
            self.queue.queue(ticket).await?;
        }
        Ok(outcome)
    }

    async fn held(&self, ticket: &TicketRecord, run: &Run) -> Result<bool, AppError> {
        self.inner.held(ticket, run).await
    }
}
