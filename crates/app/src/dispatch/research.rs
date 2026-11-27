//! Research runs on open questions: start by hand or by the pass, evaluate, clean up the worktree.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    AUTOPILOT_PREFIX, AbortCategory, Board, DispatchOutcome, DispatcherConfig, Id, IdleReason,
    LabelOverrides, ProfileChoice, ProfileReason, Project, QuestionKey, QuestionOption,
    QuestionRecord, QuestionState, RESEARCH_AGENT_PREFIX, Run, RunKind, RunStatus, RunTrigger,
    SystemMarker, TicketRecord, TicketStatus, Timestamp, escalate, idle_reason,
};
use parking_lot::Mutex;
use uuid::Uuid;

use super::accounts::{AccountPick, AccountPicker, AccountRequest, Chosen, PickMode};
use super::detached::{
    DetachedCreate, DetachedCreated, DetachedPurpose, DetachedRemove, DetachedRemoved,
    DetachedWorktreePort, detached_busy, detached_root,
};
use super::local_time::LocalTime;
use super::log::{DispatchLog, LogLevel};
use super::outcome::never_spawned;
use super::pass::{DispatchPass, PassContext, PassReport, PassStep};
use super::pause::{PauseHook, PauseKind};
use super::runs::{DispatchRuns, StartDispatchRun};
use super::start::TicketStarter;
use super::state::DispatcherStateService;
use super::tracking::{AutoRunRepository, TrackKind, Tracked};
use crate::boards::BoardRepository;
use crate::comments::CommentService;
use crate::dispatcher_config::DispatcherConfigService;
use crate::dispatcher_prompts::DispatcherPromptService;
use crate::notifier::{Notice, Notifier};
use crate::questions::{QuestionDetail, QuestionFilter, QuestionService, QuestionView};
use crate::repositories::RunnerService;
use crate::runs::{RunFilter, RunService};
use crate::tickets::{TicketPatch, TicketService, WriteOptions};
use crate::{AppError, Clock, ErrorKind};

/// After an environment or repository fault the automation waits this long.
pub const RETRY_MINUTES: i64 = 30;
/// Researches without a clear recommendation the autopilot tries per question.
pub const AUTOPILOT_ATTEMPTS: u32 = 2;
/// Room between the clock of the run start and the one of the research write.
const CLOCK_SKEW_MS: i64 = 60_000;

// reserved rows of the research tracking; no question has a nil-like id
const SINCE_ROW: Uuid = Uuid::from_u128(0x019a_0000_0000_7000_8000_0000_0000_0101);
const RETRY_ROW: Uuid = Uuid::from_u128(0x019a_0000_0000_7000_8000_0000_0000_0102);
const DAY_ROW: Uuid = Uuid::from_u128(0x019a_0000_0000_7000_8000_0000_0000_0103);

/// Faults of the question itself: the automation marks the question as done and goes on.
const QUESTION_ERRORS: [&str; 3] = [
    "board-repo-missing",
    "question-not-open",
    "question-not-found",
];

/// Who asks for the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResearchTrigger {
    Manual,
    Auto,
}

fn paused(message: String) -> AppError {
    AppError::conflict("dispatch-paused", message)
}

fn not_open(key: &QuestionKey) -> AppError {
    AppError::invalid(
        "question-not-open",
        format!("question {key} is not open - only open questions are researched"),
    )
}

fn running(key: &QuestionKey) -> AppError {
    AppError::conflict(
        "research-running",
        format!("a research of question {key} is running already"),
    )
}

fn no_repo(key: &QuestionKey) -> AppError {
    AppError::invalid(
        "board-repo-missing",
        format!(
            "question {key} has no board with a repository - link it to a ticket of such a board or set the repository of a board of its project"
        ),
    )
}

/// The board a question can be researched in, with its checkout on this machine.
#[derive(Debug, Clone)]
pub(super) struct BoardRoot {
    pub(super) board: Id<Board>,
    pub(super) project: Id<Project>,
    created_at: Timestamp,
    pub(super) root: String,
}

/// Boards with a repository whose checkout exists on this machine, oldest first.
pub(super) async fn board_roots(
    boards: &dyn BoardRepository,
    runners: &RunnerService,
) -> Result<Vec<BoardRoot>, AppError> {
    let local = runners.local().id();
    let mut found = Vec::new();
    for summary in boards.list(None).await? {
        let board = &summary.board;
        let Some(repository) = board.repository() else {
            continue;
        };
        let Some(root) = runners.root(local, repository).await? else {
            continue;
        };
        let exists = runners
            .folders_exist(local, std::slice::from_ref(&root))
            .await?;
        if exists.first().copied().unwrap_or(false) {
            found.push(BoardRoot {
                board: board.id(),
                project: board.project(),
                created_at: board.created_at(),
                root,
            });
        }
    }
    found.sort_by_key(|found| (found.created_at, found.board.as_uuid()));
    Ok(found)
}

/// Everything the research runs work with.
pub struct ResearchParts {
    pub questions: Arc<QuestionService>,
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
    pub notifier: Arc<dyn Notifier>,
}

pub struct ResearchRuns {
    parts: ResearchParts,
    /// Questions whose run is being started; a second start of one waits for nothing and fails.
    starting: Mutex<HashSet<String>>,
}

impl std::fmt::Debug for ResearchRuns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResearchRuns").finish_non_exhaustive()
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

pub(super) fn same_folder(left: &str, right: &str) -> bool {
    let plain = |text: &str| text.replace('\\', "/").trim_end_matches('/').to_lowercase();
    plain(left) == plain(right)
}

impl ResearchRuns {
    #[must_use]
    pub fn new(parts: ResearchParts) -> Arc<Self> {
        Arc::new(Self {
            parts,
            starting: Mutex::default(),
        })
    }

    /// "Research" on a question: starts at once, whatever the switch, the window or the opt-in say.
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
                self.start(key, ResearchTrigger::Manual, 1, &config).await
            })
            .await
    }

    /// Whether a research of the question is under way or waits for its evaluation.
    async fn is_running(&self, key: &QuestionKey) -> Result<bool, AppError> {
        let filter = RunFilter {
            subject_key: Some(key.to_string()),
            kind: Some(RunKind::Research),
            ..RunFilter::default()
        };
        Ok(self
            .parts
            .runs
            .find(&filter, 20)
            .await?
            .iter()
            .any(|run| run.outcome().is_none()))
    }

    /// Research runs that are under way or wait for their evaluation.
    async fn open_runs(&self) -> Result<Vec<Run>, AppError> {
        let filter = RunFilter {
            kind: Some(RunKind::Research),
            ..RunFilter::default()
        };
        let mut runs = self.parts.runs.find(&filter, 500).await?;
        runs.retain(|run| run.outcome().is_none());
        Ok(runs)
    }

    async fn board_roots(&self) -> Result<Vec<BoardRoot>, AppError> {
        board_roots(self.parts.boards.as_ref(), &self.parts.runners).await
    }

    /// The checkout: the board of a linked ticket, else the oldest board of the project.
    async fn root_for(
        &self,
        roots: &[BoardRoot],
        record: &QuestionRecord,
    ) -> Result<Option<String>, AppError> {
        for key in &record.tickets {
            let board = match self.parts.tickets.get(&key.to_string()).await {
                Ok(detail) => detail.view.ticket.record().board,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if let Some(found) = roots.iter().find(|found| found.board == board) {
                return Ok(Some(found.root.clone()));
            }
        }
        Ok(roots
            .iter()
            .find(|found| found.project == record.project)
            .map(|found| found.root.clone()))
    }

    fn choice(config: &DispatcherConfig, attempt: usize) -> ProfileChoice {
        let name = config
            .research
            .profile
            .clone()
            .unwrap_or_else(|| config.default_profile.clone());
        let base = ProfileChoice {
            profile: name,
            reason: ProfileReason::Default,
            rule: None,
            why: String::from("research"),
            escalated_from: None,
        };
        escalate(config, base, attempt)
    }

    /// Starts the research of an open question in a detached worktree at its checkout's HEAD.
    async fn start(
        &self,
        key: &str,
        trigger: ResearchTrigger,
        attempt: usize,
        config: &DispatcherConfig,
    ) -> Result<Run, AppError> {
        let parsed = QuestionKey::parse(key).map_err(|error| crate::questions::invalid(&error))?;
        let text = parsed.to_string();
        if !self.starting.lock().insert(text.clone()) {
            return Err(running(&parsed));
        }
        let _place = Starting {
            set: &self.starting,
            key: text.clone(),
        };
        let detail = self.parts.questions.get(&text).await?;
        let record = detail.view.row.question.record();
        if record.state != QuestionState::Open {
            return Err(not_open(&record.key));
        }
        if self.is_running(&record.key).await? {
            return Err(running(&record.key));
        }
        let roots = self.board_roots().await?;
        let root = self
            .root_for(&roots, record)
            .await?
            .ok_or_else(|| no_repo(&record.key))?;
        let choice = Self::choice(config, attempt);
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
        let (run, path) = self.launch(&text, &root, trigger, &choice, &chosen).await?;
        self.parts.accounts.record_start(run.id(), &chosen).await;
        self.note_start(record.id.as_uuid(), &text, trigger, &choice, &path, &run)
            .await;
        Ok(run)
    }

    /// The worktree and the run in it; a run that cannot start takes its worktree with it.
    async fn launch(
        &self,
        text: &str,
        root: &str,
        trigger: ResearchTrigger,
        choice: &ProfileChoice,
        chosen: &Chosen,
    ) -> Result<(Run, String), AppError> {
        let created = self
            .parts
            .detached
            .create(&DetachedCreate {
                root: root.to_owned(),
                purpose: DetachedPurpose::Research,
                subject: text.to_owned(),
                at: String::from("HEAD"),
            })
            .await?;
        let path = match created {
            DetachedCreated::Created { path, .. } => path,
            DetachedCreated::Busy { path } => {
                return Err(detached_busy(DetachedPurpose::Research, &path));
            }
        };
        let started = match self.parts.prompts.research_prompt(text).await {
            Ok(prompt) => {
                self.parts
                    .dispatch
                    .start(StartDispatchRun {
                        kind: RunKind::Research,
                        trigger: match trigger {
                            ResearchTrigger::Manual => RunTrigger::Manual,
                            ResearchTrigger::Auto => RunTrigger::Dispatcher,
                        },
                        subject: text.to_owned(),
                        prompt,
                        cwd: PathBuf::from(&path),
                        profile: Some(choice.profile.clone()),
                        account: Some(chosen.account),
                        extra_tools: Vec::new(),
                        agent: Some(format!("{RESEARCH_AGENT_PREFIX}{text}")),
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

    /// Counts the start and writes it to the log; neither may undo a run that started.
    async fn note_start(
        &self,
        subject: Uuid,
        text: &str,
        trigger: ResearchTrigger,
        choice: &ProfileChoice,
        path: &str,
        run: &Run,
    ) {
        let now = self.parts.clock.now();
        let auto = trigger == ResearchTrigger::Auto;
        if let Err(error) = self
            .parts
            .tracking
            .started(TrackKind::Research, subject, now, auto)
            .await
        {
            self.parts
                .log
                .error(
                    &format!("research of {text} not counted: {}", error.message()),
                    Some(text),
                )
                .await;
        }
        let how = if auto { "automatically" } else { "by hand" };
        self.parts
            .log
            .write(
                LogLevel::Info,
                &format!(
                    "research started {text} ({how}) in {path} [{}]",
                    choice.profile
                ),
                Some(text),
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
                purpose: DetachedPurpose::Research,
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

    /// Settles a finished research run: stored, empty or gone; the worktree always goes.
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
        let outcome = self
            .outcome(run, &key, dispatch.agent.as_deref(), config, pauses)
            .await;
        if let Some((root, path)) = detached_root(DetachedPurpose::Research, &dispatch.cwd) {
            self.remove(&root, &path).await;
        }
        let outcome = outcome?;
        // a pause or a vanished question counts for nothing, as for tickets
        if matches!(outcome, DispatchOutcome::Research | DispatchOutcome::Other)
            && let Err(error) = self.decide_autopilot(run, &key, outcome, config).await
        {
            self.parts
                .log
                .error(
                    &format!("autopilot for {key}: {}", error.message()),
                    Some(&key),
                )
                .await;
        }
        Ok(outcome)
    }

    async fn outcome(
        &self,
        run: &Run,
        key: &str,
        agent: Option<&str>,
        config: &DispatcherConfig,
        pauses: &dyn PauseHook,
    ) -> Result<DispatchOutcome, AppError> {
        let detail = match self.parts.questions.get(key).await {
            Ok(detail) => detail,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(DispatchOutcome::Gone),
            Err(error) => return Err(error),
        };
        let agent = agent.map_or_else(|| format!("{RESEARCH_AGENT_PREFIX}{key}"), str::to_owned);
        let started = run.started_at().unwrap_or_else(|| run.created_at());
        let since = Timestamp::from_millis(started.as_millis().saturating_sub(CLOCK_SKEW_MS));
        if self
            .parts
            .questions
            .researched_by(key, &agent, since)
            .await?
        {
            self.parts
                .log
                .write(
                    LogLevel::Info,
                    &format!("research of {key} stored"),
                    Some(key),
                    Some(run.id()),
                )
                .await;
            return Ok(DispatchOutcome::Research);
        }
        let kind = match run
            .result()
            .and_then(|result| result.abort.as_ref())
            .map(|abort| abort.category)
        {
            Some(AbortCategory::UsageLimit) => Some(PauseKind::UsageLimit),
            Some(AbortCategory::StartFailed) if !never_spawned(run) => {
                Some(PauseKind::ClaudeMissing)
            }
            _ => None,
        };
        // a run that never spawned counts for nothing and pauses nothing
        let unpaused = if never_spawned(run) {
            DispatchOutcome::StartFailed
        } else {
            DispatchOutcome::Other
        };
        let outcome_of = |kind: PauseKind| match kind {
            PauseKind::UsageLimit => DispatchOutcome::PauseLimit,
            PauseKind::ClaudeMissing => DispatchOutcome::PauseClaudeMissing,
        };
        // a pass that stopped after the comment is picked up where it was
        if commented(&detail, run) {
            return Ok(kind.map_or(unpaused, outcome_of));
        }
        let actor = self.parts.starter.actor().await?;
        // a limit or missing Claude pauses as it does for tickets and counts for nothing
        if let Some(kind) = kind {
            let title = detail.view.row.question.record().title.as_str();
            if let Some(text) = pauses.paused_research(run, title, kind, config).await? {
                let body = format!("{text}\n\nRun: {}", run.id());
                self.parts
                    .questions
                    .comment_system(
                        key,
                        &body,
                        SystemMarker::ResearchEmpty,
                        Some(run.id()),
                        actor,
                    )
                    .await?;
                return Ok(outcome_of(kind));
            }
        }
        let why = why_empty(run);
        let body = format!(
            "[dispatch] research-empty: {why}.\nNo automatic retry - \"Research\" on the question starts a new one.\n\nRun: {}",
            run.id()
        );
        self.parts
            .questions
            .comment_system(
                key,
                &body,
                SystemMarker::ResearchEmpty,
                Some(run.id()),
                actor,
            )
            .await?;
        self.parts
            .log
            .write(
                LogLevel::Info,
                &format!("research of {key} without result: {why}"),
                Some(key),
                Some(run.id()),
            )
            .await;
        Ok(unpaused)
    }

    /// The tickets of an autopilot question: open and asked by an agent of the dispatcher.
    async fn autopilot_tickets(
        &self,
        config: &DispatcherConfig,
        view: &QuestionView,
    ) -> Result<Option<Vec<TicketRecord>>, AppError> {
        let row = &view.row;
        let record = row.question.record();
        if record.state != QuestionState::Open
            || !config.asked_by_dispatch_agent(row.origin_kind, &row.origin_name)
        {
            return Ok(None);
        }
        let mut found = Vec::new();
        for key in &record.tickets {
            match self.parts.tickets.get(&key.to_string()).await {
                Ok(detail) => {
                    let ticket = detail.view.ticket.record().clone();
                    if config.autopilot_applies(&ticket.labels) {
                        found.push(ticket);
                    }
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok((!found.is_empty()).then_some(found))
    }

    /// After a research of an autopilot question: adopt the one recommendation or count the try.
    async fn decide_autopilot(
        &self,
        run: &Run,
        key: &str,
        outcome: DispatchOutcome,
        config: &DispatcherConfig,
    ) -> Result<(), AppError> {
        if !config.autopilot.enabled {
            return Ok(());
        }
        let detail = match self.parts.questions.get(key).await {
            Ok(detail) => detail,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let Some(tickets) = self.autopilot_tickets(config, &detail.view).await? else {
            return Ok(());
        };
        let subject = detail.view.row.question.record().id.as_uuid();
        let recommended: Vec<_> = if outcome == DispatchOutcome::Research {
            detail
                .newest_options()
                .iter()
                .filter(|option| option.recommended)
                .collect()
        } else {
            Vec::new()
        };
        if let [option] = recommended.as_slice() {
            return self.adopt(key, subject, option, &tickets).await;
        }
        let tried = self.count_try(subject, run).await?;
        if tried < AUTOPILOT_ATTEMPTS {
            self.parts
                .log
                .info(
                    &format!(
                        "autopilot {key}: no clear recommendation (try {tried}/{AUTOPILOT_ATTEMPTS}), the next research uses a bigger profile"
                    ),
                    Some(key),
                )
                .await;
            return Ok(());
        }
        self.give_up(key, tried, &tickets).await
    }

    /// Counts the try of a run once; the row's `last_started_at` is the run counted last.
    async fn count_try(&self, subject: Uuid, run: &Run) -> Result<u32, AppError> {
        let at = run.created_at();
        let row = self
            .parts
            .tracking
            .get(TrackKind::AutopilotResearch, subject)
            .await?
            .unwrap_or_default();
        if row.last_started_at == Some(at) {
            return Ok(row.count);
        }
        let counted = Tracked {
            count: row.count + 1,
            last_started_at: Some(at),
            auto_at: row.auto_at,
        };
        self.parts
            .tracking
            .put(TrackKind::AutopilotResearch, subject, counted)
            .await?;
        Ok(counted.count)
    }

    /// The autopilot answers with the one recommended option and tells the tickets.
    async fn adopt(
        &self,
        key: &str,
        subject: Uuid,
        option: &QuestionOption,
        tickets: &[TicketRecord],
    ) -> Result<(), AppError> {
        let actor = self.parts.starter.actor().await?;
        let answer = crate::questions::Answer {
            option: Some(option.key()),
            text: None,
            reason: Some(format!(
                "{AUTOPILOT_PREFIX} recommendation of the research adopted"
            )),
        };
        let author = crate::comments::Author::default();
        self.parts
            .questions
            .answer(key, &answer, author, actor, None)
            .await?;
        self.parts
            .tracking
            .clear(TrackKind::AutopilotResearch, subject)
            .await?;
        for ticket in tickets {
            let body = format!(
                "{AUTOPILOT_PREFIX} question {key} decided with \"{}\"",
                option.title
            );
            self.parts
                .comments
                .add_system(&ticket.key, &body, None, None, actor)
                .await?;
        }
        self.parts
            .log
            .info(
                &format!("autopilot {key}: option {} taken over", option.key()),
                Some(key),
            )
            .await;
        Ok(())
    }

    /// Researches without a clear recommendation are used up: the open tickets wait for a person.
    async fn give_up(
        &self,
        key: &str,
        tried: u32,
        tickets: &[TicketRecord],
    ) -> Result<(), AppError> {
        let actor = self.parts.starter.actor().await?;
        let reason = format!("{AUTOPILOT_PREFIX} no recommendation for {key}");
        for ticket in tickets
            .iter()
            .filter(|ticket| !matches!(ticket.status, TicketStatus::Done | TicketStatus::NotNeeded))
        {
            let text = ticket.key.to_string();
            let version = self
                .parts
                .tickets
                .get(&text)
                .await?
                .view
                .ticket
                .record()
                .version;
            let patch = TicketPatch {
                status: Some(String::from("blocked")),
                blocked_reason: Some(Some(reason.clone())),
                assignee: Some(None),
                ..TicketPatch::default()
            };
            let options = WriteOptions {
                force: true,
                ..WriteOptions::default()
            };
            self.parts
                .tickets
                .update(&text, &patch, actor, version, options)
                .await?;
            let notice = Notice::blocked(true, &text, ticket.title.as_str(), &reason);
            if let Err(error) = self.parts.notifier.notify(&notice).await {
                self.parts
                    .log
                    .error(
                        &format!("cannot show a notice: {}", error.message()),
                        Some(&text),
                    )
                    .await;
            }
        }
        self.parts
            .log
            .info(
                &format!("autopilot {key}: {reason} after {tried} researches"),
                Some(key),
            )
            .await;
        Ok(())
    }

    /// Removes research worktrees no run belongs to any more.
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
                .list(&found.root, DetachedPurpose::Research)
                .await;
            let listed = match listed {
                Ok(listed) => listed,
                Err(error) => {
                    report
                        .errors
                        .push(format!("research worktrees: {}", error.message()));
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
                    .info(&format!("orphaned research worktree {path} removed"), None)
                    .await;
            }
        }
        Ok(())
    }

    async fn tracking_row(&self, row: Uuid) -> Result<Option<Tracked>, AppError> {
        self.parts.tracking.get(TrackKind::Research, row).await
    }

    /// Turns the automation's clock on and off with its switch: only newer questions are taken up.
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
                    .put(TrackKind::Research, SINCE_ROW, row)
                    .await?;
                self.parts
                    .tracking
                    .clear(TrackKind::Research, RETRY_ROW)
                    .await
            }
            (false, Some(_)) => {
                self.parts
                    .tracking
                    .clear(TrackKind::Research, SINCE_ROW)
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

    /// Starts of today and the local day (days since 1970-01-01, as `YYYY-MM-DD` counts them).
    async fn used_today(&self, now: Timestamp) -> Result<(u32, Timestamp), AppError> {
        // the key is the local calendar day, so a change of the clock inside it counts on
        let offset = self.parts.local.offset_minutes(now).await;
        let day = Timestamp::from_millis(nullradix_domain::local_day_of(now, offset));
        let used = match self.tracking_row(DAY_ROW).await? {
            Some(row) if row.last_started_at == Some(day) => row.count,
            _ => 0,
        };
        Ok((used, day))
    }

    /// An environment fault pauses the automation; the same text is reported once.
    async fn suspend(
        &self,
        now: Timestamp,
        question: &str,
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
            .put(TrackKind::Research, RETRY_ROW, row)
            .await?;
        let message = format!(
            "research automation paused for {RETRY_MINUTES} minutes (at {question}): {}",
            error.message()
        );
        self.parts.log.error(&message, Some(question)).await;
        report.errors.push(message);
        Ok(())
    }

    async fn open_questions(&self, roots: &[BoardRoot]) -> Result<Vec<QuestionView>, AppError> {
        let projects: HashSet<Uuid> = roots.iter().map(|found| found.project.as_uuid()).collect();
        let filter = QuestionFilter {
            state: Some(QuestionState::Open),
            ..QuestionFilter::default()
        };
        let mut found = Vec::new();
        for project in projects {
            let list = self
                .parts
                .questions
                .list(Id::from_uuid(project), &filter)
                .await?;
            found.extend(list.questions);
        }
        found.sort_by_key(|view| {
            let record = view.row.question.record();
            (record.created_at, record.key.to_string())
        });
        Ok(found)
    }

    /// Starts what fits and notes it; false when the pass should stop starting research.
    async fn try_start(
        &self,
        key: &str,
        subject: Uuid,
        attempt: usize,
        context: &PassContext,
        report: &mut PassReport,
    ) -> Result<Option<Run>, AppError> {
        match self
            .start(key, ResearchTrigger::Auto, attempt, &context.config)
            .await
        {
            Ok(run) => {
                report.research.push(key.to_owned());
                Ok(Some(run))
            }
            Err(error) if QUESTION_ERRORS.contains(&error.code()) => {
                self.parts
                    .tracking
                    .mark(TrackKind::Research, subject, context.now)
                    .await?;
                report.errors.push(format!("{key}: {}", error.message()));
                Ok(None)
            }
            // a busy account or a pause ends the round without blaming anything
            Err(error) if error.code() == "dispatch-paused" => Err(error),
            Err(error) => {
                self.suspend(context.now, key, &error, report).await?;
                Err(error)
            }
        }
    }

    /// Autopilot questions first: also with the automation off, without the daily cap.
    async fn autopilot(
        &self,
        context: &PassContext,
        report: &mut PassReport,
    ) -> Result<(), AppError> {
        let config = &context.config;
        let mut free =
            config.research.wip - i64::try_from(self.open_runs().await?.len()).unwrap_or(0);
        if free <= 0 {
            return Ok(());
        }
        let roots = self.board_roots().await?;
        let tried = self
            .parts
            .tracking
            .all(TrackKind::AutopilotResearch)
            .await?;
        for view in self.open_questions(&roots).await? {
            if free <= 0 {
                break;
            }
            let record = view.row.question.record();
            let count = tried.get(&record.id.as_uuid()).map_or(0, |row| row.count);
            if count >= AUTOPILOT_ATTEMPTS
                || self.autopilot_tickets(config, &view).await?.is_none()
                || self.is_running(&record.key).await?
                || self.root_for(&roots, record).await?.is_none()
            {
                continue;
            }
            let attempt = usize::try_from(count).unwrap_or(0) + 1;
            let key = record.key.to_string();
            match self
                .try_start(&key, record.id.as_uuid(), attempt, context, report)
                .await
            {
                Ok(Some(_)) => free -= 1,
                Ok(None) => {}
                Err(_) => break,
            }
        }
        Ok(())
    }

    /// New open questions without a research, up to the places and the starts of the day.
    async fn automatic(
        &self,
        context: &PassContext,
        report: &mut PassReport,
    ) -> Result<(), AppError> {
        let config = &context.config;
        let Some(since) = self
            .tracking_row(SINCE_ROW)
            .await?
            .and_then(|row| row.auto_at)
        else {
            return Ok(());
        };
        let (mut used, day) = self.used_today(context.now).await?;
        let max = u32::try_from(config.research.max_per_day).unwrap_or(0);
        let mut free =
            config.research.wip - i64::try_from(self.open_runs().await?.len()).unwrap_or(0);
        if free <= 0 || used >= max {
            return Ok(());
        }
        let roots = self.board_roots().await?;
        let tracked = self.parts.tracking.all(TrackKind::Research).await?;
        for view in self.open_questions(&roots).await? {
            if free <= 0 || used >= max {
                break;
            }
            let record = view.row.question.record();
            let subject = record.id.as_uuid();
            if view.row.research_rounds > 0
                || record.created_at < since
                || tracked.contains_key(&subject)
                || self.is_running(&record.key).await?
                || self.root_for(&roots, record).await?.is_none()
            {
                continue;
            }
            let key = record.key.to_string();
            match self.try_start(&key, subject, 1, context, report).await {
                Ok(Some(_)) => {
                    free -= 1;
                    used += 1;
                    let row = Tracked {
                        count: used,
                        last_started_at: Some(day),
                        auto_at: None,
                    };
                    self.parts
                        .tracking
                        .put(TrackKind::Research, DAY_ROW, row)
                        .await?;
                    self.parts
                        .tracking
                        .clear(TrackKind::Research, RETRY_ROW)
                        .await?;
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
        Ok(())
    }

    async fn start_step(
        &self,
        context: &PassContext,
        report: &mut PassReport,
    ) -> Result<(), AppError> {
        let config = &context.config;
        self.follow_switch(config.research.auto, context.now)
            .await?;
        if !(config.autopilot.enabled || config.research.auto) {
            return Ok(());
        }
        // a pause or the time window holds research back as it does tickets; the switch does not
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
        if self
            .retry_at()
            .await?
            .is_some_and(|until| until > context.now)
        {
            return Ok(());
        }
        if config.autopilot.enabled {
            self.autopilot(context, report).await?;
        }
        if config.research.auto {
            self.automatic(context, report).await?;
        }
        Ok(())
    }
}

/// Whether the research-empty comment of this run is on the question already.
fn commented(detail: &QuestionDetail, run: &Run) -> bool {
    detail.comments.iter().any(|line| {
        let record = line.comment.record();
        record.system_marker == Some(SystemMarker::ResearchEmpty) && record.run == Some(run.id())
    })
}

fn why_empty(run: &Run) -> String {
    match run.result().and_then(|result| result.abort.as_ref()) {
        Some(abort) => abort.reason.trim_end_matches('.').to_owned(),
        None if run.status() == RunStatus::Cancelled => String::from("the run was cancelled"),
        None => String::from("the run ended without saving a research on the question"),
    }
}

/// The pass step that removes research worktrees whose runs are gone.
#[derive(Debug)]
pub struct ResearchOrphans(pub Arc<ResearchRuns>);

#[async_trait]
impl PassStep for ResearchOrphans {
    async fn run(&self, _context: &PassContext, report: &mut PassReport) -> Result<(), AppError> {
        self.0.remove_orphans(report).await
    }
}

/// The pass step that starts research on open questions.
#[derive(Debug)]
pub struct StartResearch(pub Arc<ResearchRuns>);

#[async_trait]
impl PassStep for StartResearch {
    async fn run(&self, context: &PassContext, report: &mut PassReport) -> Result<(), AppError> {
        self.0.start_step(context, report).await
    }
}
