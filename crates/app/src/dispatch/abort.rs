//! Aborted attempts: the comment that counts, then the ticket goes back or blocks.

use std::sync::Arc;

use nullradix_domain::{
    ABORT_PREFIX, Actor, CommentRecord, DispatcherConfig, Run, SystemMarker, TicketKey,
    TicketRecord, TicketStatus, Version, failed_attempts,
};

use super::log::{DispatchLog, LogLevel};
use crate::comments::CommentService;
use crate::tickets::{TicketPatch, TicketService, WriteOptions};
use crate::{AppError, ErrorKind};

/// Where the ticket stands when the attempt is counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// In progress with the agent; the abort gives it back.
    Claimed,
    /// A failed start already gave it back to planned.
    GivenBack,
}

/// One aborted attempt to count.
#[derive(Debug, Clone, Copy)]
pub struct AbortRequest<'a> {
    pub key: &'a TicketKey,
    /// The run that ended; a comment of it that exists already is not written again.
    pub run: Option<&'a Run>,
    pub agent: &'a str,
    pub reason: &'a str,
    pub standing: Standing,
    /// The version the ticket must still have; a changed ticket is not counted.
    pub expected: Option<Version>,
}

/// What counting an attempt came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aborted {
    Counted {
        attempt: i64,
        last: bool,
    },
    /// The ticket is not as the dispatcher left it; look again on the next pass.
    Postponed,
}

pub struct AbortAttempts {
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
    log: Arc<DispatchLog>,
}

impl std::fmt::Debug for AbortAttempts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AbortAttempts").finish_non_exhaustive()
    }
}

fn standing(record: &TicketRecord, agent: &str, standing: Standing) -> bool {
    match standing {
        Standing::Claimed => {
            record.status == TicketStatus::InProgress && record.assignee.as_deref() == Some(agent)
        }
        Standing::GivenBack => record.status == TicketStatus::Planned && record.assignee.is_none(),
    }
}

fn is_abort_of(record: &CommentRecord, actor: &Actor) -> bool {
    record.author == actor.id() && record.body.as_str().starts_with(ABORT_PREFIX)
}

fn body_of(attempt: i64, max: i64, reason: &str, last: bool, run: Option<&Run>) -> String {
    let closing = if last {
        "That was the last automatic attempt - the ticket goes to blocked."
    } else {
        "The ticket goes back to planned."
    };
    let reason = reason.trim().trim_end_matches('.');
    let body = format!("{ABORT_PREFIX}: attempt {attempt}/{max}: {reason}.\n{closing}");
    match run {
        Some(run) => format!("{body}\nRun: {}", run.id()),
        None => body,
    }
}

impl AbortAttempts {
    #[must_use]
    pub fn new(
        tickets: Arc<TicketService>,
        comments: Arc<CommentService>,
        log: Arc<DispatchLog>,
    ) -> Self {
        Self {
            tickets,
            comments,
            log,
        }
    }

    /// Counts the attempt with its comment, then gives the ticket back or blocks it.
    pub async fn abort(
        &self,
        request: &AbortRequest<'_>,
        config: &DispatcherConfig,
        actor: &Actor,
    ) -> Result<Aborted, AppError> {
        let text = request.key.to_string();
        let detail = self.tickets.get(&text).await?;
        let records: Vec<&CommentRecord> = detail
            .comments
            .iter()
            .map(|line| line.comment.record())
            .collect();
        // a comment of this run is an attempt counted before a crash
        let before = request.run.and_then(|run| {
            records
                .iter()
                .position(|record| is_abort_of(record, actor) && record.run == Some(run.id()))
        });
        let max = config.max_attempts;
        let counted = |records: &[&CommentRecord]| {
            i64::try_from(failed_attempts(records.iter().copied(), actor.id())).unwrap_or(i64::MAX)
        };
        let attempt = before.map_or_else(
            || counted(&records).saturating_add(1),
            |at| counted(&records[..=at]),
        );
        let last = attempt >= max;
        if before.is_none() {
            let moved = request
                .expected
                .is_some_and(|version| version != detail.view.ticket.record().version);
            if moved || !standing(detail.view.ticket.record(), request.agent, request.standing) {
                return Ok(Aborted::Postponed);
            }
            let body = body_of(attempt, max, request.reason, last, request.run);
            self.comments
                .add_system(
                    request.key,
                    &body,
                    Some(SystemMarker::Abort),
                    request.run.map(Run::id),
                    actor,
                )
                .await?;
            self.log
                .write(
                    LogLevel::Info,
                    &format!("abort {text} ({attempt}/{max}): {}", request.reason),
                    Some(&text),
                    request.run.map(Run::id),
                )
                .await;
        }
        // the comment moved the ticket on; only a ticket still as left is changed
        let fresh = self.tickets.get(&text).await?;
        let record = fresh.view.ticket.record();
        let counted = Aborted::Counted { attempt, last };
        if !standing(record, request.agent, request.standing) {
            return Ok(if before.is_some() {
                counted
            } else {
                Aborted::Postponed
            });
        }
        let changed = if last {
            let patch = TicketPatch {
                status: Some(String::from("blocked")),
                blocked_reason: Some(Some(format!(
                    "Automatic implementation aborted {attempt} times - please look at it"
                ))),
                assignee: Some(None),
                ..TicketPatch::default()
            };
            let options = WriteOptions {
                force: true,
                ..WriteOptions::default()
            };
            self.tickets
                .update(&text, &patch, actor, record.version, options)
                .await
                .map(|_| ())
        } else if request.standing == Standing::Claimed {
            self.tickets
                .release(&text, Some("planned"), actor, Some(record.version))
                .await
                .map(|_| ())
        } else {
            Ok(())
        };
        match changed {
            Ok(()) => Ok(counted),
            // a person changed the ticket first: look again on the next pass
            Err(error) if error.kind() == ErrorKind::Conflict => Ok(Aborted::Postponed),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_comment_names_attempt_reason_and_what_follows() {
        let body = body_of(
            1,
            3,
            "time limit of 90 min reached, run stopped.",
            false,
            None,
        );
        assert_eq!(
            body,
            "[dispatch] abort: attempt 1/3: time limit of 90 min reached, run stopped.\nThe ticket goes back to planned."
        );
        let last = body_of(2, 2, "claude exited with code 2", true, None);
        assert!(last.starts_with("[dispatch] abort: attempt 2/2: claude exited with code 2.\n"));
        assert!(last.ends_with("the ticket goes to blocked."));
    }
}
