//! A person decides a held review: accept the ticket or return it to the agent, as legacy did.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Actor, REVIEW_FEEDBACK_MAX, ReviewState, SystemMarker, TicketRecord, TicketStatus, Version,
    pending_feedback, required_text, returned_comment,
};

use super::pass::DispatchPass;
use super::start::TicketStarter;
use crate::comments::CommentService;
use crate::review::ReviewService;
use crate::runs::RunService;
use crate::tickets::{TicketPatch, TicketService, TicketView, WriteOptions};
use crate::worktrees::WorktreeService;
use crate::{AppError, ErrorKind};

/// Tries to move the ticket while a person or an agent changes it too.
const MOVE_TRIES: usize = 3;

/// What follows an accepted ticket: the merge of a later slice hangs here.
#[async_trait]
pub trait AcceptHook: Send + Sync {
    /// The ticket is done and its review is decided; the branch can be merged.
    async fn accepted(&self, ticket: &TicketRecord) -> Result<(), AppError>;
}

/// Merges nothing yet.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAcceptHook;

#[async_trait]
impl AcceptHook for NoAcceptHook {
    async fn accepted(&self, _ticket: &TicketRecord) -> Result<(), AppError> {
        Ok(())
    }
}

#[must_use]
pub fn return_not_possible(key: &str, why: &str) -> AppError {
    AppError::invalid(
        "return-not-possible",
        format!("{key} cannot go back to the agent: {why}"),
    )
}

/// Decisions of a person on a held review, serialized with the dispatcher pass.
pub struct ReviewDecisions {
    pass: Arc<DispatchPass>,
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
    worktrees: Arc<WorktreeService>,
    reviews: Arc<ReviewService>,
    runs: Arc<RunService>,
    starter: Arc<TicketStarter>,
    hook: Arc<dyn AcceptHook>,
}

impl std::fmt::Debug for ReviewDecisions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewDecisions").finish_non_exhaustive()
    }
}

/// What the decisions are built from.
pub struct DecisionParts {
    pub pass: Arc<DispatchPass>,
    pub tickets: Arc<TicketService>,
    pub comments: Arc<CommentService>,
    pub worktrees: Arc<WorktreeService>,
    pub reviews: Arc<ReviewService>,
    pub runs: Arc<RunService>,
    pub starter: Arc<TicketStarter>,
}

impl ReviewDecisions {
    #[must_use]
    pub fn new(parts: DecisionParts) -> Self {
        Self {
            pass: parts.pass,
            tickets: parts.tickets,
            comments: parts.comments,
            worktrees: parts.worktrees,
            reviews: parts.reviews,
            runs: parts.runs,
            starter: parts.starter,
            hook: Arc::new(NoAcceptHook),
        }
    }

    /// Sets what follows an accepted ticket.
    #[must_use]
    pub fn with_hook(mut self, hook: Arc<dyn AcceptHook>) -> Self {
        self.hook = hook;
        self
    }

    /// Accepts a ticket that waits for a person and decides its review; a repeat changes nothing.
    pub async fn accept(
        &self,
        key: &str,
        actor: &Actor,
        expected: Option<Version>,
        by_person: bool,
    ) -> Result<TicketView, AppError> {
        self.pass
            .exclusive(|| self.accept_now(key, actor, expected, by_person))
            .await
    }

    async fn accept_now(
        &self,
        key: &str,
        actor: &Actor,
        expected: Option<Version>,
        by_person: bool,
    ) -> Result<TicketView, AppError> {
        let detail = self.tickets.get(key).await?;
        let record = detail.view.ticket.record();
        let text = record.key.to_string();
        if !record.awaits_a_person() && record.status == TicketStatus::Done {
            // a repeat, or an accept whose decision or hook was lost
            match self.reviews.state_of(&text).await? {
                Some(ReviewState::Approved) => {
                    self.hook.accepted(record).await?;
                    return Ok(detail.view);
                }
                Some(ReviewState::Pending) => {
                    self.reviews
                        .decide(&text, ReviewState::Approved, actor.id())
                        .await?;
                    self.hook.accepted(record).await?;
                    return Ok(detail.view);
                }
                _ => {}
            }
        }
        let view = self
            .worktrees
            .accept(key, actor, expected, by_person)
            .await?;
        let record = view.ticket.record();
        // only reviews of this ticket's own runs are decided, so a worktree mate stays pending
        self.reviews
            .decide(&text, ReviewState::Approved, actor.id())
            .await?;
        self.hook.accepted(record).await?;
        Ok(view)
    }

    /// Sends a ticket that waits for its review back to the agent with the feedback.
    pub async fn return_to_agent(
        &self,
        key: &str,
        feedback: &str,
        actor: &Actor,
        expected: Option<Version>,
    ) -> Result<TicketView, AppError> {
        let feedback = required_text(feedback, REVIEW_FEEDBACK_MAX).map_err(|error| {
            AppError::text(error, "feedback-empty", format!("the feedback {error}"))
        })?;
        self.pass
            .exclusive(|| self.return_now(key, feedback, actor, expected))
            .await
    }

    async fn return_now(
        &self,
        key: &str,
        feedback: &str,
        actor: &Actor,
        expected: Option<Version>,
    ) -> Result<TicketView, AppError> {
        let detail = self.tickets.get(key).await?;
        let record = detail.view.ticket.record();
        let text = record.key.to_string();
        if !record.awaits_a_person() {
            // a repeat of a return that went through says the same again
            let repeat = record.status == TicketStatus::Planned
                && self
                    .returned_feedback(&text)
                    .await?
                    .last()
                    .map(String::as_str)
                    == Some(feedback);
            if repeat {
                if expected.is_some_and(|version| version != record.version) {
                    return Err(AppError::version_conflict("ticket", &text));
                }
                // the decision may have been lost after the ticket went back
                self.reviews
                    .decide(&text, ReviewState::Returned, actor.id())
                    .await?;
                return Ok(detail.view);
            }
            return Err(return_not_possible(
                &text,
                "it waits for no review (only blocked with \"Human Interaction: ...\" and a branch)",
            ));
        }
        if record.branch.is_none() {
            return Err(return_not_possible(&text, "it has no branch"));
        }
        if expected.is_some_and(|version| version != record.version) {
            return Err(AppError::version_conflict("ticket", &text));
        }
        let running = self.runs.active().await?.iter().any(|run| {
            run.dispatch()
                .and_then(|dispatch| dispatch.subject_key.as_deref())
                == Some(text.as_str())
        });
        if running {
            return Err(return_not_possible(&text, "a run is under way for it"));
        }
        // a retry after a failed move to planned must not note the feedback twice
        if !self
            .returned_feedback(&text)
            .await?
            .iter()
            .any(|noted| noted == feedback)
        {
            let body = returned_comment(actor.name().as_str(), feedback);
            let author = self.starter.actor().await?;
            self.comments
                .add_system(
                    &record.key,
                    &body,
                    Some(SystemMarker::ReviewReturned),
                    None,
                    author,
                )
                .await?;
        }
        let view = self.back_to_planned(&text, actor).await?;
        self.reviews
            .decide(&text, ReviewState::Returned, actor.id())
            .await?;
        Ok(view)
    }

    /// The feedback returned since the agent started last, oldest first.
    async fn returned_feedback(&self, key: &str) -> Result<Vec<String>, AppError> {
        let lines = self.comments.list(key).await?;
        Ok(pending_feedback(lines.iter().map(|line| {
            (line.comment.record().system_marker, line.comment.body())
        })))
    }

    /// Planned without a reason or assignee; the branch and the worktree stay.
    async fn back_to_planned(&self, key: &str, actor: &Actor) -> Result<TicketView, AppError> {
        let patch = TicketPatch {
            status: Some(TicketStatus::Planned.as_str().to_owned()),
            blocked_reason: Some(None),
            assignee: Some(None),
            ..TicketPatch::default()
        };
        let options = WriteOptions {
            force: true,
            ..WriteOptions::default()
        };
        for _ in 0..MOVE_TRIES {
            // the comment moves the version, so each try reads it afresh
            let fresh = self.tickets.get(key).await?;
            let version = fresh.view.ticket.record().version;
            match self
                .tickets
                .update(key, &patch, actor, version, options)
                .await
            {
                Ok(detail) => return Ok(detail.view),
                Err(error) if error.kind() == ErrorKind::Conflict => {}
                Err(error) => return Err(error),
            }
        }
        Err(AppError::version_conflict("ticket", key))
    }
}
