//! The pass step that gives back claims whose agent shows no sign of life.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{Run, SystemMarker, TicketStatus};

use super::pass::{PassContext, PassReport, PassStep};
use super::start::TicketStarter;
use crate::AppError;
use crate::comments::CommentService;
use crate::runs::RunService;
use crate::tickets::{TicketFilter, TicketService};

const MINUTE_MS: i64 = 60_000;
/// A claim on a planned ticket without a run is given back after this long, as in legacy.
pub const PLANNED_CLAIM_STALE_MINUTES: i64 = 5;

pub struct ReleaseStale {
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
    runs: Arc<RunService>,
    starter: Arc<TicketStarter>,
}

impl std::fmt::Debug for ReleaseStale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReleaseStale").finish_non_exhaustive()
    }
}

impl ReleaseStale {
    #[must_use]
    pub fn new(
        tickets: Arc<TicketService>,
        comments: Arc<CommentService>,
        runs: Arc<RunService>,
        starter: Arc<TicketStarter>,
    ) -> Self {
        Self {
            tickets,
            comments,
            runs,
            starter,
        }
    }
}

/// How long a claim may stay without a run and without news, by the status of its ticket.
#[must_use]
pub const fn stale_limit_ms(status: TicketStatus, stale_minutes: i64) -> Option<i64> {
    match status {
        TicketStatus::Planned => Some(PLANNED_CLAIM_STALE_MINUTES * MINUTE_MS),
        TicketStatus::InProgress => Some(stale_minutes * MINUTE_MS),
        _ => None,
    }
}

#[async_trait]
impl PassStep for ReleaseStale {
    async fn run(&self, context: &PassContext, report: &mut PassReport) -> Result<(), AppError> {
        let config = &context.config;
        let actor = self.starter.actor().await?;
        // read first: a setup that ends meanwhile has its run in the list read afterwards
        let launching = self.starter.launching_keys();
        let active = self.runs.active().await?;
        let with_run: HashSet<&str> = active
            .iter()
            .filter_map(Run::dispatch)
            .filter_map(|dispatch| dispatch.subject_key.as_deref())
            .collect();
        for view in self.tickets.list(&TicketFilter::default()).await? {
            let record = view.ticket.record();
            let key = record.key.to_string();
            let Some(assignee) = record.assignee.as_deref() else {
                continue;
            };
            if config.agent_prefix.is_empty()
                || !assignee.starts_with(&config.agent_prefix)
                || with_run.contains(key.as_str())
                || launching.contains(&key)
            {
                continue;
            }
            let Some(limit) = stale_limit_ms(record.status, config.stale_minutes) else {
                continue;
            };
            let idle = context.now.as_millis() - record.updated_at.as_millis();
            if idle <= limit {
                continue;
            }
            let was = record.status;
            match self
                .tickets
                .release(&key, Some("planned"), actor, Some(record.version))
                .await
            {
                Ok(_) => {
                    report.released.push(key.clone());
                    if was == TicketStatus::InProgress {
                        let body = format!(
                            "[dispatch] abort: no sign of life for more than {} min (the worker probably crashed or the machine restarted)",
                            config.stale_minutes
                        );
                        let _ = self
                            .comments
                            .add_system(&record.key, &body, SystemMarker::Abort, None, actor)
                            .await;
                    }
                }
                Err(error) => report.errors.push(format!("{key}: {}", error.message())),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_claims_have_their_own_short_limit() {
        assert_eq!(
            stale_limit_ms(TicketStatus::Planned, 150),
            Some(5 * MINUTE_MS)
        );
        assert_eq!(
            stale_limit_ms(TicketStatus::InProgress, 150),
            Some(150 * MINUTE_MS)
        );
        assert_eq!(stale_limit_ms(TicketStatus::Blocked, 150), None);
    }
}
