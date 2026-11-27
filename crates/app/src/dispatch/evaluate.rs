//! The pass step that settles finished work runs: done, human, blocked and question outcomes.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    AUTOPILOT_PREFIX, AbortCategory, Actor, DispatchOutcome, DispatcherConfig, Id, QuestionKey,
    Run, RunKind, SystemMarker, TicketRecord, TicketStatus, is_abort_of,
};
use parking_lot::RwLock;

use super::abort::{AbortAttempts, AbortRequest, Aborted, Standing};
use super::accounts::AccountPicker;
use super::log::{DispatchLog, LogLevel};
use super::merge::MergeAll;
use super::outcome::{Decision, Ended, decide, question_comment};
use super::pass::{PassContext, PassReport, PassStep};
use super::pause::{NoPauseHook, PauseHook, PauseKind};
use super::planning::PlanningRuns;
use super::research::ResearchRuns;
use super::start::TicketStarter;
use crate::comments::CommentService;
use crate::notifier::{NoNotifier, Notice, NoticeKind, Notifier};
use crate::runs::{RunFilter, RunService};
use crate::tickets::{TicketDetail, TicketPatch, TicketService, WriteOptions};
use crate::{AppError, ErrorKind};

/// Finished runs settled per pass; the rest follows on the next one.
const BATCH: u32 = 50;

/// What follows a finished ticket: the review hold and the merge hang here in later slices.
#[async_trait]
pub trait DoneGate: Send + Sync {
    /// The outcome of a run whose ticket is done; also called after an autopilot accept.
    async fn finished(
        &self,
        ticket: &TicketRecord,
        run: &Run,
        config: &DispatcherConfig,
    ) -> Result<DispatchOutcome, AppError>;
    /// Whether the gate already took this run, so that settling it again gives the same outcome.
    async fn held(&self, _ticket: &TicketRecord, _run: &Run) -> Result<bool, AppError> {
        Ok(false)
    }
}

/// Holds and merges nothing yet.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDoneGate;

#[async_trait]
impl DoneGate for NoDoneGate {
    async fn finished(
        &self,
        _ticket: &TicketRecord,
        _run: &Run,
        _config: &DispatcherConfig,
    ) -> Result<DispatchOutcome, AppError> {
        Ok(DispatchOutcome::Done)
    }
}

pub struct EvaluateRuns {
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
    runs: Arc<RunService>,
    starter: Arc<TicketStarter>,
    log: Arc<DispatchLog>,
    aborts: Arc<AbortAttempts>,
    gate: Arc<dyn DoneGate>,
    accounts: Option<Arc<AccountPicker>>,
    research: Option<Arc<ResearchRuns>>,
    planning: Option<Arc<PlanningRuns>>,
    merge: Option<Arc<MergeAll>>,
    pauses: RwLock<Arc<dyn PauseHook>>,
    notifier: Arc<dyn Notifier>,
}

impl std::fmt::Debug for EvaluateRuns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvaluateRuns").finish_non_exhaustive()
    }
}

/// Whether the settling of a run is over or has to wait for the next pass.
enum Settled {
    Yes(DispatchOutcome),
    Later,
}

impl EvaluateRuns {
    #[must_use]
    pub fn new(
        tickets: Arc<TicketService>,
        comments: Arc<CommentService>,
        runs: Arc<RunService>,
        starter: Arc<TicketStarter>,
        log: Arc<DispatchLog>,
        aborts: Arc<AbortAttempts>,
    ) -> Self {
        Self {
            tickets,
            comments,
            runs,
            starter,
            log,
            aborts,
            gate: Arc::new(NoDoneGate),
            accounts: None,
            research: None,
            planning: None,
            merge: None,
            pauses: RwLock::new(Arc::new(NoPauseHook)),
            notifier: Arc::new(NoNotifier),
        }
    }

    /// Replaces what happens to runs that ended on the usage limit or without Claude.
    pub fn set_pause_hook(&self, pauses: Arc<dyn PauseHook>) {
        *self.pauses.write() = pauses;
    }

    /// Shows the notices of questions and blocked tickets.
    #[must_use]
    pub fn with_notifier(mut self, notifier: Arc<dyn Notifier>) -> Self {
        self.notifier = notifier;
        self
    }

    async fn toast(&self, notice: Notice) {
        if let Err(error) = self.notifier.notify(&notice).await {
            self.log
                .error(
                    &format!("cannot show a notice: {}", error.message()),
                    notice.ticket.as_deref(),
                )
                .await;
        }
    }

    /// Replaces what follows a finished ticket.
    #[must_use]
    pub fn with_gate(mut self, gate: Arc<dyn DoneGate>) -> Self {
        self.gate = gate;
        self
    }

    /// Has the usage at the end of each settled run recorded for the reserves.
    #[must_use]
    pub fn with_accounts(mut self, accounts: Arc<AccountPicker>) -> Self {
        self.accounts = Some(accounts);
        self
    }

    /// Has research runs settled too; without it they wait.
    #[must_use]
    pub fn with_research(mut self, research: Arc<ResearchRuns>) -> Self {
        self.research = Some(research);
        self
    }

    /// Has plan runs settled too; without it they wait.
    #[must_use]
    pub fn with_planning(mut self, planning: Arc<PlanningRuns>) -> Self {
        self.planning = Some(planning);
        self
    }

    /// Has merge sessions settled too; without it they wait.
    #[must_use]
    pub fn with_merge(mut self, merge: Arc<MergeAll>) -> Self {
        self.merge = Some(merge);
        self
    }

    /// Settles a merge session: its job merges again, tries again or is blocked.
    async fn settle_merge(
        &self,
        run: &Run,
        key: &str,
        context: &PassContext,
        report: &mut PassReport,
    ) {
        let Some(merge) = &self.merge else {
            return;
        };
        let settled = merge.settle_session(run, context).await;
        self.record_settled(run, key, settled, report).await;
    }

    /// Settles a research run: its question gets the research or the comment that it left none.
    async fn settle_research(
        &self,
        run: &Run,
        key: &str,
        config: &DispatcherConfig,
        report: &mut PassReport,
    ) {
        let Some(research) = &self.research else {
            return;
        };
        let settled = research.settle(run, config, &*self.pause_hook()).await;
        self.record_settled(run, key, settled, report).await;
    }

    /// Settles a plan run: its ticket gets the plan or the comment that it left none.
    async fn settle_plan(
        &self,
        run: &Run,
        key: &str,
        config: &DispatcherConfig,
        report: &mut PassReport,
    ) {
        let Some(planning) = &self.planning else {
            return;
        };
        let settled = planning.settle(run, config, &*self.pause_hook()).await;
        self.record_settled(run, key, settled, report).await;
    }

    /// Writes the outcome of a side run down; a run settled meanwhile is left alone.
    async fn record_settled(
        &self,
        run: &Run,
        key: &str,
        settled: Result<DispatchOutcome, AppError>,
        report: &mut PassReport,
    ) {
        match settled {
            Ok(outcome) => match self.runs.settle_outcome(run.id(), outcome).await {
                Ok(_) => {
                    self.record_end(run).await;
                    self.log
                        .write(
                            LogLevel::Info,
                            &format!("settled {key}: {}", outcome.as_str()),
                            Some(key),
                            Some(run.id()),
                        )
                        .await;
                }
                Err(error) if error.kind() == ErrorKind::Conflict => {}
                Err(error) => report.errors.push(format!("{key}: {}", error.message())),
            },
            Err(error) => report.errors.push(format!("{key}: {}", error.message())),
        }
    }

    /// Keeps the usage of the account at the end of the run; repeating it changes nothing.
    async fn record_end(&self, run: &Run) {
        let account = run.dispatch().and_then(|dispatch| dispatch.account);
        if let (Some(accounts), Some(account)) = (&self.accounts, account) {
            accounts.record_end(run.id(), Id::from_uuid(account)).await;
        }
    }

    async fn settle(
        &self,
        run: &Run,
        key: &str,
        config: &DispatcherConfig,
    ) -> Result<Settled, AppError> {
        let detail = match self.tickets.get(key).await {
            Ok(detail) => detail,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(Settled::Yes(DispatchOutcome::Gone));
            }
            Err(error) => return Err(error),
        };
        // a pass lost after the hold settles the run as held, unless the ticket is still done
        let current = detail.view.ticket.record();
        if current.status != TicketStatus::Done && self.gate.held(current, run).await? {
            return Ok(Settled::Yes(DispatchOutcome::Review));
        }
        let record = detail.view.ticket.record();
        let questions: Vec<QuestionKey> = detail
            .view
            .open_questions
            .iter()
            .map(|question| question.key.clone())
            .collect();
        // a pass that stopped after the comment is picked up where it was
        let actor = self.starter.actor().await?;
        let asked = question_posted(&detail, run);
        let counted = abort_posted(&detail, run, actor);
        let decision = if asked {
            Decision::Question(questions)
        } else if counted {
            Decision::Abort(Ended {
                category: None,
                reason: String::new(),
            })
        } else {
            decide(config, &agent_of(run, key, config), run, record, &questions)
        };
        let outcome = match &decision {
            Decision::StartFailed => return self.start_failed(run, record, config).await,
            Decision::Abort(ended) => {
                return self.abort(run, record, ended, counted, config, actor).await;
            }
            Decision::Question(questions) => {
                if !self.ask(&detail, run, questions, asked, config).await? {
                    return Ok(Settled::Later);
                }
                DispatchOutcome::Question
            }
            Decision::AutoAccept { reason } => {
                let accepted = self.auto_accept(&detail, run, reason).await?;
                self.gate.finished(&accepted, run, config).await?
            }
            Decision::Done => self.gate.finished(record, run, config).await?,
            Decision::Blocked(reason) => {
                let reason = if reason.trim().is_empty() {
                    "Blocked"
                } else {
                    reason.as_str()
                };
                self.toast(Notice::blocked(
                    config.autopilot_applies(&record.labels),
                    key,
                    record.title.as_str(),
                    reason,
                ))
                .await;
                DispatchOutcome::Blocked
            }
            other => other.outcome().unwrap_or(DispatchOutcome::Other),
        };
        Ok(Settled::Yes(outcome))
    }

    fn pause_hook(&self) -> Arc<dyn PauseHook> {
        self.pauses.read().clone()
    }

    /// A run that never spawned has its own outcome: no attempt, no pause, a wait.
    async fn start_failed(
        &self,
        run: &Run,
        record: &TicketRecord,
        config: &DispatcherConfig,
    ) -> Result<Settled, AppError> {
        let outcome = DispatchOutcome::StartFailed;
        // its start gave the claim back already, and a newer one may be there now
        self.wait_out(run, record, outcome, config, false, true)
            .await
    }

    /// No hook takes the run: the ticket goes back uncounted and does not start for a while.
    async fn wait_out(
        &self,
        run: &Run,
        record: &TicketRecord,
        outcome: DispatchOutcome,
        config: &DispatcherConfig,
        release: bool,
        hold: bool,
    ) -> Result<Settled, AppError> {
        let key = record.key.to_string();
        let mut version = record.version;
        let ours = record.status == TicketStatus::InProgress
            && record.assignee.as_deref() == Some(agent_of(run, &key, config).as_str());
        if release && ours {
            let actor = self.starter.actor().await?;
            match self
                .tickets
                .release(
                    &key,
                    Some("planned"),
                    actor,
                    Some(version),
                    WriteOptions::default(),
                )
                .await
            {
                Ok(view) => version = view.ticket.record().version,
                // a person changed the ticket first: look again on the next pass
                Err(error) if error.kind() == ErrorKind::Conflict => return Ok(Settled::Later),
                Err(error) => return Err(error),
            }
        }
        if hold {
            self.starter
                .hold_off(&key, version, config.pause_minutes_on_limit);
        }
        Ok(Settled::Yes(outcome))
    }

    /// Whether a newer work run of the ticket exists; it owns the ticket's claim now.
    async fn superseded(&self, run: &Run, key: &str) -> Result<bool, AppError> {
        let Some(finished) = run.finished_at() else {
            return Ok(false);
        };
        let filter = RunFilter {
            subject_key: Some(key.to_owned()),
            kind: Some(RunKind::Work),
            ..RunFilter::default()
        };
        let newest = self.runs.find(&filter, 1).await?;
        Ok(newest
            .first()
            .is_some_and(|other| other.id() != run.id() && other.created_at() > finished))
    }

    /// Counts the attempt of a run that achieved nothing, unless a pause takes the run.
    async fn abort(
        &self,
        run: &Run,
        record: &TicketRecord,
        ended: &Ended,
        counted: bool,
        config: &DispatcherConfig,
        actor: &Actor,
    ) -> Result<Settled, AppError> {
        let key = record.key.to_string();
        let kind = match ended.category {
            Some(AbortCategory::UsageLimit) => Some(PauseKind::UsageLimit),
            Some(AbortCategory::StartFailed) => Some(PauseKind::ClaudeMissing),
            _ => None,
        };
        let kind = match kind {
            Some(PauseKind::UsageLimit)
                if self.pause_hook().limit_exhausted(run, record).await? =>
            {
                None
            }
            other => other,
        };
        if let Some(kind) = kind.filter(|_| !counted) {
            if let Some(outcome) = self.pause_hook().paused(run, record, kind, config).await? {
                return Ok(Settled::Yes(outcome));
            }
            let ours = !self.superseded(run, &key).await?;
            return self
                .wait_out(run, record, pause_outcome(kind), config, ours, ours)
                .await;
        }
        let agent = agent_of(run, &key, config);
        let request = AbortRequest {
            key: &record.key,
            run: Some(run),
            agent: &agent,
            reason: &ended.reason,
            standing: Standing::Claimed,
            expected: None,
            superseded: self.superseded(run, &key).await?,
        };
        Ok(match self.aborts.abort(&request, config, actor).await? {
            Aborted::Counted { last: true, .. } => Settled::Yes(DispatchOutcome::AbortFinal),
            Aborted::Counted { last: false, .. } => Settled::Yes(DispatchOutcome::Abort),
            Aborted::Postponed => Settled::Later,
        })
    }

    /// Comments, then gives the ticket back; false when the ticket changed in between.
    async fn ask(
        &self,
        detail: &TicketDetail,
        run: &Run,
        questions: &[QuestionKey],
        asked: bool,
        config: &DispatcherConfig,
    ) -> Result<bool, AppError> {
        let actor = self.starter.actor().await?;
        let record = detail.view.ticket.record();
        let key = record.key.clone();
        if !asked {
            let body = question_comment(questions, record.branch.as_deref());
            self.comments
                .add_system(
                    &key,
                    &body,
                    Some(SystemMarker::Question),
                    Some(run.id()),
                    actor,
                )
                .await?;
            let waiting: Vec<String> = questions.iter().map(ToString::to_string).collect();
            self.toast(Notice {
                kind: NoticeKind::Question,
                ticket: Some(key.to_string()),
                title: record.title.as_str().to_owned(),
                text: format!("waiting for {}", waiting.join(", ")),
            })
            .await;
        }
        // the comment moved the ticket on; only a ticket still with the agent is given back
        let fresh = self.tickets.get(&key.to_string()).await?;
        let current = fresh.view.ticket.record();
        let ours = current.status == TicketStatus::InProgress
            && current.assignee.as_deref()
                == Some(agent_of(run, &key.to_string(), config).as_str());
        if !ours {
            return Ok(true);
        }
        let version = current.version;
        match self
            .tickets
            .release(
                &key.to_string(),
                Some("planned"),
                actor,
                Some(version),
                WriteOptions::default(),
            )
            .await
        {
            Ok(_) => Ok(true),
            // a person changed the ticket first: look again on the next pass
            Err(error) if error.kind() == ErrorKind::Conflict => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Autopilot accepts a ticket that waits for a person; its reason is the acceptance if none.
    async fn auto_accept(
        &self,
        detail: &TicketDetail,
        run: &Run,
        reason: &str,
    ) -> Result<TicketRecord, AppError> {
        let actor = self.starter.actor().await?;
        let record = detail.view.ticket.record();
        let key = record.key.to_string();
        let mut version = record.version;
        if record.acceptance.trim().is_empty() {
            let patch = TicketPatch {
                acceptance: Some(reason.to_owned()),
                ..TicketPatch::default()
            };
            let updated = self
                .tickets
                .update(&key, &patch, actor, version, WriteOptions::default())
                .await?;
            version = updated.view.ticket.record().version;
        }
        let body = format!("{AUTOPILOT_PREFIX} accepted automatically - {reason}");
        let posted = detail.comments.iter().any(|line| {
            let comment = line.comment.record();
            line.comment.body() == body
                && comment.system_marker.is_none()
                && comment.run == Some(run.id())
        });
        if !posted {
            self.comments
                .add_system(&record.key, &body, None, Some(run.id()), actor)
                .await?;
            version = self.tickets.get(&key).await?.view.ticket.record().version;
        }
        let accepted = self
            .tickets
            .accept(&key, actor, Some(version), false)
            .await?;
        Ok(accepted.ticket.record().clone())
    }
}

/// The agent the run worked under; runs from before it was kept use the configured name.
pub(super) fn agent_of(run: &Run, key: &str, config: &DispatcherConfig) -> String {
    run.dispatch()
        .and_then(|dispatch| dispatch.agent.clone())
        .unwrap_or_else(|| config.work_agent(key))
}

/// The outcome a run paused for `kind` is settled with.
const fn pause_outcome(kind: PauseKind) -> DispatchOutcome {
    match kind {
        PauseKind::UsageLimit => DispatchOutcome::PauseLimit,
        PauseKind::ClaudeMissing => DispatchOutcome::PauseClaudeMissing,
    }
}

/// Whether the abort comment of this run is on the ticket already.
fn abort_posted(detail: &TicketDetail, run: &Run, dispatcher: &Actor) -> bool {
    detail
        .comments
        .iter()
        .any(|line| is_abort_of(line.comment.record(), dispatcher.id(), run.id()))
}

/// Whether the question comment of this run is on the ticket already.
fn question_posted(detail: &TicketDetail, run: &Run) -> bool {
    detail.comments.iter().any(|line| {
        let record = line.comment.record();
        record.system_marker == Some(SystemMarker::Question) && record.run == Some(run.id())
    })
}

#[async_trait]
impl PassStep for EvaluateRuns {
    async fn run(&self, context: &PassContext, report: &mut PassReport) -> Result<(), AppError> {
        // read first: a run that starts meanwhile has its place in the list read afterwards
        let launching = self.starter.launching_keys();
        let finished = self.runs.unevaluated(BATCH).await?;
        let active = self.runs.active().await?;
        let working: HashSet<&str> = active
            .iter()
            .filter_map(Run::dispatch)
            .filter_map(|dispatch| dispatch.subject_key.as_deref())
            .collect();
        for run in &finished {
            let Some(dispatch) = run.dispatch() else {
                continue;
            };
            let Some(key) = dispatch.subject_key.as_deref() else {
                continue;
            };
            if dispatch.kind == RunKind::Research {
                self.settle_research(run, key, &context.config, report)
                    .await;
                continue;
            }
            if dispatch.kind == RunKind::Plan {
                self.settle_plan(run, key, &context.config, report).await;
                continue;
            }
            if dispatch.kind == RunKind::Merge {
                self.settle_merge(run, key, context, report).await;
                continue;
            }
            // a ticket with a live run waits
            if dispatch.kind != RunKind::Work || working.contains(key) || launching.contains(key) {
                continue;
            }
            match self.settle(run, key, &context.config).await {
                Ok(Settled::Yes(outcome)) => {
                    match self.runs.settle_outcome(run.id(), outcome).await {
                        Ok(_) => {
                            self.record_end(run).await;
                            self.log
                                .write(
                                    LogLevel::Info,
                                    &format!("settled {key}: {}", outcome.as_str()),
                                    Some(key),
                                    Some(run.id()),
                                )
                                .await;
                        }
                        Err(error) if error.kind() == ErrorKind::Conflict => {}
                        Err(error) => report.errors.push(format!("{key}: {}", error.message())),
                    }
                }
                Ok(Settled::Later) => {}
                Err(error) => report.errors.push(format!("{key}: {}", error.message())),
            }
        }
        Ok(())
    }
}
