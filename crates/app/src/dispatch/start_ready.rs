//! The pass step that starts ready tickets up to the limit.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{Run, RunKind};

use super::candidates::Candidates;
use super::pass::{PassContext, PassReport, PassStep, counts_for_wip};
use super::start::{StartOutcome, TicketStarter};
use crate::AppError;
use crate::runs::RunService;

pub struct StartReady {
    candidates: Arc<Candidates>,
    starter: Arc<TicketStarter>,
    runs: Arc<RunService>,
}

impl std::fmt::Debug for StartReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartReady").finish_non_exhaustive()
    }
}

impl StartReady {
    #[must_use]
    pub fn new(
        candidates: Arc<Candidates>,
        starter: Arc<TicketStarter>,
        runs: Arc<RunService>,
    ) -> Self {
        Self {
            candidates,
            starter,
            runs,
        }
    }
}

/// Free places left under the limit; planning and research runs do not count.
#[must_use]
pub fn free_places(wip: i64, running: usize, setting_up: usize) -> i64 {
    let busy = i64::try_from(running + setting_up).unwrap_or(i64::MAX);
    wip.saturating_sub(busy)
}

#[async_trait]
impl PassStep for StartReady {
    async fn run(&self, context: &PassContext, report: &mut PassReport) -> Result<(), AppError> {
        if context.idle.is_some() {
            return Ok(());
        }
        // read first: a setup that ends meanwhile has its run in the list read afterwards
        let launching = self.starter.launching();
        let active = self.runs.active().await?;
        let mut free = free_places(
            context.config.wip,
            active.iter().filter(|run| counts_for_wip(run)).count(),
            launching,
        );
        // a ticket that is being planned must wait: its plan still changes
        let planning: HashSet<&str> = active
            .iter()
            .filter_map(Run::dispatch)
            .filter(|dispatch| dispatch.kind == RunKind::Plan)
            .filter_map(|dispatch| dispatch.subject_key.as_deref())
            .collect();
        let candidates = self.candidates.scan(&context.config, None).await?;
        let mut waiting = None;
        for candidate in candidates
            .iter()
            .filter(|candidate| candidate.not_ready.is_none())
        {
            let key = candidate.key().to_string();
            if planning.contains(key.as_str()) {
                continue;
            }
            if free <= 0 {
                report.queued.push(key);
                continue;
            }
            match self.starter.start(candidate, &context.config).await {
                StartOutcome::Started(_) | StartOutcome::SettingUp => {
                    report.started.push(key);
                    free -= 1;
                }
                StartOutcome::Skipped(_) => {}
                StartOutcome::WaitsForAccount(reason) => {
                    waiting.get_or_insert(format!("{key} {reason}"));
                }
                StartOutcome::Failed(message) => report.errors.push(format!("{key}: {message}")),
            }
        }
        // nothing started because no account was free: the status says why
        if report.started.is_empty() && report.idle.is_none() {
            report.idle.clone_from(&waiting);
            report.waiting = waiting;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_never_go_below_zero_meaning_none_left() {
        assert_eq!(free_places(3, 1, 1), 1);
        assert_eq!(free_places(1, 1, 0), 0);
        assert!(free_places(1, 4, 0) < 1);
        assert_eq!(free_places(0, 0, 0), 0);
    }
}
