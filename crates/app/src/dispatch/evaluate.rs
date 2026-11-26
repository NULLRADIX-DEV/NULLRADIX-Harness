//! The pass step that settles finished work runs: done, human, blocked and question outcomes.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    AUTOPILOT_PREFIX, DispatchOutcome, DispatcherConfig, QuestionKey, Run, RunKind, SystemMarker,
    TicketRecord, TicketStatus,
};

use super::log::{DispatchLog, LogLevel};
use super::outcome::{Decision, decide, question_comment};
use super::pass::{PassContext, PassReport, PassStep};
use super::start::TicketStarter;
use crate::comments::CommentService;
use crate::runs::RunService;
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
    gate: Arc<dyn DoneGate>,
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
    ) -> Self {
        Self {
            tickets,
            comments,
            runs,
            starter,
            log,
            gate: Arc::new(NoDoneGate),
        }
    }

    /// Replaces what follows a finished ticket.
    #[must_use]
    pub fn with_gate(mut self, gate: Arc<dyn DoneGate>) -> Self {
        self.gate = gate;
        self
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
        let record = detail.view.ticket.record();
        let questions: Vec<QuestionKey> = detail
            .view
            .open_questions
            .iter()
            .map(|question| question.key.clone())
            .collect();
        // a pass that stopped after the comment is picked up where it was
        let asked = question_posted(&detail, run);
        let decision = if asked {
            Decision::Question(questions)
        } else {
            decide(config, &agent_of(run, key, config), run, record, &questions)
        };
        let outcome = match &decision {
            Decision::Pending(_) => return Ok(Settled::Later),
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
            other => other.outcome().unwrap_or(DispatchOutcome::Other),
        };
        Ok(Settled::Yes(outcome))
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
            .release(&key.to_string(), Some("planned"), actor, Some(version))
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
fn agent_of(run: &Run, key: &str, config: &DispatcherConfig) -> String {
    run.dispatch()
        .and_then(|dispatch| dispatch.agent.clone())
        .unwrap_or_else(|| config.work_agent(key))
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
            // research, plan and merge runs have their own slices; a ticket with a live run waits
            if dispatch.kind != RunKind::Work || working.contains(key) {
                continue;
            }
            match self.settle(run, key, &context.config).await {
                Ok(Settled::Yes(outcome)) => {
                    match self.runs.settle_outcome(run.id(), outcome).await {
                        Ok(_) => {
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
