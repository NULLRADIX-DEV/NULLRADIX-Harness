//! Holds a finished ticket for a review before it is merged, as legacy `ReviewService::hold`.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    DispatchOutcome, DispatcherConfig, Run, SystemMarker, TestRun, TicketRecord, TicketStatus,
    review_reason, test_summary,
};

use super::evaluate::DoneGate;
use super::start::TicketStarter;
use super::stream::{Joined, LineJoiner};
use super::test_runs::TestRuns;
use crate::comments::CommentService;
use crate::notifier::{NoNotifier, Notice, NoticeKind, Notifier};
use crate::review::{Held, ReviewService};
use crate::runs::{OutputStream, RunService};
use crate::tickets::{TicketPatch, TicketService, WriteOptions};
use crate::{AppError, ErrorKind};

/// Output lines read per request while looking for the tests.
const PAGE: u32 = 1000;
/// Pages read at most; a run with more output keeps the tests found so far.
const MAX_PAGES: usize = 500;
/// Longest output line looked at for a test call or its result.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Tries to block the ticket while a person or an agent changes it too.
const BLOCK_TRIES: usize = 3;

pub struct ReviewGate {
    review: Arc<ReviewService>,
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
    runs: Arc<RunService>,
    starter: Arc<TicketStarter>,
    notifier: Arc<dyn Notifier>,
}

impl std::fmt::Debug for ReviewGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewGate").finish_non_exhaustive()
    }
}

impl ReviewGate {
    #[must_use]
    pub fn new(
        review: Arc<ReviewService>,
        tickets: Arc<TicketService>,
        comments: Arc<CommentService>,
        runs: Arc<RunService>,
        starter: Arc<TicketStarter>,
    ) -> Self {
        Self {
            review,
            tickets,
            comments,
            runs,
            starter,
            notifier: Arc::new(NoNotifier),
        }
    }

    /// Shows a notice when a ticket starts to wait for its review.
    #[must_use]
    pub fn with_notifier(mut self, notifier: Arc<dyn Notifier>) -> Self {
        self.notifier = notifier;
        self
    }

    /// The tests the agent ran, read from the output of the run.
    async fn tests_of(&self, run: &Run) -> Result<Vec<TestRun>, AppError> {
        let mut joiner = LineJoiner::with_cap(MAX_LINE_BYTES);
        let mut tests = TestRuns::new();
        let mut after = 0;
        for _ in 0..MAX_PAGES {
            let lines = match self.runs.output(run.id(), after, PAGE).await {
                Ok((_, progress)) => progress.lines,
                Err(error) if error.kind() == ErrorKind::NotFound => break,
                Err(error) => return Err(error),
            };
            let read = lines.len();
            let Some(last) = lines.last().map(|line| line.seq) else {
                break;
            };
            after = last;
            let mut whole = Vec::new();
            for line in lines
                .iter()
                .filter(|line| line.stream == OutputStream::Stdout)
            {
                if let Some(Joined::Line(text)) = joiner.push(&line.text, line.continues) {
                    whole.push(text);
                }
            }
            // the lines can be large, so parsing them stays off the async runtime
            tests = tokio::task::spawn_blocking(move || {
                for line in &whole {
                    tests.feed(line);
                }
                tests
            })
            .await
            .map_err(AppError::internal)?;
            if u32::try_from(read).is_ok_and(|read| read < PAGE) {
                break;
            }
        }
        if let Some(Joined::Line(text)) = joiner.finish() {
            tests.feed(&text);
        }
        Ok(tests.finish())
    }

    /// Whether the comment of this run, that the ticket waits for approval, is there.
    async fn commented(&self, ticket: &TicketRecord, run: &Run) -> Result<bool, AppError> {
        let detail = self.tickets.get(&ticket.key.to_string()).await?;
        Ok(detail.comments.iter().any(|line| {
            let record = line.comment.record();
            record.system_marker == Some(SystemMarker::Review) && record.run == Some(run.id())
        }))
    }

    /// Comments that the ticket waits for approval.
    async fn comment(
        &self,
        ticket: &TicketRecord,
        run: &Run,
        tests: &[TestRun],
        changes: &str,
    ) -> Result<(), AppError> {
        let body = format!(
            "[dispatch] review: waiting for approval - {changes}, tests: {}.\nAccept it or return it to the agent in the review panel.",
            test_summary(tests)
        );
        let actor = self.starter.actor().await?;
        self.comments
            .add_system(
                &ticket.key,
                &body,
                Some(SystemMarker::Review),
                Some(run.id()),
                actor,
            )
            .await
    }

    /// Blocks the ticket on the review and clears its assignee, unless it is not done any more.
    async fn block(&self, ticket: &TicketRecord) -> Result<(), AppError> {
        let key = ticket.key.to_string();
        let actor = self.starter.actor().await?;
        let patch = TicketPatch {
            status: Some(TicketStatus::Blocked.as_str().to_owned()),
            blocked_reason: Some(Some(review_reason())),
            assignee: Some(None),
            ..TicketPatch::default()
        };
        let options = WriteOptions {
            force: true,
            ..WriteOptions::default()
        };
        for _ in 0..BLOCK_TRIES {
            // the comment and a person both move the version, so each try reads it afresh
            let fresh = self.tickets.get(&key).await?;
            let record = fresh.view.ticket.record();
            if record.status != TicketStatus::Done {
                return Ok(());
            }
            match self
                .tickets
                .update(&key, &patch, actor, record.version, options)
                .await
            {
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == ErrorKind::Conflict => {}
                Err(error) => return Err(error),
            }
        }
        Err(AppError::conflict(
            "version-conflict",
            format!("{key} kept changing while it was held for review"),
        ))
    }
}

#[async_trait]
impl DoneGate for ReviewGate {
    async fn finished(
        &self,
        ticket: &TicketRecord,
        run: &Run,
        config: &DispatcherConfig,
    ) -> Result<DispatchOutcome, AppError> {
        // autopilot has no review, and a ticket without a branch has nothing to look at
        if !config.review_before_merge
            || ticket.branch.is_none()
            || config.autopilot_applies(&ticket.labels)
        {
            return Ok(DispatchOutcome::Done);
        }
        let tests = self.tests_of(run).await?;
        match self.review.hold(ticket, run.id(), &tests).await? {
            Held::NoWorktree | Held::NoBranch => return Ok(DispatchOutcome::Done),
            // held and commented: a person may have accepted it since, so it is not blocked again
            Held::Already if self.commented(ticket, run).await? => {
                return Ok(DispatchOutcome::Review);
            }
            Held::New | Held::Already => {}
        }
        let changes = self.review.summary(ticket).await?;
        self.comment(ticket, run, &tests, &changes).await?;
        self.block(ticket).await?;
        let notice = Notice {
            kind: NoticeKind::Review,
            ticket: Some(ticket.key.to_string()),
            title: ticket.title.as_str().to_owned(),
            text: format!("waiting for approval - {changes}"),
        };
        if let Err(error) = self.notifier.notify(&notice).await {
            tracing::warn!(ticket = %ticket.key, "cannot show the review notice: {}", error.message());
        }
        Ok(DispatchOutcome::Review)
    }

    async fn held(&self, run: &Run) -> Result<bool, AppError> {
        self.review.held(run.id()).await
    }
}
