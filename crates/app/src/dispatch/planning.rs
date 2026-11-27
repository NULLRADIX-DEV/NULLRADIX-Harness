//! Plan runs on tickets: start by hand or by the pass, evaluate, clean up the worktree.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    AbortCategory, DispatchOutcome, DispatcherConfig, IdleReason, LabelOverrides,
    PLAN_AGENT_PREFIX, Run, RunKind, RunStatus, RunTrigger, SystemMarker, TicketRecord,
    TicketStatus, Timestamp, idle_reason, planning_candidate,
};
use parking_lot::Mutex;
use uuid::Uuid;

use super::accounts::{AccountPick, AccountPicker, AccountRequest, PickMode};
use super::detached::{
    DetachedCreate, DetachedCreated, DetachedPurpose, DetachedRemove, DetachedRemoved,
    DetachedWorktreePort, detached_busy, detached_root,
};
use super::local_time::LocalTime;
use super::log::{DispatchLog, LogLevel};
use super::outcome::never_spawned;
use super::pass::{DispatchPass, PassContext, PassReport, PassStep, counts_for_wip};
use super::pause::{PauseHook, PauseKind};
use super::research::{BoardRoot, board_roots, same_folder};
use super::runs::{DispatchRuns, StartDispatchRun};
use super::start::TicketStarter;
use super::state::DispatcherStateService;
use super::tracking::{AutoRunRepository, TrackKind, Tracked};
use crate::boards::BoardRepository;
use crate::comments::CommentService;
use crate::dispatcher_config::DispatcherConfigService;
use crate::dispatcher_prompts::DispatcherPromptService;
use crate::repositories::RunnerService;
use crate::runs::{RunFilter, RunService};
use crate::tickets::{TicketDetail, TicketFilter, TicketScope, TicketService};
use crate::{AppError, Clock, ErrorKind};

/// After an environment or repository fault the automation waits this long.
pub const RETRY_MINUTES: i64 = 30;

// reserved rows of the planning tracking; no ticket has an id like these
const SINCE_ROW: Uuid = Uuid::from_u128(0x019a_0000_0000_7000_8000_0000_0000_0201);
const RETRY_ROW: Uuid = Uuid::from_u128(0x019a_0000_0000_7000_8000_0000_0000_0202);

/// Faults of the ticket itself: the automation marks the ticket as tried and goes on.
const TICKET_ERRORS: [&str; 6] = [
    "plan-no-repo",
    "plan-not-possible",
    "plan-waiting-question",
    "plan-claimed",
    "plan-running",
    "ticket-not-found",
];

/// Who asks for the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanTrigger {
    Manual,
    Auto,
}

fn paused(message: String) -> AppError {
    AppError::conflict("dispatch-paused", message)
}

fn running(key: &str) -> AppError {
    AppError::conflict(
        "plan-running",
        format!("a planning of {key} is running already"),
    )
}

fn not_possible(message: String) -> AppError {
    AppError::invalid("plan-not-possible", message)
}

fn no_repo(key: &str) -> AppError {
    AppError::invalid(
        "plan-no-repo",
        format!(
            "{key} is not on a board with a repository - only tickets of boards with a repository on this machine are planned"
        ),
    )
}

/// Everything the plan runs work with.
pub struct PlanningParts {
    pub tickets: Arc<TicketService>,
    pub comments: Arc<CommentService>,
    pub boards: Arc<dyn BoardRepository>,
    pub runners: Arc<RunnerService>,
    pub runs: Arc<RunService>,
    pub dispatch: Arc<DispatchRuns>,
    pub prompts: Arc<DispatcherPromptService>,
    pub detached: Arc<dyn DetachedWorktreePort>,
    pub tracking: Arc<dyn AutoRunRepository>,
    pub accounts: Arc<AccountPicker>,
    pub state: Arc<DispatcherStateService>,
    pub config: Arc<DispatcherConfigService>,
    pub pass: Arc<DispatchPass>,
    pub local: Arc<dyn LocalTime>,
    pub starter: Arc<TicketStarter>,
    pub log: Arc<DispatchLog>,
    pub clock: Arc<dyn Clock>,
}

pub struct PlanningRuns {
    parts: PlanningParts,
    /// Tickets whose run is being started; a second start of one fails.
    starting: Mutex<HashSet<String>>,
}

impl std::fmt::Debug for PlanningRuns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanningRuns").finish_non_exhaustive()
    }
}

/// Gives the place of a start back, also when the start fails or stops early.
struct Starting<'a> {
    set: &'a Mutex<HashSet<String>>,
    key: String,
}

impl Drop for Starting<'_> {
    fn drop(&mut self) {
        self.set.lock().remove(&self.key);
    }
}

/// The agent a plan run works under.
fn agent_of(key: &str) -> String {
    format!("{PLAN_AGENT_PREFIX}{key}")
}

impl PlanningRuns {
    #[must_use]
    pub fn new(parts: PlanningParts) -> Arc<Self> {
        Arc::new(Self {
            parts,
            starting: Mutex::default(),
        })
    }

    /// "Plan" on a ticket: starts at once, whatever the switch, the window or the opt-in say.
    pub async fn start_manual(&self, key: &str) -> Result<Run, AppError> {
        self.parts
            .pass
            .exclusive(|| async {
                let config = self.parts.config.get().await?.config;
                let now = self.parts.clock.now();
                let state = self.parts.state.get().await?;
                if state.paused_until(now).is_some() {
                    return Err(paused(String::from(
                        "the dispatcher is paused - resume it or wait for the pause to end",
                    )));
                }
                self.start(key, PlanTrigger::Manual, &config).await
            })
            .await
    }

    /// Plan runs that are under way or wait for their evaluation.
    async fn open_runs(&self) -> Result<Vec<Run>, AppError> {
        let filter = RunFilter {
            kind: Some(RunKind::Plan),
            ..RunFilter::default()
        };
        let mut runs = self.parts.runs.find(&filter, 500).await?;
        runs.retain(|run| run.outcome().is_none());
        Ok(runs)
    }

    async fn board_roots(&self) -> Result<Vec<BoardRoot>, AppError> {
        board_roots(self.parts.boards.as_ref(), &self.parts.runners).await
    }

    fn choice(config: &DispatcherConfig) -> String {
        config.planning.profile.clone().unwrap_or_else(|| {
            let own = "planning";
            if config.profile(own).is_some() {
                own.to_owned()
            } else {
                config.default_profile.clone()
            }
        })
    }

    /// Whether the ticket may be planned now: its status, its questions and its runs.
    async fn check(&self, detail: &TicketDetail) -> Result<(), AppError> {
        let record = detail.view.ticket.record();
        let key = record.key.to_string();
        if matches!(
            record.status,
            TicketStatus::InProgress | TicketStatus::Done | TicketStatus::NotNeeded
        ) {
            return Err(not_possible(format!(
                "{key} is {} - only tickets in backlog, in planning, planned or blocked are planned",
                record.status.as_str()
            )));
        }
        if !detail.view.open_questions.is_empty() {
            let keys: Vec<String> = detail
                .view
                .open_questions
                .iter()
                .map(|question| question.key.to_string())
                .collect();
            return Err(AppError::invalid(
                "plan-waiting-question",
                format!(
                    "{key} waits on question {} - decide first, then plan",
                    keys.join(", ")
                ),
            ));
        }
        let filter = RunFilter {
            subject_key: Some(key.clone()),
            ..RunFilter::default()
        };
        let runs = self.parts.runs.find(&filter, 20).await?;
        let kinds: Vec<RunKind> = runs
            .iter()
            .filter_map(|run| {
                let kind = run.dispatch()?.kind;
                let under_way =
                    run.status().is_active() || (kind == RunKind::Plan && run.outcome().is_none());
                under_way.then_some(kind)
            })
            .collect();
        if kinds.contains(&RunKind::Plan) {
            return Err(running(&key));
        }
        if let Some(kind) = kinds.first() {
            return Err(not_possible(format!(
                "another run of {key} is under way ({})",
                kind.as_str()
            )));
        }
        Ok(())
    }

    /// Starts the planning of a ticket in a detached worktree at its checkout's HEAD.
    async fn start(
        &self,
        key: &str,
        trigger: PlanTrigger,
        config: &DispatcherConfig,
    ) -> Result<Run, AppError> {
        let parsed = nullradix_domain::TicketKey::parse(key)
            .map_err(|error| crate::tickets::invalid(&error))?;
        let text = parsed.to_string();
        if !self.starting.lock().insert(text.clone()) {
            return Err(running(&text));
        }
        let _place = Starting {
            set: &self.starting,
            key: text.clone(),
        };
        let detail = self.parts.tickets.get(&text).await?;
        self.check(&detail).await?;
        let record = detail.view.ticket.record();
        let roots = self.board_roots().await?;
        let root = roots
            .iter()
            .find(|found| found.board == record.board)
            .map(|found| found.root.clone())
            .ok_or_else(|| no_repo(&text))?;
        let profile = Self::choice(config);
        let chosen = match self
            .parts
            .accounts
            .pick(&AccountRequest::default(), config, PickMode::Start)
            .await?
        {
            AccountPick::Picked(chosen) => chosen,
            AccountPick::Waiting { reason, .. } => {
                return Err(paused(format!("no Claude account is free - {reason}")));
            }
        };
        let agent = agent_of(&text);
        let actor = self.parts.starter.actor().await?;
        self.parts
            .tickets
            .claim_planning(&text, &agent, actor, false)
            .await?;
        let launched = self
            .launch(&text, &root, trigger, &profile, chosen.account, &agent)
            .await;
        let (run, path) = match launched {
            Ok(found) => found,
            Err(error) => {
                self.release(&text, &agent).await;
                return Err(error);
            }
        };
        self.parts.accounts.record_start(run.id(), &chosen).await;
        self.note_start(record, trigger, &profile, &path, &run)
            .await;
        Ok(run)
    }

    /// The worktree and the run in it; a run that cannot start takes its worktree with it.
    async fn launch(
        &self,
        text: &str,
        root: &str,
        trigger: PlanTrigger,
        profile: &str,
        account: nullradix_domain::Id<nullradix_domain::Account>,
        agent: &str,
    ) -> Result<(Run, String), AppError> {
        let created = self
            .parts
            .detached
            .create(&DetachedCreate {
                root: root.to_owned(),
                purpose: DetachedPurpose::Plan,
                subject: text.to_owned(),
                at: String::from("HEAD"),
            })
            .await?;
        let path = match created {
            DetachedCreated::Created { path, .. } => path,
            DetachedCreated::Busy { path } => {
                return Err(detached_busy(DetachedPurpose::Plan, &path));
            }
        };
        let started = match self.parts.prompts.plan_prompt(text).await {
            Ok(prompt) => {
                self.parts
                    .dispatch
                    .start(StartDispatchRun {
                        kind: RunKind::Plan,
                        trigger: match trigger {
                            PlanTrigger::Manual => RunTrigger::Manual,
                            PlanTrigger::Auto => RunTrigger::Dispatcher,
                        },
                        subject: text.to_owned(),
                        prompt,
                        cwd: PathBuf::from(&path),
                        profile: Some(profile.to_owned()),
                        account: Some(account),
                        extra_tools: Vec::new(),
                        agent: Some(agent.to_owned()),
                        overrides: LabelOverrides::default(),
                    })
                    .await
            }
            Err(error) => Err(error),
        };
        match started {
            Ok(run) => Ok((run, path)),
            Err(error) => {
                self.remove(root, &path).await;
                Err(error)
            }
        }
    }

    /// Gives the planning claim back; what goes wrong is only logged.
    async fn release(&self, key: &str, agent: &str) {
        let released = match self.parts.starter.actor().await {
            Ok(actor) => {
                self.parts
                    .tickets
                    .release_planning(key, agent, actor, false)
                    .await
            }
            Err(error) => Err(error),
        };
        if let Err(error) = released {
            self.parts
                .log
                .error(
                    &format!("planning claim of {key} not released: {}", error.message()),
                    Some(key),
                )
                .await;
        }
    }

    /// Counts the start and writes it to the log; neither may undo a run that started.
    async fn note_start(
        &self,
        record: &TicketRecord,
        trigger: PlanTrigger,
        profile: &str,
        path: &str,
        run: &Run,
    ) {
        let now = self.parts.clock.now();
        let auto = trigger == PlanTrigger::Auto;
        let text = record.key.to_string();
        if let Err(error) = self
            .parts
            .tracking
            .started(TrackKind::Planning, record.id.as_uuid(), now, auto)
            .await
        {
            self.parts
                .log
                .error(
                    &format!("planning of {text} not counted: {}", error.message()),
                    Some(&text),
                )
                .await;
        }
        let how = if auto { "automatically" } else { "by hand" };
        self.parts
            .log
            .write(
                LogLevel::Info,
                &format!("planning started {text} ({how}) in {path} [{profile}]"),
                Some(&text),
                Some(run.id()),
            )
            .await;
    }

    /// Removes the worktree; what stays is reported and left to the orphan step.
    async fn remove(&self, root: &str, path: &str) {
        let removed = self
            .parts
            .detached
            .remove(&DetachedRemove {
                root: root.to_owned(),
                purpose: DetachedPurpose::Plan,
                path: path.to_owned(),
            })
            .await;
        match removed {
            Ok(DetachedRemoved::Removed) => {}
            Ok(DetachedRemoved::Kept(reason)) => self.parts.log.error(&reason, None).await,
            Err(error) => {
                self.parts
                    .log
                    .error(&format!("cannot remove {path}: {}", error.message()), None)
                    .await;
            }
        }
    }

    /// Settles a finished plan run: planned, asked, empty or gone; the worktree always goes.
    pub async fn settle(
        &self,
        run: &Run,
        config: &DispatcherConfig,
        pauses: &dyn PauseHook,
    ) -> Result<DispatchOutcome, AppError> {
        let Some(dispatch) = run.dispatch() else {
            return Ok(DispatchOutcome::Other);
        };
        let key = dispatch.subject_key.clone().unwrap_or_default();
        let agent = dispatch.agent.clone().unwrap_or_else(|| agent_of(&key));
        let outcome = self.outcome(run, &key, &agent, config, pauses).await;
        if let Some((root, path)) = detached_root(DetachedPurpose::Plan, &dispatch.cwd) {
            self.remove(&root, &path).await;
        }
        outcome
    }

    /// A run that never spawned leaves no comment, so the start is tried again by the next pass.
    async fn log_never_spawned(&self, run: &Run, key: &str) -> DispatchOutcome {
        self.parts
            .log
            .write(
                LogLevel::Warn,
                &format!("planning of {key} never started: {}", why_empty(run)),
                Some(key),
                Some(run.id()),
            )
            .await;
        DispatchOutcome::StartFailed
    }

    async fn outcome(
        &self,
        run: &Run,
        key: &str,
        agent: &str,
        config: &DispatcherConfig,
        pauses: &dyn PauseHook,
    ) -> Result<DispatchOutcome, AppError> {
        let detail = match self.parts.tickets.get(key).await {
            Ok(detail) => detail,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(DispatchOutcome::Gone),
            Err(error) => return Err(error),
        };
        let record = detail.view.ticket.record();
        if planned(record, agent, run.created_at()) {
            self.parts
                .log
                .write(
                    LogLevel::Info,
                    &format!("plan of {key} saved"),
                    Some(key),
                    Some(run.id()),
                )
                .await;
            return Ok(DispatchOutcome::Planned);
        }
        self.release(key, agent).await;
        if !detail.view.open_questions.is_empty() {
            let keys: Vec<String> = detail
                .view
                .open_questions
                .iter()
                .map(|question| question.key.to_string())
                .collect();
            self.parts
                .log
                .write(
                    LogLevel::Info,
                    &format!("planning of {key} waits on question {}", keys.join(", ")),
                    Some(key),
                    Some(run.id()),
                )
                .await;
            return Ok(DispatchOutcome::Question);
        }
        if never_spawned(run) {
            return Ok(self.log_never_spawned(run, key).await);
        }
        let kind = match run
            .result()
            .and_then(|result| result.abort.as_ref())
            .map(|abort| abort.category)
        {
            Some(AbortCategory::UsageLimit) => Some(PauseKind::UsageLimit),
            Some(AbortCategory::StartFailed) => Some(PauseKind::ClaudeMissing),
            _ => None,
        };
        let outcome_of = |kind: PauseKind| match kind {
            PauseKind::UsageLimit => DispatchOutcome::PauseLimit,
            PauseKind::ClaudeMissing => DispatchOutcome::PauseClaudeMissing,
        };
        // a pass that stopped after the comment is picked up where it was
        if commented(&detail, run) {
            return Ok(kind.map_or(DispatchOutcome::Other, outcome_of));
        }
        let actor = self.parts.starter.actor().await?;
        if let Some(kind) = kind
            && let Some(text) = pauses.paused_plan(run, record, kind, config).await?
        {
            let body = format!("{text}\n\nRun: {}", run.id());
            self.parts
                .comments
                .add_system(
                    &record.key,
                    &body,
                    Some(SystemMarker::PlanEmpty),
                    Some(run.id()),
                    actor,
                )
                .await?;
            return Ok(outcome_of(kind));
        }
        let why = why_empty(run);
        let body = format!(
            "[dispatch] plan-empty: {why}.\nNo automatic retry - \"Plan\" on the ticket starts a new one.\n\nRun: {}",
            run.id()
        );
        self.parts
            .comments
            .add_system(
                &record.key,
                &body,
                Some(SystemMarker::PlanEmpty),
                Some(run.id()),
                actor,
            )
            .await?;
        self.parts
            .log
            .write(
                LogLevel::Info,
                &format!("planning of {key} without result: {why}"),
                Some(key),
                Some(run.id()),
            )
            .await;
        Ok(DispatchOutcome::Other)
    }

    /// Removes plan worktrees no run belongs to any more.
    pub async fn remove_orphans(&self, report: &mut PassReport) -> Result<(), AppError> {
        let open = self.open_runs().await?;
        let busy: Vec<String> = open
            .iter()
            .filter_map(Run::dispatch)
            .map(|dispatch| dispatch.cwd.clone())
            .collect();
        let mut seen = HashSet::new();
        for found in self.board_roots().await? {
            if !seen.insert(found.root.to_lowercase()) {
                continue;
            }
            let listed = self
                .parts
                .detached
                .list(&found.root, DetachedPurpose::Plan)
                .await;
            let listed = match listed {
                Ok(listed) => listed,
                Err(error) => {
                    report
                        .errors
                        .push(format!("plan worktrees: {}", error.message()));
                    continue;
                }
            };
            for path in listed {
                if busy.iter().any(|folder| same_folder(folder, &path)) {
                    continue;
                }
                self.remove(&found.root, &path).await;
                self.parts
                    .log
                    .info(&format!("orphaned plan worktree {path} removed"), None)
                    .await;
            }
        }
        Ok(())
    }

    async fn tracking_row(&self, row: Uuid) -> Result<Option<Tracked>, AppError> {
        self.parts.tracking.get(TrackKind::Planning, row).await
    }

    /// Turns the automation's clock on and off with its switch: only newer tickets are taken up.
    async fn follow_switch(&self, on: bool, now: Timestamp) -> Result<(), AppError> {
        let since = self.tracking_row(SINCE_ROW).await?;
        match (on, since) {
            (true, None) => {
                let row = Tracked {
                    auto_at: Some(now),
                    ..Tracked::default()
                };
                self.parts
                    .tracking
                    .put(TrackKind::Planning, SINCE_ROW, row)
                    .await?;
                self.parts
                    .tracking
                    .clear(TrackKind::Planning, RETRY_ROW)
                    .await
            }
            (false, Some(_)) => {
                self.parts
                    .tracking
                    .clear(TrackKind::Planning, SINCE_ROW)
                    .await
            }
            _ => Ok(()),
        }
    }

    async fn retry_at(&self) -> Result<Option<Timestamp>, AppError> {
        Ok(self
            .tracking_row(RETRY_ROW)
            .await?
            .and_then(|row| row.auto_at))
    }

    /// An environment fault pauses the automation for a while.
    async fn suspend(
        &self,
        now: Timestamp,
        ticket: &str,
        error: &AppError,
        report: &mut PassReport,
    ) -> Result<(), AppError> {
        let until = Timestamp::from_millis(now.as_millis() + RETRY_MINUTES * 60_000);
        let row = Tracked {
            auto_at: Some(until),
            ..Tracked::default()
        };
        self.parts
            .tracking
            .put(TrackKind::Planning, RETRY_ROW, row)
            .await?;
        let message = format!(
            "planning automation paused for {RETRY_MINUTES} minutes (at {ticket}): {}",
            error.message()
        );
        self.parts.log.error(&message, Some(ticket)).await;
        report.errors.push(message);
        Ok(())
    }

    /// Tickets that need planning since the switch, on boards with a checkout here.
    async fn candidates(
        &self,
        roots: &[BoardRoot],
        since: Timestamp,
        now: Timestamp,
    ) -> Result<Vec<TicketRecord>, AppError> {
        let mut found = Vec::new();
        for root in roots {
            let filter = TicketFilter {
                scope: TicketScope {
                    project: None,
                    board: Some(root.board),
                },
                needs_planning: Some(true),
                ..TicketFilter::default()
            };
            for view in self.parts.tickets.list(&filter).await? {
                let record = view.ticket.record();
                if view.open_questions.is_empty()
                    && planning_candidate(record, now)
                    && record.created_at.max(record.updated_at) >= since
                {
                    found.push(record.clone());
                }
            }
        }
        found.sort_by_key(|record| (record.priority.rank(), record.key.number()));
        Ok(found)
    }

    /// Free places of the ticket limit: planning runs do not count themselves.
    async fn free_ticket_places(&self, config: &DispatcherConfig) -> Result<i64, AppError> {
        let active = self.parts.runs.active().await?;
        let busy = active.iter().filter(|run| counts_for_wip(run)).count()
            + self.parts.starter.launching();
        Ok(config
            .wip
            .saturating_sub(i64::try_from(busy).unwrap_or(i64::MAX)))
    }

    async fn start_step(
        &self,
        context: &PassContext,
        report: &mut PassReport,
    ) -> Result<(), AppError> {
        let config = &context.config;
        self.follow_switch(config.planning.auto, context.now)
            .await?;
        if !config.planning.auto {
            return Ok(());
        }
        // a pause or the time window holds planning back as it does tickets; the switch does not
        let state = self.parts.state.get().await?;
        let minute = self.parts.local.minute_of_day(context.now).await;
        let held = idle_reason(
            true,
            state.paused_until(context.now),
            config.window.as_ref(),
            context.now,
            minute,
        );
        if matches!(
            held,
            Some(IdleReason::Paused(_) | IdleReason::OutsideWindow { .. })
        ) {
            return Ok(());
        }
        let Some(since) = self
            .tracking_row(SINCE_ROW)
            .await?
            .and_then(|row| row.auto_at)
        else {
            return Ok(());
        };
        if self
            .retry_at()
            .await?
            .is_some_and(|until| until > context.now)
        {
            return Ok(());
        }
        // planned only while the limit of tickets has room
        if self.free_ticket_places(config).await? <= 0 {
            return Ok(());
        }
        let mut free =
            config.planning.wip - i64::try_from(self.open_runs().await?.len()).unwrap_or(0);
        if free <= 0 {
            return Ok(());
        }
        let roots = self.board_roots().await?;
        let tried = self.parts.tracking.all(TrackKind::Planning).await?;
        for record in self.candidates(&roots, since, context.now).await? {
            if free <= 0 {
                break;
            }
            let subject = record.id.as_uuid();
            if tried.contains_key(&subject) {
                continue;
            }
            let key = record.key.to_string();
            match self.start(&key, PlanTrigger::Auto, config).await {
                Ok(_) => {
                    report.planned.push(key);
                    free -= 1;
                    self.parts
                        .tracking
                        .clear(TrackKind::Planning, RETRY_ROW)
                        .await?;
                }
                Err(error) if TICKET_ERRORS.contains(&error.code()) => {
                    self.parts
                        .tracking
                        .mark(TrackKind::Planning, subject, context.now)
                        .await?;
                    report.errors.push(format!("{key}: {}", error.message()));
                }
                // a busy account or a pause ends the round without blaming anything
                Err(error) if error.code() == "dispatch-paused" => break,
                Err(error) => {
                    self.suspend(context.now, &key, &error, report).await?;
                    break;
                }
            }
        }
        Ok(())
    }
}

/// Whether the agent of the run saved a plan since the run was created.
fn planned(record: &TicketRecord, agent: &str, since: Timestamp) -> bool {
    record
        .planned
        .as_ref()
        .is_some_and(|(by, at)| by == agent && *at >= since)
}

/// Whether the plan-empty comment of this run is on the ticket already.
fn commented(detail: &TicketDetail, run: &Run) -> bool {
    detail.comments.iter().any(|line| {
        let record = line.comment.record();
        record.system_marker == Some(SystemMarker::PlanEmpty) && record.run == Some(run.id())
    })
}

fn why_empty(run: &Run) -> String {
    match run.result().and_then(|result| result.abort.as_ref()) {
        Some(abort) => abort.reason.trim_end_matches('.').to_owned(),
        None if run.status() == RunStatus::Cancelled => String::from("the run was cancelled"),
        None => String::from("the run ended without saving a plan or a question"),
    }
}

/// The pass step that removes plan worktrees whose runs are gone.
#[derive(Debug)]
pub struct PlanOrphans(pub Arc<PlanningRuns>);

#[async_trait]
impl PassStep for PlanOrphans {
    async fn run(&self, _context: &PassContext, report: &mut PassReport) -> Result<(), AppError> {
        self.0.remove_orphans(report).await
    }
}

/// The pass step that starts planning of tickets that need it.
#[derive(Debug)]
pub struct StartPlanning(pub Arc<PlanningRuns>);

#[async_trait]
impl PassStep for StartPlanning {
    async fn run(&self, context: &PassContext, report: &mut PassReport) -> Result<(), AppError> {
        self.0.start_step(context, report).await
    }
}
