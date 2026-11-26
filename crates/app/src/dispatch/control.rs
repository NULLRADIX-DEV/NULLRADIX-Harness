//! What the dispatcher is doing, and the controls for it: switch, pause, resume, pass now.

use std::sync::Arc;
use std::time::Duration;

use nullradix_domain::{
    Board, DispatcherConfig, DispatcherPhase, Id, Run, Timestamp, Version, choose_profile,
    idle_reason, phase,
};

use super::accounts::{AccountPick, AccountPicker, AccountRequest, PickContext, PickMode};
use super::candidates::{Candidate, Candidates};
use super::local_time::LocalTime;
use super::pass::{DispatchPass, PassReport, counts_for_wip, idle_message};
use super::planning::PlanningRuns;
use super::research::ResearchRuns;
use super::start::TicketStarter;
use super::state::DispatcherStateService;
use crate::boards::{self, BoardRepository};
use crate::dispatcher_config::DispatcherConfigService;
use crate::runs::RunService;
use crate::{AppError, Clock};
use nullradix_domain::RunKind;

/// A planned ticket with the reason the dispatcher does or does not take it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketReadiness {
    pub key: String,
    pub title: String,
    /// `ready`, `ready - waits for a free place`, or what is missing.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DispatcherStatus {
    pub enabled: bool,
    pub phase: DispatcherPhase,
    pub message: String,
    pub paused_until: Option<Timestamp>,
    pub wip: i64,
    /// Tickets being worked on: running work and merge runs and setups under way.
    pub active: usize,
    /// Running planning runs; they do not count for the limit.
    pub planning: usize,
    /// Runs of the dispatcher that have not ended.
    pub runs: Vec<Run>,
    pub tickets: Vec<TicketReadiness>,
    pub last_pass_at: Option<Timestamp>,
    pub last_errors: Vec<String>,
    /// When the scheduler passes next, if one runs.
    pub next_pass_at: Option<Timestamp>,
    pub agent_prefix: String,
    pub board: Option<Id<Board>>,
    pub version: Version,
}

/// How long a pause lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseFor {
    Minutes(u32),
    Until(Timestamp),
}

pub struct DispatcherService {
    research: Arc<ResearchRuns>,
    planning: Arc<PlanningRuns>,
    config: Arc<DispatcherConfigService>,
    state: Arc<DispatcherStateService>,
    pass: Arc<DispatchPass>,
    candidates: Arc<Candidates>,
    starter: Arc<TicketStarter>,
    accounts: Arc<AccountPicker>,
    runs: Arc<RunService>,
    boards: Arc<dyn BoardRepository>,
    clock: Arc<dyn Clock>,
    local: Arc<dyn LocalTime>,
    /// Time between passes of the scheduler; none when no scheduler runs.
    interval: Option<Duration>,
}

impl std::fmt::Debug for DispatcherService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatcherService").finish_non_exhaustive()
    }
}

/// Everything the service works with.
pub struct ControlParts {
    pub research: Arc<ResearchRuns>,
    pub planning: Arc<PlanningRuns>,
    pub config: Arc<DispatcherConfigService>,
    pub state: Arc<DispatcherStateService>,
    pub pass: Arc<DispatchPass>,
    pub candidates: Arc<Candidates>,
    pub starter: Arc<TicketStarter>,
    pub accounts: Arc<AccountPicker>,
    pub runs: Arc<RunService>,
    pub boards: Arc<dyn BoardRepository>,
    pub clock: Arc<dyn Clock>,
    pub local: Arc<dyn LocalTime>,
    pub interval: Option<Duration>,
}

impl DispatcherService {
    #[must_use]
    pub fn new(parts: ControlParts) -> Self {
        Self {
            research: parts.research,
            planning: parts.planning,
            config: parts.config,
            state: parts.state,
            pass: parts.pass,
            candidates: parts.candidates,
            starter: parts.starter,
            accounts: parts.accounts,
            runs: parts.runs,
            boards: parts.boards,
            clock: parts.clock,
            local: parts.local,
            interval: parts.interval,
        }
    }

    /// The pass, for slices that put their steps in or take its lock.
    #[must_use]
    pub const fn pass(&self) -> &Arc<DispatchPass> {
        &self.pass
    }

    /// Starts the research of an open question at once, whatever the switch or the window say.
    pub async fn start_research(&self, question: &str) -> Result<Run, AppError> {
        self.research.start_manual(question).await
    }

    /// Starts the planning of a ticket at once, whatever the switch or the window say.
    pub async fn start_planning(&self, ticket: &str) -> Result<Run, AppError> {
        self.planning.start_manual(ticket).await
    }

    async fn check_board(&self, board: Option<Id<Board>>) -> Result<(), AppError> {
        if let Some(id) = board
            && self.boards.get(id).await?.is_none()
        {
            return Err(boards::not_found(id));
        }
        Ok(())
    }

    /// Why no account is free for a ready ticket, in the words of the start; none when one is.
    async fn waits_for_account(
        &self,
        context: &PickContext,
        config: &DispatcherConfig,
        candidate: &Candidate,
    ) -> Result<Option<String>, AppError> {
        let record = candidate.ticket().record();
        let choice = choose_profile(config, &record.labels, record.priority, &record.plan);
        let request = AccountRequest::for_ticket(config, &choice, &record.labels);
        Ok(
            match self
                .accounts
                .pick_in(context, &request, config, PickMode::Preview)
                .await?
            {
                AccountPick::Waiting { reason, .. } => Some(format!("waits - {reason}")),
                AccountPick::Picked(_) => None,
            },
        )
    }

    pub async fn status(&self, board: Option<Id<Board>>) -> Result<DispatcherStatus, AppError> {
        self.check_board(board).await?;
        let config = self.config.get().await?.config;
        let state = self.state.get().await?;
        let now = self.clock.now();
        let enabled = state.enabled(config.enabled);
        let paused_until = state.paused_until(now);
        let minute = self.local.minute_of_day(now).await;
        let idle = idle_reason(enabled, paused_until, config.window.as_ref(), now, minute);
        // read first: a setup that ends meanwhile has its run in the list read afterwards
        let launching = self.starter.launching();
        let runs: Vec<Run> = self
            .runs
            .active()
            .await?
            .into_iter()
            .filter(|run| run.dispatch().is_some())
            .collect();
        let working = runs.iter().filter(|run| counts_for_wip(run)).count();
        let active = working + launching;
        let planning = runs
            .iter()
            .filter_map(Run::dispatch)
            .filter(|dispatch| dispatch.kind == RunKind::Plan)
            .count();
        let mut free = config.wip - i64::try_from(active).unwrap_or(i64::MAX);
        let mut tickets = Vec::new();
        let context = self.accounts.load(None, PickMode::Preview).await?;
        for candidate in self.candidates.scan(&config, board).await? {
            let record = candidate.ticket().record();
            let reason = match &candidate.not_ready {
                Some(reason) => reason.to_string(),
                // with no account free the ticket waits without taking a place
                None => match self
                    .waits_for_account(&context, &config, &candidate)
                    .await?
                {
                    Some(waits) => waits,
                    None if free > 0 => {
                        free -= 1;
                        String::from("ready")
                    }
                    None => String::from("ready - waits for a free place"),
                },
            };
            tickets.push(TicketReadiness {
                key: record.key.to_string(),
                title: record.title.as_str().to_owned(),
                reason,
            });
        }
        let last = self.pass.last_report().or_else(|| {
            state
                .record()
                .last_pass
                .as_deref()
                .and_then(PassReport::from_json)
        });
        let message = match &idle {
            Some(reason) => idle_message(reason),
            None if active > 0 => format!("{active} ticket(s) in progress"),
            // the last pass started nothing because no account was free
            None => last
                .as_ref()
                .and_then(|report| report.waiting.clone())
                .unwrap_or_else(|| String::from("Idle")),
        };
        let last_pass_at = last
            .as_ref()
            .map(|report| Timestamp::from_millis(report.at));
        let next_pass_at = last_pass_at
            .zip(self.interval)
            .and_then(|(at, interval)| at.checked_add(interval));
        Ok(DispatcherStatus {
            enabled,
            phase: phase(idle.as_ref(), active),
            message,
            paused_until,
            wip: config.wip,
            active,
            planning,
            runs,
            tickets,
            last_pass_at,
            last_errors: last.map_or_else(Vec::new, |report| report.errors),
            next_pass_at,
            agent_prefix: config.agent_prefix,
            board,
            version: state.record().version,
        })
    }

    /// Turns starting new tickets on or off, whatever the configuration says.
    pub async fn set_enabled(&self, enabled: bool) -> Result<DispatcherStatus, AppError> {
        self.state.set_enabled(Some(enabled)).await?;
        self.status(None).await
    }

    pub async fn pause(&self, how: PauseFor) -> Result<DispatcherStatus, AppError> {
        match how {
            PauseFor::Minutes(minutes) => self.state.pause_for(minutes).await?,
            PauseFor::Until(until) => self.state.pause_until(until).await?,
        };
        self.status(None).await
    }

    pub async fn resume(&self) -> Result<DispatcherStatus, AppError> {
        self.state.resume().await?;
        self.accounts.pauses().resume_all().await?;
        self.status(None).await
    }

    /// Runs one pass now, or joins the one that runs; a pass serves every board.
    pub async fn run_pass(&self) -> PassReport {
        self.pass.run().await
    }
}
