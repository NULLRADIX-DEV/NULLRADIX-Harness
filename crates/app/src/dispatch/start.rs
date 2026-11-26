//! Taking a ready ticket: claim it, bring its worktree up, run the setup and start Claude.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nullradix_domain::{
    Actor, ActorKind, ActorName, DispatcherConfig, Id, ProfileChoice, Run, RunKind, RunTrigger,
    SETUP_REASON, SetupState, SystemMarker, TicketKey, TicketStatus, Version, choose_profile,
};
use parking_lot::Mutex;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use super::candidates::{Candidate, Candidates};
use super::log::DispatchLog;
use super::runs::{DispatchRuns, StartDispatchRun};
use super::setup::{SetupOutcome, SetupRequest, WorktreeSetupPort};
use crate::authors::author_invalid;
use crate::boards::ActorRepository;
use crate::comments::CommentService;
use crate::dispatcher_prompts::DispatcherPromptService;
use crate::repositories::RunnerService;
use crate::runs::RunService;
use crate::tickets::{TicketPatch, TicketService, WriteOptions};
use crate::worktrees::{Resumed, StartMode, WorktreeService};
use crate::{AppError, Clock, ErrorKind, IdGenerator};

/// Name of the actor that writes for the dispatcher.
pub const DISPATCHER_NAME: &str = "dispatcher";

/// Output of a failed setup step that goes into the comment.
const OUTPUT_TAIL_CHARS: usize = 2000;
/// Longest step text in a block reason.
const STEP_REASON_CHARS: usize = 200;

/// How taking a ticket ended.
#[derive(Debug, Clone, PartialEq)]
pub enum StartOutcome {
    Started(Id<Run>),
    /// The setup commands run in the background; Claude starts when they succeed.
    SettingUp,
    /// The ticket stays where it is; the text says why.
    Skipped(String),
    /// Starting failed after the ticket was claimed, and it was given back.
    Failed(String),
}

/// The pieces of a start that the background part needs as well.
struct Launch {
    key: TicketKey,
    agent: String,
    branch: String,
    cwd: String,
    choice: ProfileChoice,
    notes: Vec<String>,
    /// The version of the ticket after the last write of this start.
    version: Version,
}

pub struct TicketStarter {
    candidates: Arc<Candidates>,
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
    worktrees: Arc<WorktreeService>,
    runners: Arc<RunnerService>,
    runs: Arc<RunService>,
    dispatch: Arc<DispatchRuns>,
    prompts: Arc<DispatcherPromptService>,
    setup: Arc<dyn WorktreeSetupPort>,
    actors: Arc<dyn ActorRepository>,
    log: Arc<DispatchLog>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    actor: OnceCell<Actor>,
    /// Tickets whose setup runs in the background, with its folder; they count for the limit.
    launching: Mutex<HashMap<String, String>>,
    /// Tickets whose start failed, with their version afterwards; they wait for a change.
    failed: Mutex<HashMap<String, Version>>,
    /// Stops the setups that run in the background.
    cancel: CancellationToken,
}

impl std::fmt::Debug for TicketStarter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TicketStarter").finish_non_exhaustive()
    }
}

/// Everything a starter works with.
pub struct StarterParts {
    pub candidates: Arc<Candidates>,
    pub tickets: Arc<TicketService>,
    pub comments: Arc<CommentService>,
    pub worktrees: Arc<WorktreeService>,
    pub runners: Arc<RunnerService>,
    pub runs: Arc<RunService>,
    pub dispatch: Arc<DispatchRuns>,
    pub prompts: Arc<DispatcherPromptService>,
    pub setup: Arc<dyn WorktreeSetupPort>,
    pub actors: Arc<dyn ActorRepository>,
    pub log: Arc<DispatchLog>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
}

fn same_folder(left: &str, right: &str) -> bool {
    let plain = |text: &str| text.replace('\\', "/").trim_end_matches('/').to_owned();
    plain(left) == plain(right)
}

fn tail(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.trim().to_owned();
    }
    text.chars()
        .skip(count - limit)
        .collect::<String>()
        .trim()
        .to_owned()
}

impl TicketStarter {
    #[must_use]
    pub fn new(parts: StarterParts) -> Arc<Self> {
        Arc::new(Self {
            candidates: parts.candidates,
            tickets: parts.tickets,
            comments: parts.comments,
            worktrees: parts.worktrees,
            runners: parts.runners,
            runs: parts.runs,
            dispatch: parts.dispatch,
            prompts: parts.prompts,
            setup: parts.setup,
            actors: parts.actors,
            log: parts.log,
            clock: parts.clock,
            ids: parts.ids,
            actor: OnceCell::new(),
            launching: Mutex::default(),
            failed: Mutex::default(),
            cancel: CancellationToken::new(),
        })
    }

    /// Tickets that are being set up right now.
    #[must_use]
    pub fn launching(&self) -> usize {
        self.launching.lock().len()
    }

    /// The tickets that are being set up right now.
    #[must_use]
    pub fn launching_keys(&self) -> HashSet<String> {
        self.launching.lock().keys().cloned().collect()
    }

    /// Whether a setup runs in that folder right now.
    fn sets_up_in(&self, folder: &str) -> bool {
        self.launching
            .lock()
            .values()
            .any(|running| same_folder(running, folder))
    }

    /// Stops the setups that run, kills their processes and waits for them up to `limit`.
    pub async fn shutdown(&self, limit: Duration) {
        self.cancel.cancel();
        if tokio::time::timeout(limit, self.wait_idle()).await.is_err() {
            self.log.error("setups did not stop in time", None).await;
        }
    }

    /// Waits until no setup runs in the background any more.
    pub async fn wait_idle(&self) {
        while self.launching() > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The actor the dispatcher writes as.
    pub async fn actor(&self) -> Result<&Actor, AppError> {
        self.actor
            .get_or_try_init(|| async {
                let name =
                    ActorName::parse(DISPATCHER_NAME).map_err(|error| author_invalid(&error))?;
                let candidate = Actor::new(
                    self.ids.as_ref().next(),
                    ActorKind::Dispatcher,
                    name,
                    self.clock.now(),
                );
                self.actors.ensure(&candidate).await
            })
            .await
    }

    /// Takes the ticket if it is still ready; a failure after the claim gives it back.
    pub async fn start(
        self: &Arc<Self>,
        candidate: &Candidate,
        config: &DispatcherConfig,
    ) -> StartOutcome {
        let key = candidate.key().to_string();
        match self.take(candidate.key(), config).await {
            Ok(outcome) => outcome,
            Err(error) => {
                let message = error.message().to_owned();
                self.log
                    .error(&format!("cannot start {key}: {message}"), Some(&key))
                    .await;
                StartOutcome::Failed(message)
            }
        }
    }

    async fn take(
        self: &Arc<Self>,
        key: &TicketKey,
        config: &DispatcherConfig,
    ) -> Result<StartOutcome, AppError> {
        if self.cancel.is_cancelled() {
            return Ok(StartOutcome::Skipped(String::from("shutting down")));
        }
        let Some(fresh) = self.candidates.fresh(config, key).await? else {
            return Ok(StartOutcome::Skipped(String::from("no longer planned")));
        };
        if let Some(reason) = &fresh.not_ready {
            return Ok(StartOutcome::Skipped(reason.to_string()));
        }
        let Some(root) = fresh.root.clone() else {
            return Ok(StartOutcome::Skipped(String::from(
                "the repository is not reachable",
            )));
        };
        let record = fresh.ticket().record();
        if self.waits_for_change(&key.to_string(), record.version) {
            return Ok(StartOutcome::Skipped(String::from(
                "the last start failed; the ticket waits for a change",
            )));
        }
        // a run in the folder of the ticket must end first, the ticket stays unclaimed meanwhile
        if let Some(path) = &record.worktree
            && self.busy(path).await?
        {
            return Ok(StartOutcome::Skipped(String::from(
                "the worktree is busy with a run",
            )));
        }
        if let Some(path) = &record.worktree
            && self.sets_up_in(path)
        {
            return Ok(StartOutcome::Skipped(String::from(
                "a setup runs in the worktree",
            )));
        }
        let choice = choose_profile(config, &record.labels, record.priority, &record.plan);
        let actor = self.actor().await?;
        let text = key.to_string();
        let agent = config.work_agent(&text);
        let claimed = self
            .tickets
            .claim(
                &text,
                &agent,
                Some("planned"),
                actor,
                Some(record.version),
                false,
            )
            .await;
        let version = match claimed {
            Ok(view) => view.ticket.record().version,
            Err(error) => {
                return if error.kind() == ErrorKind::Internal {
                    Err(error)
                } else {
                    Ok(StartOutcome::Skipped(error.message().to_owned()))
                };
            }
        };
        let launched = self
            .launch(&fresh, &root, config, choice, agent, version)
            .await;
        if launched.is_err() {
            self.remember_failure(&text).await;
        }
        launched
    }

    /// Whether the ticket is unchanged since a start of it failed.
    fn waits_for_change(&self, key: &str, version: Version) -> bool {
        let mut failed = self.failed.lock();
        match failed.get(key) {
            Some(known) if *known == version => true,
            Some(_) => {
                failed.remove(key);
                false
            }
            None => false,
        }
    }

    /// Keeps the version a failed start left, so the next pass does not try it again.
    async fn remember_failure(&self, key: &str) {
        if let Ok(detail) = self.tickets.get(key).await {
            let version = detail.view.ticket.record().version;
            self.failed.lock().insert(key.to_owned(), version);
        }
    }

    async fn busy(&self, path: &str) -> Result<bool, AppError> {
        Ok(self
            .runs
            .active()
            .await?
            .iter()
            .filter_map(Run::dispatch)
            .any(|dispatch| same_folder(&dispatch.cwd, path)))
    }

    /// The worktree, then the setup and the run; what fails or waits gives the claim back.
    async fn launch(
        self: &Arc<Self>,
        candidate: &Candidate,
        root: &str,
        config: &DispatcherConfig,
        choice: ProfileChoice,
        agent: String,
        claimed: Version,
    ) -> Result<StartOutcome, AppError> {
        let mut version = claimed;
        let outcome = self
            .place_and_run(candidate, root, config, choice, agent, &mut version)
            .await;
        if !matches!(
            outcome,
            Ok(StartOutcome::Started(_) | StartOutcome::SettingUp)
        ) {
            self.give_back(candidate.key(), config, version).await;
        }
        outcome
    }

    async fn place_and_run(
        self: &Arc<Self>,
        candidate: &Candidate,
        root: &str,
        config: &DispatcherConfig,
        choice: ProfileChoice,
        agent: String,
        version: &mut Version,
    ) -> Result<StartOutcome, AppError> {
        let actor = self.actor().await?;
        let key = candidate.key().clone();
        let text = key.to_string();
        let mut notes = Vec::new();
        let mut placed = None;
        if candidate.ticket().record().branch.is_some() {
            match self.worktrees.resume(&text, actor).await? {
                Resumed::Ready(worktree) => {
                    let moved = self
                        .tickets
                        .move_to(
                            &text,
                            Some("in_progress"),
                            None,
                            actor,
                            None,
                            WriteOptions::default(),
                        )
                        .await?;
                    *version = moved.ticket.record().version;
                    placed = Some((worktree.branch, worktree.path, false));
                }
                Resumed::Reset(note) => {
                    notes.push(note);
                    *version = self.tickets.get(&text).await?.view.ticket.record().version;
                }
                Resumed::Skipped(reason) => return Ok(StartOutcome::Skipped(reason)),
            }
        }
        let (branch, cwd, created) = if let Some(placed) = placed {
            placed
        } else {
            let mode = match config.join_worktrees {
                nullradix_domain::JoinWorktrees::SameSpec => StartMode::AutoJoin,
                nullradix_domain::JoinWorktrees::None => StartMode::New,
            };
            let started = self
                .worktrees
                .start(&text, self.runners.local().id(), root, mode, false, actor)
                .await?;
            *version = started.ticket.ticket.record().version;
            notes.extend(started.warnings);
            if let Some(created) = started.target_created {
                let body = format!(
                    "[dispatch] merge: target branch {} created from {}",
                    created.branch, created.from
                );
                self.note(&key, &body, SystemMarker::Merge, None).await;
            }
            (started.branch, started.worktree, !started.joined)
        };
        let launch = Launch {
            key,
            agent,
            branch,
            cwd,
            choice,
            notes,
            version: *version,
        };
        if self.sets_up_in(&launch.cwd) {
            return Ok(StartOutcome::Skipped(String::from(
                "a setup runs in the worktree",
            )));
        }
        if self.needs_setup(config, created, &text).await? {
            return Ok(if self.defer(config.clone(), launch) {
                StartOutcome::SettingUp
            } else {
                StartOutcome::Skipped(String::from("shutting down"))
            });
        }
        Ok(match self.begin_run(&launch, config).await? {
            Some(run) => StartOutcome::Started(run.id()),
            None => StartOutcome::Skipped(String::from("the ticket changed while the run started")),
        })
    }

    /// Setup runs for a new worktree and for one whose setup did not finish.
    async fn needs_setup(
        &self,
        config: &DispatcherConfig,
        created: bool,
        key: &str,
    ) -> Result<bool, AppError> {
        if config.worktree_setup.is_empty() {
            return Ok(false);
        }
        let Some((worktree, state)) = self.worktrees.setup_state(key).await? else {
            return Ok(false);
        };
        let unfinished = matches!(state, SetupState::Pending | SetupState::Failed);
        if created || unfinished {
            self.worktrees
                .mark_setup(worktree, SetupState::Pending)
                .await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Runs the setup apart from the pass; false when shutting down, then nothing runs.
    fn defer(self: &Arc<Self>, config: DispatcherConfig, launch: Launch) -> bool {
        let text = launch.key.to_string();
        {
            // checked under the lock that wait_idle reads, so no setup starts after it
            let mut launching = self.launching.lock();
            if self.cancel.is_cancelled() {
                return false;
            }
            launching.insert(text.clone(), launch.cwd.clone());
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = this.set_up(&config, &launch).await {
                let message = format!("cannot start {text}: {}", error.message());
                this.log.error(&message, Some(&text)).await;
                this.give_back(&launch.key, &config, launch.version).await;
                this.remember_failure(&text).await;
            }
            this.launching.lock().remove(&text);
        });
        true
    }

    async fn set_up(&self, config: &DispatcherConfig, launch: &Launch) -> Result<(), AppError> {
        let text = launch.key.to_string();
        let minutes = u64::try_from(config.worktree_setup_timeout_minutes)
            .unwrap_or(1)
            .max(1);
        let outcome = self
            .setup
            .run(
                &SetupRequest {
                    root: launch.cwd.clone(),
                    steps: config.worktree_setup.clone(),
                    timeout: Duration::from_secs(minutes * 60),
                    git_bash: config.claude.git_bash_path.clone(),
                },
                &self.cancel,
            )
            .await?;
        let Some((worktree, _)) = self.worktrees.setup_state(&text).await? else {
            return Ok(());
        };
        match outcome {
            SetupOutcome::Done => {
                self.worktrees
                    .mark_setup(worktree, SetupState::Done)
                    .await?;
                // the ticket may have been taken, moved or released while the setup ran
                if !self.still_ours(&launch.key, config).await? {
                    self.log
                        .write(
                            super::log::LogLevel::Info,
                            &format!("skipped the run of {text}: the ticket changed during setup"),
                            Some(&text),
                            None,
                        )
                        .await;
                    return Ok(());
                }
                self.begin_run(launch, config).await?;
                Ok(())
            }
            SetupOutcome::Cancelled => {
                self.give_back(&launch.key, config, launch.version).await;
                Ok(())
            }
            SetupOutcome::Failed(failure) => {
                self.worktrees
                    .mark_setup(worktree, SetupState::Failed)
                    .await?;
                // a ticket that changed during the setup is not blocked for it
                if !self.still_ours(&launch.key, config).await? {
                    self.log
                        .write(
                            super::log::LogLevel::Info,
                            &format!("skipped blocking {text}: the ticket changed during setup"),
                            Some(&text),
                            None,
                        )
                        .await;
                    return Ok(());
                }
                self.block_for_setup(launch, &failure.step(), &failure.cause(), &failure.output)
                    .await
            }
        }
    }

    /// Whether the ticket is still in progress with the agent of the dispatcher.
    async fn still_ours(
        &self,
        key: &TicketKey,
        config: &DispatcherConfig,
    ) -> Result<bool, AppError> {
        let text = key.to_string();
        let detail = self.tickets.get(&text).await?;
        let record = detail.view.ticket.record();
        Ok(record.status == TicketStatus::InProgress
            && record.assignee.as_deref() == Some(config.work_agent(&text).as_str()))
    }

    /// A failed setup blocks the ticket for a person; it is no attempt of the agent.
    async fn block_for_setup(
        &self,
        launch: &Launch,
        step: &str,
        cause: &str,
        output: &str,
    ) -> Result<(), AppError> {
        let actor = self.actor().await?;
        let text = launch.key.to_string();
        let short: String = step.chars().take(STEP_REASON_CHARS).collect();
        let reason = format!("{SETUP_REASON} {short}");
        let version = self.tickets.get(&text).await?.view.ticket.record().version;
        let patch = TicketPatch {
            status: Some(String::from("blocked")),
            blocked_reason: Some(Some(reason.clone())),
            assignee: Some(None),
            ..TicketPatch::default()
        };
        self.tickets
            .update(&text, &patch, actor, version, WriteOptions::default())
            .await?;
        let mut body = format!("[dispatch] setup: {step} failed ({cause}).");
        let output = tail(output, OUTPUT_TAIL_CHARS);
        if !output.is_empty() {
            body.push('\n');
            body.push_str(&output);
        }
        self.note(&launch.key, &body, SystemMarker::Setup, None)
            .await;
        self.log
            .error(&format!("setup of {text} failed: {reason}"), Some(&text))
            .await;
        Ok(())
    }

    /// Starts the run and notes it; none, with the run stopped, when the ticket changed meanwhile.
    async fn begin_run(
        &self,
        launch: &Launch,
        config: &DispatcherConfig,
    ) -> Result<Option<Run>, AppError> {
        let text = launch.key.to_string();
        let prompt = self
            .prompts
            .work_prompt(&text, &launch.branch, Vec::new())
            .await?;
        let run = self
            .dispatch
            .start(StartDispatchRun {
                kind: RunKind::Work,
                trigger: RunTrigger::Dispatcher,
                subject: text.clone(),
                prompt,
                cwd: PathBuf::from(&launch.cwd),
                profile: Some(launch.choice.profile.clone()),
                account: None,
                extra_tools: Vec::new(),
            })
            .await?;
        if !self.still_ours(&launch.key, config).await? {
            if let Err(error) = self.runs.cancel(run.id()).await {
                self.log
                    .error(
                        &format!("cannot stop the run of {text}: {}", error.message()),
                        Some(&text),
                    )
                    .await;
            }
            return Ok(None);
        }
        let profile = run.dispatch().map(|dispatch| &dispatch.profile);
        let mut body = format!(
            "[dispatch] start: handed to {}. Branch {}, profile {} (model {}, effort {}, max {} turns, {} min). Reason: {}.",
            launch.agent,
            launch.branch,
            launch.choice.profile,
            profile
                .and_then(|p| p.model.as_deref())
                .unwrap_or("default"),
            profile
                .and_then(|p| p.effort.as_deref())
                .unwrap_or("default"),
            profile.map_or(0, |p| p.max_turns),
            profile.map_or(0, |p| p.timeout_minutes),
            launch.choice.why,
        );
        if !launch.notes.is_empty() {
            body.push_str("\nNote: ");
            body.push_str(&launch.notes.join(" "));
        }
        self.note(&launch.key, &body, SystemMarker::Start, Some(run.id()))
            .await;
        self.log
            .write(
                super::log::LogLevel::Info,
                &format!("started {text} as {} in {}", launch.agent, launch.cwd),
                Some(&text),
                Some(run.id()),
            )
            .await;
        Ok(Some(run))
    }

    /// A comment on the ticket; failing to write it never undoes a run that started.
    async fn note(&self, key: &TicketKey, body: &str, marker: SystemMarker, run: Option<Id<Run>>) {
        let Ok(actor) = self.actor().await else {
            return;
        };
        if let Err(error) = self
            .comments
            .add_system(key, body, marker, run, actor)
            .await
        {
            self.log
                .error(
                    &format!("comment on {key} not written: {}", error.message()),
                    Some(&key.to_string()),
                )
                .await;
        }
    }

    /// Clears the claim only while the ticket is as this start left it; the link stays.
    async fn give_back(&self, key: &TicketKey, config: &DispatcherConfig, version: Version) {
        let Ok(actor) = self.actor().await else {
            return;
        };
        let text = key.to_string();
        let ours = self.tickets.get(&text).await.is_ok_and(|detail| {
            let record = detail.view.ticket.record();
            record.version == version
                && record.assignee.as_deref() == Some(config.work_agent(&text).as_str())
        });
        if !ours {
            return;
        }
        let released = self
            .tickets
            .release(&text, Some("planned"), actor, Some(version))
            .await;
        if let Err(error) = released
            && error.kind() != ErrorKind::Conflict
        {
            self.log
                .error(
                    &format!("cannot give {text} back: {}", error.message()),
                    Some(&text),
                )
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_compare_by_their_forward_slash_spelling() {
        assert!(same_folder("C:\\w\\ARC-0001\\", "C:/w/ARC-0001"));
        assert!(!same_folder("/w/ARC-0001", "/w/ARC-0002"));
    }

    #[test]
    fn only_the_end_of_an_output_is_kept() {
        assert_eq!(tail("  short  ", 20), "short");
        assert_eq!(tail("abcdef", 3), "def");
    }
}
