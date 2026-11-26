//! Merge sessions: Claude merges the target into the branch where a conflict stopped the merge.

use std::path::PathBuf;

use nullradix_domain::{
    ActorKind, DispatchOutcome, LabelOverrides, MergePhase, ProfileChoice, ReportComment, Run,
    RunKind, RunStatus, RunTrigger, SystemMarker, TicketStatus, choose_profile, escalate,
    failed_attempts, hold_files, label_overrides, merge_session_choice, merge_session_limit,
    previous_attempts, session_failed_note, session_started_note,
};

use super::{Branch, MergeAll};
use crate::dispatch::accounts::Chosen;
use crate::dispatch::accounts::{AccountPick, AccountRequest, PickMode};
use crate::dispatch::log::LogLevel;
use crate::dispatch::merge_git::{SessionCheck, SessionVerdict, TargetTip};
use crate::dispatch::merge_jobs::merge_session_open;
use crate::dispatch::pass::{PassContext, idle_message};
use crate::dispatch::runs::StartDispatchRun;
use crate::repositories::runner_unreachable;
use crate::tickets::TicketDetail;
use crate::worktrees::WorktreeEntry;
use crate::{AppError, ErrorKind};

use nullradix_domain::MergeJob;

/// Errors that say nothing about the merge itself: the next pass tries the same step again.
fn transient(error: &AppError) -> bool {
    error.kind() == ErrorKind::Internal
        || matches!(error.code(), "version-conflict" | "runner-unreachable")
}

/// The keys of the tickets, comma separated.
fn key_list(records: &[&nullradix_domain::TicketRecord]) -> String {
    let keys: Vec<String> = records
        .iter()
        .map(|record| record.key.to_string())
        .collect();
    keys.join(", ")
}

/// How a session ended, for the comment and the block reason.
fn ending(run: &Run) -> String {
    let how = if run.status() == RunStatus::TimedOut {
        String::from("time limit")
    } else {
        format!(
            "exit code {}",
            run.exit_code()
                .map_or_else(|| String::from("none"), |code| code.to_string())
        )
    };
    format!("{how}, run {}", run.id())
}

impl MergeAll {
    /// Whether a merge session of the worktree's tickets has not been settled yet.
    pub(super) async fn session_open(&self, entry: &WorktreeEntry) -> Result<bool, AppError> {
        let subjects: Vec<String> = entry
            .tickets
            .iter()
            .filter(|link| link.live())
            .map(|link| link.key.to_string())
            .collect();
        merge_session_open(&self.parts.runs, subjects).await
    }

    /// The merge was undone: a session resolves the conflict, or the job waits for one.
    pub(super) async fn conflict(
        &self,
        job: MergeJob,
        branch: &Branch,
        root: &str,
        files: &[String],
        context: &PassContext,
    ) -> Result<(), AppError> {
        let list = hold_files(files);
        if job.record().session_used {
            let problem =
                format!("merge conflict in {list} - the merge session did not resolve it");
            let detail = format!("merge conflict in {list} persists");
            return self
                .session_failed(job, branch, context, &problem, &detail)
                .await;
        }
        if let Some(reason) = &context.idle {
            let version = job.record().version;
            let key = branch.done().first().map(|record| record.key.to_string());
            let text = idle_message(reason).to_lowercase();
            self.parts
                .log
                .write(
                    LogLevel::Info,
                    &format!(
                        "merge conflict {} - {text}, the merge session waits",
                        key.as_deref().unwrap_or(&branch.name)
                    ),
                    key.as_deref(),
                    None,
                )
                .await;
            return self
                .save(job.waiting(false).conflicted(files), version)
                .await;
        }
        match self.start_session(&job, branch, root, files, context).await {
            Err(error) if transient(&error) => Err(error),
            // without a session the job would come back every pass: a person has to look
            Err(error) => {
                let problem = format!(
                    "merge conflict in {list}; the merge session did not start: {}",
                    error.message()
                );
                self.block(job, branch, context, &problem).await
            }
            Ok(()) => Ok(()),
        }
    }

    /// The profile of the session, with label overrides and the account it runs with.
    async fn pick_session(
        &self,
        detail: &TicketDetail,
        session: u32,
        context: &PassContext,
    ) -> Result<(ProfileChoice, LabelOverrides, Chosen), AppError> {
        let config = &context.config;
        let record = detail.view.ticket.record();
        let actor = self.parts.starter.actor().await?;
        let earlier = failed_attempts(
            detail.comments.iter().map(|line| line.comment.record()),
            actor.id(),
        );
        let base = escalate(
            config,
            choose_profile(config, &record.labels, record.priority, &record.plan),
            earlier + 1,
        );
        let autopilot = config.autopilot_applies(&record.labels);
        let choice = merge_session_choice(config, base.clone(), session, autopilot);
        // a profile the session replaced takes no label override with it
        let overrides = if choice.profile == base.profile {
            label_overrides(config, &record.labels)
        } else {
            LabelOverrides::default()
        };
        let mut request = AccountRequest::for_ticket(config, &choice, &record.labels);
        if let Some(model) = &overrides.model {
            request.model = Some(model.clone());
        }
        if let Some(effort) = &overrides.effort {
            request.effort = Some(effort.clone());
        }
        match self
            .parts
            .accounts
            .pick(&request, config, PickMode::Start)
            .await?
        {
            AccountPick::Picked(chosen) => Ok((choice, overrides, chosen)),
            AccountPick::Waiting { reason, .. } => Err(AppError::unavailable(
                "account-unavailable",
                format!("no Claude account is free - {reason}"),
            )),
        }
    }

    /// Starts the session in the worktree of the first finished ticket and notes it on all.
    async fn start_session(
        &self,
        job: &MergeJob,
        branch: &Branch,
        root: &str,
        files: &[String],
        context: &PassContext,
    ) -> Result<(), AppError> {
        let config = &context.config;
        let done = branch.done();
        let Some(first) = done.first() else {
            return Ok(());
        };
        let Some(detail) = branch
            .on_branch()
            .find(|detail| detail.view.ticket.record().key == first.key)
        else {
            return Ok(());
        };
        let key = first.key.to_string();
        let tip = self
            .parts
            .git
            .target_tip(root, job.record().target_branch.as_deref())
            .await?
            .ok_or_else(|| {
                AppError::invalid("merge-target-missing", "the target branch has no commit")
            })?;
        let branch_tip = self
            .parts
            .git
            .branch_tip(root, &branch.name)
            .await?
            .ok_or_else(|| AppError::invalid("merge-branch-missing", "the branch has no commit"))?;
        let sessions = job.record().conflict_sessions + 1;
        let autopilot = config.autopilot_applies(&first.labels);
        let max = merge_session_limit(config);
        let (choice, overrides, chosen) = self.pick_session(detail, sessions, context).await?;
        let agent = config.work_agent(&key);
        let previous = (job.record().conflict_sessions > 0).then(|| {
            let lines: Vec<ReportComment<'_>> = detail
                .comments
                .iter()
                .map(|line| ReportComment {
                    by_dispatcher: line.author_kind == ActorKind::Dispatcher,
                    author: &line.author_name,
                    body: line.comment.body(),
                })
                .collect();
            previous_attempts(&lines, &agent)
        });
        let folder = self.parts.worktree_service.ensure(&key).await?;
        let prompt = self
            .parts
            .prompts
            .merge_prompt(&key, &branch.name, &tip.into, previous)
            .await?;
        let run = self
            .parts
            .dispatch
            .start(StartDispatchRun {
                kind: RunKind::Merge,
                trigger: RunTrigger::Dispatcher,
                subject: key.clone(),
                prompt,
                cwd: PathBuf::from(&folder.path),
                profile: Some(choice.profile.clone()),
                overrides,
                account: Some(chosen.account),
                extra_tools: Vec::new(),
                agent: Some(agent),
            })
            .await?;
        let version = job.record().version;
        let started = job
            .clone()
            .session_started(sessions, &tip.tip, &branch_tip, files);
        if let Err(error) = self.save(started, version).await {
            // the job does not know the run: it must not run on its own
            if let Err(stop) = self.parts.runs.cancel(run.id()).await {
                tracing::warn!("cannot stop the merge session: {}", stop.message());
            }
            return Err(error);
        }
        self.parts.accounts.record_start(run.id(), &chosen).await;
        let numbered = autopilot.then_some((sessions, max));
        let body = session_started_note(files, &tip.into, &choice.profile, numbered);
        let names = self.note_run(&done, &body, run.id()).await?;
        self.parts
            .log
            .write(
                LogLevel::Info,
                &format!(
                    "merge conflict {names} ({}) - merge session started",
                    files.join(", ")
                ),
                Some(&key),
                Some(run.id()),
            )
            .await;
        Ok(())
    }

    /// A conflict comment on every finished ticket; says which tickets it reached.
    async fn note_run(
        &self,
        records: &[&nullradix_domain::TicketRecord],
        body: &str,
        run: nullradix_domain::Id<Run>,
    ) -> Result<String, AppError> {
        let actor = self.parts.starter.actor().await?;
        for record in records {
            self.parts
                .comments
                .add_system(
                    &record.key,
                    body,
                    Some(SystemMarker::MergeConflict),
                    Some(run),
                    actor,
                )
                .await?;
        }
        Ok(key_list(records))
    }

    /// A session ended without a solution: autopilot tries again up to its limit, else a person.
    pub(super) async fn session_failed(
        &self,
        job: MergeJob,
        branch: &Branch,
        context: &PassContext,
        problem: &str,
        detail: &str,
    ) -> Result<(), AppError> {
        let done = branch.done();
        let Some(first) = done.first() else {
            return self.cancel(job).await;
        };
        if !context.config.autopilot_applies(&first.labels) {
            return self.block(job, branch, context, problem).await;
        }
        let used = job.record().conflict_sessions.max(1);
        let max = merge_session_limit(&context.config);
        if used >= max {
            let text = format!("autopilot - {used} merge sessions without solution ({detail}).");
            return self.block(job, branch, context, &text).await;
        }
        let version = job.record().version;
        self.save(job.next_session(), version).await?;
        let body = session_failed_note(used, max, detail);
        self.note(&done, &body, SystemMarker::MergeConflict).await?;
        let names = key_list(&done);
        self.parts
            .log
            .write(
                LogLevel::Info,
                &format!("merge session {used}/{max} {names} without solution - next attempt"),
                Some(&first.key.to_string()),
                None,
            )
            .await;
        Ok(())
    }

    /// Settles a finished merge session: the job merges again, tries again or is blocked.
    pub async fn settle_session(
        &self,
        run: &Run,
        context: &PassContext,
    ) -> Result<DispatchOutcome, AppError> {
        let outcome = DispatchOutcome::MergeSession;
        let Some(key) = run
            .dispatch()
            .and_then(|dispatch| dispatch.subject_key.as_deref())
        else {
            return Ok(DispatchOutcome::Other);
        };
        let detail = match self.parts.tickets.get(key).await {
            Ok(detail) => detail,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(outcome),
            Err(error) => return Err(error),
        };
        let Some(entry) = self
            .parts
            .worktrees
            .of_ticket(detail.view.ticket.record().id)
            .await?
        else {
            return Ok(outcome);
        };
        let Some(job) = self
            .parts
            .queue
            .repository()
            .open_of(entry.worktree.id())
            .await?
        else {
            return Ok(outcome);
        };
        // a retry or a cancel took the job from this session meanwhile
        if job.record().phase != MergePhase::ConflictSession {
            return Ok(outcome);
        }
        let branch = self.branch(entry.clone()).await?;
        match self.evaluate_session(job, &branch, run, key, context).await {
            Ok(()) => Ok(outcome),
            Err(error) if transient(&error) => Err(error),
            Err(error) => {
                // a job that cannot be evaluated would hang: a person has to look
                let Some(job) = self
                    .parts
                    .queue
                    .repository()
                    .open_of(entry.worktree.id())
                    .await?
                else {
                    return Ok(outcome);
                };
                let problem = format!(
                    "the merge session could not be evaluated: {}",
                    error.message()
                );
                if branch.done().is_empty() {
                    self.cancel(job).await?;
                } else {
                    self.block(job, &branch, context, &problem).await?;
                }
                Ok(outcome)
            }
        }
    }

    async fn evaluate_session(
        &self,
        job: MergeJob,
        branch: &Branch,
        run: &Run,
        key: &str,
        context: &PassContext,
    ) -> Result<(), AppError> {
        let on_branch = branch
            .on_branch()
            .map(|detail| detail.view.ticket.record())
            .find(|record| record.key.to_string() == key);
        // the ticket of the run moved on: the others of the worktree carry on
        if on_branch.is_none_or(|record| record.status != TicketStatus::Done) {
            let version = job.record().version;
            return self.save(job.pending_again(), version).await;
        }
        if branch.done().is_empty() {
            return self.cancel(job).await;
        }
        let worktree = branch.entry.worktree.record();
        if worktree.runner != self.parts.runners.local().id() {
            return Err(runner_unreachable(worktree.runner));
        }
        let root = self
            .parts
            .runners
            .root(worktree.runner, worktree.repository)
            .await?;
        let record = job.record();
        let target = record.target_branch.clone();
        let stored = record.session_target().map(str::to_owned);
        let before = record.session_branch_tip().map(str::to_owned);
        let version = record.version;
        let cwd = run
            .dispatch()
            .map_or_else(String::new, |dispatch| dispatch.cwd.clone());
        let (verdict, now) = match &root {
            Some(root) => {
                let now: Option<TargetTip> =
                    self.parts.git.target_tip(root, target.as_deref()).await?;
                let head = stored
                    .clone()
                    .or_else(|| now.as_ref().map(|found| found.tip.clone()));
                let verdict = match head {
                    Some(head) => {
                        let check = SessionCheck {
                            root,
                            target_head: &head,
                            branch_tip: before.as_deref(),
                            branch: &branch.name,
                            folder: &cwd,
                        };
                        self.parts.git.session_resolved(&check).await?
                    }
                    None => SessionVerdict::TargetMissing,
                };
                (verdict, now)
            }
            None => (SessionVerdict::TargetMissing, None),
        };
        if let SessionVerdict::BadCommit(id) = &verdict {
            return Err(AppError::invalid(
                "merge-commit-invalid",
                format!("\"{id}\" is not a commit id"),
            ));
        }
        if verdict != SessionVerdict::Resolved {
            let ended = ending(run);
            let (what, detail) = match &verdict {
                SessionVerdict::WorkLost => {
                    let what = String::from("dropped the work of the branch");
                    let detail = format!("{what}, {ended}");
                    (what, detail)
                }
                SessionVerdict::Markers(files) => {
                    let what = format!("left conflict markers in {}", hold_files(files));
                    let detail = format!("{what}, {ended}");
                    (what, detail)
                }
                _ => (
                    String::from("could not resolve the conflict"),
                    ended.clone(),
                ),
            };
            let problem = format!("the merge session {what} ({ended}).");
            return self
                .session_failed(job, branch, context, &problem, &detail)
                .await;
        }
        // a target that moved on during the session gets a fresh chance at a session
        let moved = matches!(
            (&stored, &now),
            (Some(head), Some(found)) if found.tip != *head
        );
        self.save(job.session_resolved(moved), version).await?;
        if moved {
            self.parts
                .log
                .write(
                    LogLevel::Info,
                    &format!(
                        "merge session of {key} resolved, the target moved on - merging again"
                    ),
                    Some(key),
                    Some(run.id()),
                )
                .await;
        }
        Ok(())
    }
}
