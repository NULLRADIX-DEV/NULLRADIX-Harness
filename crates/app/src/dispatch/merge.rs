//! The merge pass steps: queue the merges autopilot forgot, then merge every open job per branch.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Actor, HOLD_NOTICE_MINUTES, HoldKind, MERGE_REASON, MergeJob, MergeStrategy, SystemMarker,
    Ticket, TicketRecord, TicketStatus, Timestamp, WorktreePurpose, finished_for_merge,
    merge_block_reason, merge_message, ready_to_merge,
};

use super::log::{DispatchLog, LogLevel};
use super::merge_git::{BranchDeletion, DeleteBranch, MergeGit, MergeOutcome, MergeRequest};
use super::merge_jobs::MergeQueue;
use super::pass::{PassContext, PassReport, PassStep};
use super::start::TicketStarter;
use crate::comments::CommentService;
use crate::notifier::{NoNotifier, Notice, NoticeKind, Notifier};
use crate::repositories::{RunnerService, runner_unreachable};
use crate::runs::RunService;
use crate::tickets::{TicketDetail, TicketFilter, TicketPatch, TicketService, WriteOptions};
use crate::worktrees::{WorktreeEntry, WorktreeRepository, WorktreeService};
use crate::{AppError, ChangeSignal, Clock, ErrorKind};
use nullradix_domain::Column;

/// What the merge needs to put a branch into its target.
pub struct MergeParts {
    pub queue: Arc<MergeQueue>,
    pub worktrees: Arc<dyn WorktreeRepository>,
    pub worktree_service: Arc<WorktreeService>,
    pub tickets: Arc<TicketService>,
    pub comments: Arc<CommentService>,
    pub runners: Arc<RunnerService>,
    pub git: Arc<dyn MergeGit>,
    pub starter: Arc<TicketStarter>,
    pub log: Arc<DispatchLog>,
    pub clock: Arc<dyn Clock>,
    pub signal: ChangeSignal,
}

/// Milliseconds in a minute.
const MILLIS_PER_MINUTE: i64 = 60_000;

/// Codes of a merge whose failure needs a person; any other failure is tried again.
const BLOCKING_CODES: [&str; 2] = ["merge-abort-failed", "merge-worktree-foreign"];

pub struct MergeAll {
    parts: MergeParts,
    notifier: Arc<dyn Notifier>,
}

impl std::fmt::Debug for MergeAll {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeAll").finish_non_exhaustive()
    }
}

/// The tickets a job merges, as they are now.
struct Branch {
    entry: WorktreeEntry,
    name: String,
    /// Live links whose ticket still exists, in join order.
    live: Vec<TicketDetail>,
}

impl Branch {
    fn on_branch(&self) -> impl Iterator<Item = &TicketDetail> {
        self.live
            .iter()
            .filter(|detail| detail.view.ticket.record().branch.as_deref() == Some(&self.name))
    }

    fn done(&self) -> Vec<&TicketRecord> {
        self.on_branch()
            .map(|detail| detail.view.ticket.record())
            .filter(|record| record.status == TicketStatus::Done)
            .collect()
    }
}

impl MergeAll {
    #[must_use]
    pub fn new(parts: MergeParts) -> Self {
        Self {
            parts,
            notifier: Arc::new(NoNotifier),
        }
    }

    /// Shows a notice when a branch is merged or the merge is blocked.
    #[must_use]
    pub fn with_notifier(mut self, notifier: Arc<dyn Notifier>) -> Self {
        self.notifier = notifier;
        self
    }

    async fn branch(&self, entry: WorktreeEntry) -> Result<Branch, AppError> {
        let mut live = Vec::new();
        for link in entry.tickets.iter().filter(|link| link.live()) {
            match self.parts.tickets.get(&link.key.to_string()).await {
                Ok(detail) => live.push(detail),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let name = entry.worktree.branch().as_str().to_owned();
        Ok(Branch { entry, name, live })
    }

    async fn save(
        &self,
        job: MergeJob,
        expected: nullradix_domain::Version,
    ) -> Result<(), AppError> {
        match self
            .parts
            .queue
            .repository()
            .save(&job, expected, self.parts.clock.now())
            .await?
        {
            Some(change) => {
                self.parts.signal.notify(&change);
                Ok(())
            }
            None => Err(AppError::conflict(
                "version-conflict",
                "the merge job changed while it was processed",
            )),
        }
    }

    async fn note(
        &self,
        keys: &[&TicketRecord],
        body: &str,
        marker: SystemMarker,
    ) -> Result<(), AppError> {
        let actor = self.parts.starter.actor().await?;
        for record in keys {
            self.parts
                .comments
                .add_system(&record.key, body, Some(marker), None, actor)
                .await?;
        }
        Ok(())
    }

    /// Nothing is left to merge: the ticket moved on or is gone.
    async fn cancel(&self, job: MergeJob) -> Result<(), AppError> {
        let version = job.record().version;
        let at = self.parts.clock.now();
        self.save(job.cancelled(at), version).await
    }

    /// Processes one job; the merged keys go into the report.
    async fn process(
        &self,
        job: MergeJob,
        context: &PassContext,
        report: &mut PassReport,
    ) -> Result<(), AppError> {
        let record = job.record().clone();
        let Some(entry) = self.parts.worktrees.get(record.worktree).await? else {
            return self.cancel(job).await;
        };
        if entry.worktree.record().purpose != WorktreePurpose::Work {
            return self.cancel(job).await;
        }
        let branch = self.branch(entry).await?;
        let done = branch.done();
        if done.is_empty() {
            return self.cancel(job).await;
        }
        let tickets: Vec<&Ticket> = branch
            .on_branch()
            .map(|detail| &detail.view.ticket)
            .collect();
        if !ready_to_merge(tickets.iter().copied()) {
            return self.wait(job, &branch).await;
        }
        // a conflict was seen and no merge session exists yet: the job stands still
        if record.conflict_seen {
            return Ok(());
        }
        let worktree = branch.entry.worktree.record();
        if worktree.runner != self.parts.runners.local().id() {
            return Err(runner_unreachable(worktree.runner));
        }
        let Some(root) = self
            .parts
            .runners
            .root(worktree.runner, worktree.repository)
            .await?
        else {
            return self
                .block(
                    job,
                    &branch,
                    context,
                    "the repository has no checkout on its runner",
                )
                .await;
        };
        self.land(job, &branch, &root, context, report).await
    }

    /// Merges the branch where its target is, or finishes a merge the target holds already.
    async fn land(
        &self,
        job: MergeJob,
        branch: &Branch,
        root: &str,
        context: &PassContext,
        report: &mut PassReport,
    ) -> Result<(), AppError> {
        let done = branch.done();
        let target = job.record().target_branch.clone();
        if let Some(target) = &target {
            match self.parts.git.ensure_target(root, target).await {
                Ok(Some(from)) => {
                    let body =
                        format!("[dispatch] merge: target branch {target} created from {from}.");
                    self.note(&done, &body, SystemMarker::Merge).await?;
                }
                Ok(None) => {}
                Err(error)
                    if error.code() == "git-failed" || BLOCKING_CODES.contains(&error.code()) =>
                {
                    let problem =
                        format!("target branch {target} is not usable: {}", error.message());
                    return self.block(job, branch, context, &problem).await;
                }
                Err(error) => return Err(error),
            }
        }
        // the merge is in the target and the branch is gone: finish instead of merging again
        if let Some(earlier) = self
            .parts
            .git
            .merged_earlier(root, &branch.name, target.as_deref())
            .await?
        {
            let landed = Landed {
                into: earlier.into,
                commit: earlier.commit,
                tip: None,
                warning: None,
            };
            return self
                .finish(job, branch, root, landed, context, report)
                .await;
        }
        let strategy = context.config.merge_strategy;
        let named: Vec<(String, String)> = done
            .iter()
            .map(|record| (record.key.to_string(), record.title.as_str().to_owned()))
            .collect();
        let request = MergeRequest {
            root: root.to_owned(),
            branch: branch.name.clone(),
            target,
            message: merge_message(&named, &branch.name, strategy),
            strategy,
        };
        match self.parts.git.merge(&request).await {
            Ok(MergeOutcome::Merged {
                into,
                commit,
                tip,
                warning,
            }) => {
                let landed = Landed {
                    into,
                    commit,
                    tip: Some(tip),
                    warning,
                };
                self.finish(job, branch, root, landed, context, report)
                    .await
            }
            Ok(MergeOutcome::Conflict { files }) => self.conflict(job, branch, &files).await,
            Ok(MergeOutcome::Held { kind, place, text }) => {
                self.hold(job, branch, (kind, &place, &text)).await
            }
            Ok(MergeOutcome::Refused { reason }) => self.block(job, branch, context, &reason).await,
            Err(error) if BLOCKING_CODES.contains(&error.code()) => {
                self.block(job, branch, context, error.message()).await
            }
            Err(error) => Err(error),
        }
    }

    /// Some ticket of the branch is open again: wait, and say so once.
    async fn wait(&self, job: MergeJob, branch: &Branch) -> Result<(), AppError> {
        let record = job.record();
        if record.phase == nullradix_domain::MergePhase::Waiting
            && record.wait_noted
            && job.held_text().is_none()
        {
            return Ok(());
        }
        let open: Vec<String> = branch
            .on_branch()
            .map(|detail| detail.view.ticket.record())
            .filter(|record| !finished_for_merge(record.status, record.blocked_reason.as_deref()))
            .map(|record| record.key.to_string())
            .collect();
        if !record.wait_noted {
            let body = format!(
                "[dispatch] merge-waiting: waiting for {} before {} is merged.",
                open.join(", "),
                branch.name
            );
            self.note(&branch.done(), &body, SystemMarker::MergeWait)
                .await?;
        }
        let version = record.version;
        self.save(job.waiting(true), version).await
    }

    /// The folder cannot take the merge yet: say so once per reason, tell the person after a while.
    async fn hold(
        &self,
        job: MergeJob,
        branch: &Branch,
        held: (HoldKind, &str, &str),
    ) -> Result<(), AppError> {
        let (kind, place, text) = held;
        let record = job.record();
        let line = format!("{place}: {text}");
        let fresh = record.held_by.as_deref() != Some(kind.as_str());
        let waited =
            (self.parts.clock.now().as_millis() - record.since.as_millis()) / MILLIS_PER_MINUTE;
        let notice = !record.hold_noticed && waited >= HOLD_NOTICE_MINUTES;
        if !fresh && !notice && job.held_text() == Some(line.as_str()) {
            return Ok(());
        }
        let done = branch.done();
        if fresh {
            let body = format!(
                "[dispatch] merge-waiting: waiting for {line} - merged automatically once that is \
                 done."
            );
            self.note(&done, &body, SystemMarker::MergeWait).await?;
        }
        let version = record.version;
        self.save(job.held(kind, &line, notice), version).await?;
        let Some(first) = done.first() else {
            return Ok(());
        };
        let key = first.key.to_string();
        if fresh {
            self.parts
                .log
                .write(
                    LogLevel::Info,
                    &format!("merge of {key} waits for {line}"),
                    Some(&key),
                    None,
                )
                .await;
        }
        if notice {
            let how_long = if waited < 120 {
                format!("{waited} minutes")
            } else {
                format!("{} hours", waited / 60)
            };
            self.parts
                .log
                .write(
                    LogLevel::Info,
                    &format!("merge of {key} has waited {how_long}: reported"),
                    Some(&key),
                    None,
                )
                .await;
            let notice = Notice {
                kind: NoticeKind::MergeWaiting,
                ticket: Some(key),
                title: first.title.as_str().to_owned(),
                text: format!("merge has waited {how_long} for {line}"),
            };
            self.toast(&notice).await;
        }
        Ok(())
    }

    /// The merge was undone; the job keeps the files for a merge session and stands still.
    async fn conflict(
        &self,
        job: MergeJob,
        branch: &Branch,
        files: &[String],
    ) -> Result<(), AppError> {
        let version = job.record().version;
        let job = job.waiting(false).conflicted(files);
        let problem = job.record().problem.clone().unwrap_or_default();
        let body = format!("[dispatch] merge-conflict: {problem}; the branch is not merged.");
        self.note(&branch.done(), &body, SystemMarker::MergeConflict)
            .await?;
        let key = branch.done().first().map(|record| record.key.to_string());
        self.parts
            .log
            .write(
                LogLevel::Info,
                &format!("{problem} on {}", branch.name),
                key.as_deref(),
                None,
            )
            .await;
        self.save(job, version).await
    }

    /// Marks every done ticket of the branch blocked with `Merge: <problem>` and ends the job.
    async fn block(
        &self,
        job: MergeJob,
        branch: &Branch,
        context: &PassContext,
        problem: &str,
    ) -> Result<(), AppError> {
        let reason = merge_block_reason(problem);
        let actor = self.parts.starter.actor().await?;
        let patch = TicketPatch {
            status: Some(TicketStatus::Blocked.as_str().to_owned()),
            blocked_reason: Some(Some(reason.clone())),
            ..TicketPatch::default()
        };
        let options = WriteOptions {
            force: true,
            ..WriteOptions::default()
        };
        let mut first: Option<TicketRecord> = None;
        for record in branch.done() {
            let key = record.key.to_string();
            let fresh = self.parts.tickets.get(&key).await?;
            let current = fresh.view.ticket.record();
            if current.status != TicketStatus::Done {
                continue;
            }
            self.parts
                .tickets
                .update(&key, &patch, actor, current.version, options)
                .await?;
            self.parts
                .comments
                .add_system(
                    &record.key,
                    &format!("[dispatch] merge: not possible: {problem}"),
                    Some(SystemMarker::Merge),
                    None,
                    actor,
                )
                .await?;
            first.get_or_insert_with(|| current.clone());
        }
        let version = job.record().version;
        let at = self.parts.clock.now();
        self.save(job.failed(problem, at), version).await?;
        let Some(first) = first else {
            return Ok(());
        };
        self.parts
            .log
            .write(
                LogLevel::Info,
                &format!("merge blocked {}: {problem}", first.key),
                Some(&first.key.to_string()),
                None,
            )
            .await;
        // the Merge inbox entry and the retry of a later slice hang here
        let notice = Notice::blocked(
            context.config.autopilot_applies(&first.labels),
            &first.key.to_string(),
            first.title.as_str(),
            &reason,
        );
        self.toast(&notice).await;
        Ok(())
    }

    async fn toast(&self, notice: &Notice) {
        if let Err(error) = self.notifier.notify(notice).await {
            tracing::warn!("cannot show the merge notice: {}", error.message());
        }
    }

    /// Cleans up after the merge, then notes it on every ticket and ends the job.
    async fn finish(
        &self,
        job: MergeJob,
        branch: &Branch,
        root: &str,
        landed: Landed,
        context: &PassContext,
        report: &mut PassReport,
    ) -> Result<(), AppError> {
        let actor = self.parts.starter.actor().await?;
        let mut warnings: Vec<String> = landed.warning.into_iter().collect();
        let path = branch.entry.worktree.path().as_str().to_owned();
        let folder = self
            .parts
            .worktree_service
            .remove_merged_folder(&branch.entry)
            .await?;
        let mut cleaned = true;
        if let Some(reason) = folder {
            warnings.push(format!("worktree {path} stays: {reason}"));
            cleaned = false;
        }
        if cleaned && let Some(tip) = landed.tip {
            let request = DeleteBranch {
                root: root.to_owned(),
                branch: branch.name.clone(),
                tip,
                into: landed.into.clone(),
            };
            if let BranchDeletion::Kept(reason) = self.parts.git.delete_merged(&request).await? {
                warnings.push(reason);
                cleaned = false;
            }
        }
        self.reopen_blocked(branch, actor).await?;
        if cleaned {
            self.parts
                .worktree_service
                .clear_merged_links(&branch.entry, actor)
                .await?;
        }
        let head = format!(
            "[dispatch] merged into {} ({}).",
            landed.into, landed.commit
        );
        let body = if warnings.is_empty() {
            format!(
                "{head} Branch {} cleaned up, worktree {path} removed.",
                branch.name
            )
        } else {
            format!("{head} Cleanup incomplete:\n{}", warnings.join("\n"))
        };
        let keys: Vec<&TicketRecord> = branch
            .live
            .iter()
            .map(|detail| detail.view.ticket.record())
            .collect();
        self.note(&keys, &body, SystemMarker::Merged).await?;
        let version = job.record().version;
        let at: Timestamp = self.parts.clock.now();
        let strategy: MergeStrategy = context.config.merge_strategy;
        self.save(
            job.merged(&landed.into, &landed.commit, strategy, at),
            version,
        )
        .await?;
        let names: Vec<String> = keys.iter().map(|record| record.key.to_string()).collect();
        self.parts
            .log
            .write(
                LogLevel::Info,
                &format!(
                    "merged {} into {} ({})",
                    names.join(", "),
                    landed.into,
                    landed.commit
                ),
                names.first().map(String::as_str),
                None,
            )
            .await;
        if let Some(first) = keys.first() {
            let notice = Notice {
                kind: NoticeKind::Merged,
                ticket: Some(first.key.to_string()),
                title: first.title.as_str().to_owned(),
                text: format!("merged into {} ({})", landed.into, landed.commit),
            };
            self.toast(&notice).await;
        }
        report.merged.extend(names);
        Ok(())
    }

    /// Tickets blocked by an earlier merge problem are done again.
    async fn reopen_blocked(&self, branch: &Branch, actor: &Actor) -> Result<(), AppError> {
        let patch = TicketPatch {
            status: Some(TicketStatus::Done.as_str().to_owned()),
            ..TicketPatch::default()
        };
        let options = WriteOptions {
            force: true,
            ..WriteOptions::default()
        };
        for detail in branch.on_branch() {
            let key = detail.view.ticket.record().key.to_string();
            // the ticket may have moved on since the merge started
            let fresh = self.parts.tickets.get(&key).await?;
            let record = fresh.view.ticket.record();
            let blocked = record.status == TicketStatus::Blocked
                && record
                    .blocked_reason
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with(MERGE_REASON);
            if !blocked {
                continue;
            }
            self.parts
                .tickets
                .update(&key, &patch, actor, record.version, options)
                .await?;
        }
        Ok(())
    }
}

/// Where the branch landed.
struct Landed {
    into: String,
    commit: String,
    /// Where the branch stood when it was merged; none when it was gone already.
    tip: Option<String>,
    warning: Option<String>,
}

#[async_trait]
impl PassStep for MergeAll {
    async fn run(&self, context: &PassContext, report: &mut PassReport) -> Result<(), AppError> {
        let jobs = self.parts.queue.repository().open().await?;
        for job in jobs {
            let id = job.record().worktree;
            if let Err(error) = self.process(job, context, report).await {
                report
                    .errors
                    .push(format!("merge {id}: {}", error.message()));
            }
        }
        Ok(())
    }
}

/// Queues the merges of tickets that autopilot finished and nobody queued, as legacy catch-up.
pub struct MergeCatchUp {
    queue: Arc<MergeQueue>,
    tickets: Arc<TicketService>,
    worktrees: Arc<dyn WorktreeRepository>,
    runners: Arc<RunnerService>,
    runs: Arc<RunService>,
    git: Arc<dyn MergeGit>,
}

impl std::fmt::Debug for MergeCatchUp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeCatchUp").finish_non_exhaustive()
    }
}

impl MergeCatchUp {
    #[must_use]
    pub fn new(
        queue: Arc<MergeQueue>,
        tickets: Arc<TicketService>,
        worktrees: Arc<dyn WorktreeRepository>,
        runners: Arc<RunnerService>,
        runs: Arc<RunService>,
        git: Arc<dyn MergeGit>,
    ) -> Self {
        Self {
            queue,
            tickets,
            worktrees,
            runners,
            runs,
            git,
        }
    }
}

#[async_trait]
impl PassStep for MergeCatchUp {
    async fn run(&self, context: &PassContext, _report: &mut PassReport) -> Result<(), AppError> {
        if !context.config.autopilot.enabled {
            return Ok(());
        }
        let working: HashSet<String> = self
            .runs
            .active()
            .await?
            .iter()
            .filter_map(|run| run.dispatch())
            .filter_map(|dispatch| dispatch.subject_key.clone())
            .collect();
        let filter = TicketFilter {
            columns: Some(vec![Column::Done]),
            ..TicketFilter::default()
        };
        for view in self.tickets.list(&filter).await? {
            let record = view.ticket.record();
            let key = record.key.to_string();
            let ours = record
                .assignee
                .as_deref()
                .is_some_and(|assignee| assignee.starts_with(&context.config.agent_prefix));
            if record.status != TicketStatus::Done
                || record.branch.is_none()
                || !ours
                || !context.config.autopilot_applies(&record.labels)
                || working.contains(&key)
            {
                continue;
            }
            let Some(entry) = self.worktrees.of_ticket(record.id).await? else {
                continue;
            };
            let worktree = entry.worktree.record();
            if worktree.runner != self.runners.local().id() {
                continue;
            }
            let Some(root) = self
                .runners
                .root(worktree.runner, worktree.repository)
                .await?
            else {
                continue;
            };
            let target = record
                .target_branch
                .as_ref()
                .or(worktree.target_branch.as_ref())
                .map(|target| target.as_str().to_owned());
            // an open job has the branch in hand
            if self
                .queue
                .repository()
                .open_of(entry.worktree.id())
                .await?
                .is_some()
            {
                continue;
            }
            // a branch that is gone or already in the target has nothing to catch up
            let branch = worktree.branch.as_str();
            if self.git.branch_tip(&root, branch).await?.is_none()
                || self
                    .git
                    .integrated(&root, branch, target.as_deref())
                    .await?
            {
                continue;
            }
            self.queue.queue(record).await?;
        }
        Ok(())
    }
}
