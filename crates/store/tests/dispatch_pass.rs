#![allow(
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::cloned_ref_to_slice_refs,
    reason = "integration tests fail loudly"
)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nullradix_app::accounts::{AccountFolders, AccountService};
use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::comments::{Author, CommentService};
use nullradix_app::dispatch::accounts::{
    AccountLimits, AccountPause, AccountPauseRepository, AccountPick, AccountRequest, PauseReason,
    PickMode, UsageSampleRepository,
};
use nullradix_app::dispatch::assemble::{Dispatcher, DispatcherDeps, assemble};
use nullradix_app::dispatch::detached::{
    DetachedCreate, DetachedCreated, DetachedPurpose, DetachedRemove, DetachedRemoved,
    DetachedWorktreePort, detached_path, detached_subject,
};
use nullradix_app::dispatch::local_time::LocalTime;
use nullradix_app::dispatch::merge_git::{
    BranchDeletion, DeleteBranch, MergeGit, MergeOutcome, MergeRequest, MergedEarlier,
};
use nullradix_app::dispatch::pause::{NoPauseHook, PauseHook, PauseKind};
use nullradix_app::dispatch::runs::{DispatchRuns, RunFilesPort};
use nullradix_app::dispatch::setup::{SetupFailure, SetupOutcome, SetupRequest, WorktreeSetupPort};
use nullradix_app::dispatcher_config::DispatcherConfigService;
use nullradix_app::dispatcher_prompts::DispatcherPromptService;
use nullradix_app::notifier::{Notice, NoticeKind, Notifier};
use nullradix_app::projects::ProjectService;
use nullradix_app::questions::{NewQuestion, QuestionService};
use nullradix_app::repositories::{Located, RunnerService, not_a_git_project};
use nullradix_app::review::{
    BranchChanges, BranchFile, ChangedFile, FilePatch, ReviewParts, ReviewRequest, ReviewService,
    review_file_unknown,
};
use nullradix_app::runs::{
    OutputLine, OutputStream, RunFilter, RunProgress, RunService, RunSpec, RunnerPort,
};
use nullradix_app::secrets::MemorySecretStore;
use nullradix_app::session_detail::{
    ChangeRequest, DiffRequest, FileDiff, SessionChanges, TranscriptChunk, TranscriptRead,
    TranscriptSource, TranscriptSummary, change_not_found,
};
use nullradix_app::tickets::{
    NewTicket, TicketPatch, TicketRead, TicketRepository, TicketScope, TicketService,
    TicketSnapshot, TicketWrite, WriteOptions,
};
use nullradix_app::usage::{AccountStatus, AccountUsage, UsageReason, UsageWindow};
use nullradix_app::worktrees::{
    CreatedWorktree, EnsureWorktree, EnsuredWorktree, NewWorktree, RemoveWorktree, StartMode,
    WorktreeRemoval, WorktreeService,
};
use nullradix_app::{AppError, Change, ChangeSignal, Clock, UuidV7Generator};
use nullradix_domain::{
    Abort, AbortCategory, Account, Actor, ActorKind, Board, ConfigDir, DispatchOutcome, Id, Run,
    RunEnd, RunOutcome, RunResult, Runner, SetupState, SystemMarker, TicketStatus, Timestamp,
    Version, WorktreeSetupStep,
};
use nullradix_store::Store;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const START: i64 = 1_800_000_000_000;
const MINUTE: i64 = 60_000;

#[cfg(windows)]
const DRIVE: &str = "C:";
#[cfg(not(windows))]
const DRIVE: &str = "";
const ROOT: &str = if cfg!(windows) {
    "C:/work/arches"
} else {
    "/work/arches"
};

/// The moment, and the offset of the local time from UTC in minutes.
struct FakeClock(AtomicI64, std::sync::atomic::AtomicI32);

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.0.load(Ordering::SeqCst))
    }
}

#[async_trait]
impl LocalTime for FakeClock {
    async fn offset_minutes(&self, _at: Timestamp) -> i32 {
        self.1.load(Ordering::SeqCst)
    }
}

/// Git and Claude double: created worktrees exist, runs stay alive.
#[derive(Default)]
struct Machine {
    folders: Mutex<HashSet<String>>,
    specs: Mutex<Vec<RunSpec>>,
    /// The next worktree creation waits for `create_release` when set.
    hold_create: Mutex<Option<Arc<Notify>>>,
    create_entered: Notify,
    fail_create: std::sync::atomic::AtomicBool,
    fail_start: std::sync::atomic::AtomicBool,
    /// The next run start waits for this gate when set.
    hold_start: Mutex<Option<Arc<Notify>>>,
    start_entered: Notify,
    cancelled: Mutex<Vec<Id<Run>>>,
    /// Output lines each run printed, oldest first.
    outputs: Mutex<HashMap<Id<Run>, Vec<OutputLine>>>,
    /// What git says about the branch; fails when `None`.
    changes: Mutex<Option<BranchChanges>>,
    patches: Mutex<HashMap<String, FilePatch>>,
    files: Mutex<HashMap<String, BranchFile>>,
    /// Roots and branches the review asked about.
    asked: Mutex<Vec<ReviewRequest>>,
}

#[async_trait]
impl RunnerPort for Machine {
    async fn check(&self, _spec: &RunSpec) -> Result<(), AppError> {
        Ok(())
    }

    async fn start(&self, _run: Id<Run>, spec: &RunSpec) -> Result<u32, AppError> {
        self.specs.lock().unwrap().push(spec.clone());
        if self.fail_start.load(Ordering::SeqCst) {
            return Err(AppError::unavailable(
                "claude-missing",
                "claude is not installed",
            ));
        }
        let gate = self.hold_start.lock().unwrap().take();
        if let Some(gate) = gate {
            self.start_entered.notify_one();
            gate.notified().await;
        }
        Ok(4242)
    }

    async fn cancel(&self, run: Id<Run>) -> Result<(), AppError> {
        self.cancelled.lock().unwrap().push(run);
        Ok(())
    }

    async fn progress(
        &self,
        run: Id<Run>,
        after: i64,
        limit: u32,
    ) -> Result<RunProgress, AppError> {
        let outputs = self.outputs.lock().unwrap();
        let lines = outputs
            .get(&run)
            .into_iter()
            .flatten()
            .filter(|line| line.seq > after)
            .take(limit as usize)
            .cloned()
            .collect();
        Ok(RunProgress {
            lines,
            report: None,
            alive: true,
        })
    }

    async fn locate(&self, path: &str) -> Result<Located, AppError> {
        let mut parts = path
            .trim_start_matches(DRIVE)
            .trim_start_matches('/')
            .split('/');
        match (parts.next(), parts.next()) {
            (Some("work"), Some(name)) if !name.is_empty() => Ok(Located {
                root: format!("{DRIVE}/work/{name}"),
                key: format!("{DRIVE}/work/{name}"),
                name: name.to_owned(),
            }),
            _ => Err(not_a_git_project(path)),
        }
    }

    async fn create_worktree(&self, request: &NewWorktree) -> Result<CreatedWorktree, AppError> {
        let gate = self.hold_create.lock().unwrap().take();
        if let Some(gate) = gate {
            self.create_entered.notify_one();
            gate.notified().await;
        }
        if self.fail_create.load(Ordering::SeqCst) {
            return Err(AppError::invalid("git-failed", "git is broken"));
        }
        self.folders.lock().unwrap().insert(request.path.clone());
        Ok(CreatedWorktree::default())
    }

    async fn folders_exist(&self, paths: &[String]) -> Result<Vec<bool>, AppError> {
        let folders = self.folders.lock().unwrap();
        Ok(paths
            .iter()
            .map(|path| path == ROOT || folders.contains(path))
            .collect())
    }

    async fn ensure_worktree(&self, request: &EnsureWorktree) -> Result<EnsuredWorktree, AppError> {
        if self.folders.lock().unwrap().insert(request.path.clone()) {
            Ok(EnsuredWorktree::Restored { moved_aside: None })
        } else {
            Ok(EnsuredWorktree::Present)
        }
    }

    async fn remove_worktree(
        &self,
        _request: &RemoveWorktree,
    ) -> Result<WorktreeRemoval, AppError> {
        Ok(WorktreeRemoval::Removed)
    }

    async fn read_transcript(
        &self,
        _request: &TranscriptRead,
    ) -> Result<Option<TranscriptChunk>, AppError> {
        Ok(None)
    }

    async fn transcript_facts(
        &self,
        _source: &TranscriptSource,
    ) -> Result<Option<TranscriptSummary>, AppError> {
        Ok(None)
    }

    async fn session_changes(&self, _request: &ChangeRequest) -> Result<SessionChanges, AppError> {
        Ok(SessionChanges::none())
    }

    async fn session_diff(&self, request: &DiffRequest) -> Result<FileDiff, AppError> {
        Err(change_not_found(&request.path))
    }

    async fn review_changes(&self, request: &ReviewRequest) -> Result<BranchChanges, AppError> {
        self.asked.lock().unwrap().push(request.clone());
        self.changes
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| AppError::invalid("git-failed", "git is broken"))
    }

    async fn review_patch(
        &self,
        _request: &ReviewRequest,
        path: &str,
    ) -> Result<FilePatch, AppError> {
        self.patches
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| review_file_unknown(path, "branch"))
    }

    async fn review_file(
        &self,
        _root: &str,
        _branch: &str,
        path: &str,
    ) -> Result<BranchFile, AppError> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| review_file_unknown(path, "branch"))
    }
}

#[async_trait]
impl RunFilesPort for Machine {
    async fn read_screenshot(
        &self,
        _run: Id<Run>,
        _file: &str,
    ) -> Result<Option<Vec<u8>>, AppError> {
        Ok(None)
    }
}

/// Detached worktrees the test sees: created folders exist until removed.
#[derive(Default)]
struct Detached {
    folders: Mutex<HashSet<String>>,
    created: Mutex<Vec<DetachedCreate>>,
    removed: Mutex<Vec<String>>,
    busy: std::sync::atomic::AtomicBool,
    keep: std::sync::atomic::AtomicBool,
    fail_create: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl DetachedWorktreePort for Detached {
    async fn create(&self, request: &DetachedCreate) -> Result<DetachedCreated, AppError> {
        if self.fail_create.load(Ordering::SeqCst) {
            return Err(AppError::invalid("git-failed", "git is broken"));
        }
        let path = detached_path(&request.root, request.purpose, &request.subject);
        if self.busy.load(Ordering::SeqCst) {
            return Ok(DetachedCreated::Busy { path });
        }
        self.created.lock().unwrap().push(request.clone());
        self.folders.lock().unwrap().insert(path.clone());
        Ok(DetachedCreated::Created {
            path,
            long_paths_enabled: false,
        })
    }

    async fn remove(&self, request: &DetachedRemove) -> Result<DetachedRemoved, AppError> {
        if self.keep.load(Ordering::SeqCst) {
            return Ok(DetachedRemoved::Kept(String::from("a file is locked")));
        }
        self.folders.lock().unwrap().remove(&request.path);
        self.removed.lock().unwrap().push(request.path.clone());
        Ok(DetachedRemoved::Removed)
    }

    async fn list(&self, _root: &str, purpose: DetachedPurpose) -> Result<Vec<String>, AppError> {
        let mut folders: Vec<String> = self
            .folders
            .lock()
            .unwrap()
            .iter()
            .filter(|path| detached_subject(purpose, path).is_some())
            .cloned()
            .collect();
        folders.sort();
        Ok(folders)
    }
}

struct NoFolders;

#[async_trait]
impl AccountFolders for NoFolders {
    fn root(&self) -> String {
        String::from("/accounts")
    }

    async fn create(&self, slug: &nullradix_domain::AccountSlug) -> Result<ConfigDir, AppError> {
        Ok(ConfigDir::from_stored(format!(
            "/accounts/{}",
            slug.as_str()
        )))
    }

    async fn check(&self, _dir: &ConfigDir) -> Result<(), AppError> {
        Ok(())
    }

    fn key(&self, dir: &ConfigDir) -> String {
        dir.as_str().to_owned()
    }

    fn slug_key(&self, slug: &nullradix_domain::AccountSlug) -> String {
        format!("/accounts/{}", slug.as_str())
    }

    fn owns(&self, _dir: &ConfigDir) -> bool {
        true
    }

    async fn remove(&self, _dir: &ConfigDir) -> Result<(), AppError> {
        Ok(())
    }
}

/// Usage readings the test sets; a reading can wait for a gate.
#[derive(Default)]
struct FakeLimits {
    usage: Mutex<HashMap<Id<Account>, AccountUsage>>,
    /// The next reading waits for this gate when set.
    hold: Mutex<Option<Arc<Notify>>>,
    entered: Notify,
    /// The `fresh` flag of every reading asked for.
    fresh: Mutex<Vec<bool>>,
    /// Accounts whose folder has no login.
    logged_out: Mutex<HashSet<Id<Account>>>,
}

impl FakeLimits {
    fn set(&self, account: Id<Account>, five: u32, week: u32, resets: Option<Timestamp>) {
        let window = |percent| {
            Some(UsageWindow {
                percent,
                resets_at: resets,
            })
        };
        self.usage.lock().unwrap().insert(
            account,
            AccountUsage {
                reason: None,
                plan: None,
                five_hour: window(five),
                seven_day: window(week),
                fetched_at: None,
            },
        );
    }
}

#[async_trait]
impl AccountLimits for FakeLimits {
    async fn logged_in(&self, accounts: &[Account]) -> HashMap<Id<Account>, bool> {
        let out = self.logged_out.lock().unwrap();
        accounts
            .iter()
            .map(|account| (account.id(), !out.contains(&account.id())))
            .collect()
    }

    async fn status(&self, accounts: &[Account], fresh: bool) -> Vec<AccountStatus> {
        self.fresh.lock().unwrap().push(fresh);
        let gate = self.hold.lock().unwrap().take();
        if let Some(gate) = gate {
            self.entered.notify_one();
            gate.notified().await;
        }
        let usage = self.usage.lock().unwrap();
        accounts
            .iter()
            .map(|account| AccountStatus {
                account: account.id(),
                logged_in: true,
                email: None,
                folder: String::new(),
                usage: usage
                    .get(&account.id())
                    .cloned()
                    .unwrap_or_else(|| AccountUsage::unavailable(UsageReason::RequestFailed, None)),
            })
            .collect()
    }
}

/// Pause storage that adds a pause right after the next read, as another writer would.
struct InjectingPauses {
    inner: Arc<Store>,
    inject: Mutex<Option<AccountPause>>,
}

#[async_trait]
impl AccountPauseRepository for InjectingPauses {
    async fn all(&self) -> Result<Vec<AccountPause>, AppError> {
        let rows = self.inner.all().await?;
        let pause = self.inject.lock().unwrap().take();
        if let Some(pause) = pause {
            self.inner.put(&pause).await?;
        }
        Ok(rows)
    }

    async fn put(&self, pause: &AccountPause) -> Result<Change, AppError> {
        self.inner.put(pause).await
    }

    async fn remove(
        &self,
        account: Id<Account>,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError> {
        self.inner.remove(account, at).await
    }

    async fn clear(&self, at: Timestamp) -> Result<Vec<Change>, AppError> {
        self.inner.clear(at).await
    }
}

/// Git double for the merge: answers come from the test, requests are kept.
#[derive(Default)]
struct ScriptedGit {
    /// The branch is gone.
    gone: std::sync::atomic::AtomicBool,
    merges: Mutex<std::collections::VecDeque<MergeOutcome>>,
    earlier: Mutex<Option<MergedEarlier>>,
    integrated: std::sync::atomic::AtomicBool,
    created: Mutex<Option<String>>,
    deletion: Mutex<Option<BranchDeletion>>,
    requests: Mutex<Vec<MergeRequest>>,
    deleted: Mutex<Vec<DeleteBranch>>,
    /// The next merge waits for this gate when set.
    hold_merge: Mutex<Option<Arc<Notify>>>,
    merge_entered: Notify,
    /// The next merge fails with this error.
    fail_merge: Mutex<Option<AppError>>,
    /// Every merge of this branch fails.
    fail_branch: Mutex<Option<String>>,
    /// Calls of `branch_tip` and `integrated`.
    probes: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl MergeGit for ScriptedGit {
    async fn ensure_target(&self, _root: &str, _target: &str) -> Result<Option<String>, AppError> {
        Ok(self.created.lock().unwrap().take())
    }

    async fn branch_tip(&self, _root: &str, _branch: &str) -> Result<Option<String>, AppError> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        Ok((!self.gone.load(Ordering::SeqCst)).then(|| String::from("tip0001")))
    }

    async fn integrated(
        &self,
        _root: &str,
        _branch: &str,
        _target: Option<&str>,
    ) -> Result<bool, AppError> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        Ok(self.integrated.load(Ordering::SeqCst))
    }

    async fn merged_earlier(
        &self,
        _root: &str,
        _branch: &str,
        _target: Option<&str>,
    ) -> Result<Option<MergedEarlier>, AppError> {
        Ok(self.earlier.lock().unwrap().clone())
    }

    async fn merge(&self, request: &MergeRequest) -> Result<MergeOutcome, AppError> {
        self.requests.lock().unwrap().push(request.clone());
        if self.fail_branch.lock().unwrap().as_deref() == Some(request.branch.as_str()) {
            return Err(AppError::conflict("git-failed", "branch locked"));
        }
        if let Some(error) = self.fail_merge.lock().unwrap().take() {
            return Err(error);
        }
        let gate = self.hold_merge.lock().unwrap().take();
        if let Some(gate) = gate {
            self.merge_entered.notify_one();
            gate.notified().await;
        }
        let scripted = self.merges.lock().unwrap().pop_front();
        Ok(scripted.unwrap_or_else(|| MergeOutcome::Merged {
            into: request
                .target
                .clone()
                .unwrap_or_else(|| String::from("main")),
            commit: String::from("abc1234"),
            tip: String::from("tip0001"),
            warning: None,
        }))
    }

    async fn delete_merged(&self, request: &DeleteBranch) -> Result<BranchDeletion, AppError> {
        self.deleted.lock().unwrap().push(request.clone());
        Ok(self
            .deletion
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(BranchDeletion::Deleted))
    }
}

/// Setup double that holds every setup until the test lets it go.
#[derive(Default)]
struct ScriptedSetup {
    runs: Mutex<Vec<SetupRequest>>,
    failure: Mutex<Option<SetupFailure>>,
    panics: Mutex<bool>,
    entered: Notify,
    release: Mutex<Option<Arc<Notify>>>,
}

#[async_trait]
impl WorktreeSetupPort for ScriptedSetup {
    async fn run(
        &self,
        request: &SetupRequest,
        cancel: &CancellationToken,
    ) -> Result<SetupOutcome, AppError> {
        self.runs.lock().unwrap().push(request.clone());
        assert!(!*self.panics.lock().unwrap(), "the setup crashed");
        self.entered.notify_one();
        let gate = self.release.lock().unwrap().clone();
        if let Some(gate) = gate {
            tokio::select! {
                () = gate.notified() => {}
                () = cancel.cancelled() => return Ok(SetupOutcome::Cancelled),
            }
        }
        Ok(match self.failure.lock().unwrap().clone() {
            Some(failure) => SetupOutcome::Failed(failure),
            None => SetupOutcome::Done,
        })
    }
}

/// Ticket reads that change one ticket right after the next list, as a person would.
struct Hooked {
    inner: Arc<Store>,
    bump: Mutex<Option<String>>,
    /// A person renames this ticket right after its nth read from now, which stays stale.
    rename_on_get: Mutex<Option<(String, usize)>>,
    /// Reads of a ticket blocked by a failed setup, comment included, fail while set.
    fail_setup_block_reads: std::sync::atomic::AtomicBool,
    plain: TicketService,
    actor: Actor,
}

#[async_trait]
impl TicketRepository for Hooked {
    async fn begin(&self) -> Result<Box<dyn TicketWrite>, AppError> {
        self.inner.begin().await
    }

    async fn list(&self, scope: TicketScope) -> Result<TicketSnapshot, AppError> {
        let snapshot = self.inner.list(scope).await?;
        let key = self.bump.lock().unwrap().take();
        if let Some(key) = key {
            let version = self.plain.get(&key).await?.view.ticket.record().version;
            let patch = TicketPatch {
                title: Some(String::from("Renamed")),
                ..TicketPatch::default()
            };
            self.plain
                .update(&key, &patch, &self.actor, version, FORCE)
                .await?;
        }
        Ok(snapshot)
    }

    async fn get(&self, key: &nullradix_domain::TicketKey) -> Result<Option<TicketRead>, AppError> {
        let read = self.inner.get(key).await?;
        let blocked = read.as_ref().is_some_and(|read| {
            read.row.ticket.record().status == nullradix_domain::TicketStatus::Blocked
                && read.comments.iter().any(|line| {
                    line.comment.record().system_marker
                        == Some(nullradix_domain::SystemMarker::Setup)
                })
        });
        if blocked && self.fail_setup_block_reads.load(Ordering::SeqCst) {
            return Err(AppError::unavailable("read-failed", "the read failed"));
        }
        let rename = {
            let mut armed = self.rename_on_get.lock().unwrap();
            match armed.as_mut() {
                Some((armed_key, reads)) if *armed_key == key.to_string() => {
                    *reads -= 1;
                    (*reads == 0).then(|| armed_key.clone())
                }
                _ => None,
            }
        };
        if let Some(key) = rename {
            *self.rename_on_get.lock().unwrap() = None;
            let version = self.plain.get(&key).await?.view.ticket.record().version;
            let patch = TicketPatch {
                title: Some(String::from("Renamed")),
                ..TicketPatch::default()
            };
            self.plain
                .update(&key, &patch, &self.actor, version, FORCE)
                .await?;
        }
        Ok(read)
    }
}

/// Notices the dispatcher showed.
#[derive(Default)]
struct Notices(Mutex<Vec<Notice>>);

#[async_trait]
impl Notifier for Notices {
    async fn notify(&self, notice: &Notice) -> Result<(), AppError> {
        self.0.lock().unwrap().push(notice.clone());
        Ok(())
    }
}

struct Fixture {
    notices: Arc<Notices>,
    hook: Arc<Hooked>,
    clock: Arc<FakeClock>,
    machine: Arc<Machine>,
    git: Arc<ScriptedGit>,
    setup: Arc<ScriptedSetup>,
    detached: Arc<Detached>,
    dispatcher: Dispatcher,
    accounts: Arc<AccountService>,
    limits: Arc<FakeLimits>,
    pauses: Arc<InjectingPauses>,
    config: Arc<DispatcherConfigService>,
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
    questions: Arc<QuestionService>,
    worktrees: Arc<WorktreeService>,
    reviews: Arc<ReviewService>,
    runs: Arc<RunService>,
    prompts: Arc<DispatcherPromptService>,
    local: Id<Runner>,
    person: Actor,
    board: Board,
    store: Arc<Store>,
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let clock = Arc::new(FakeClock(
        AtomicI64::new(START),
        std::sync::atomic::AtomicI32::new(0),
    ));
    let ids = Arc::new(UuidV7Generator);
    let machine = Arc::new(Machine::default());
    let setup = Arc::new(ScriptedSetup::default());
    let detached = Arc::new(Detached::default());
    let git = Arc::new(ScriptedGit::default());
    let runners = Arc::new(
        RunnerService::start(
            store.clone(),
            store.clone(),
            machine.clone(),
            clock.clone(),
            ids.clone(),
            signal.clone(),
        )
        .await
        .unwrap(),
    );
    let local = runners.local().id();
    let person = local_person(store.as_ref(), "anna", ids.as_ref(), clock.as_ref())
        .await
        .unwrap();
    let projects = ProjectService::new(store.clone(), clock.clone(), ids.clone(), signal.clone());
    let boards = BoardService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        runners.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    );
    let project = projects.create("ARC", "Arches").await.unwrap();
    let created = boards.create(project.id(), "Main", &person).await.unwrap();
    let board = boards
        .set_repository(
            project.id(),
            created.board.id(),
            local,
            ROOT,
            Version::INITIAL,
        )
        .await
        .unwrap()
        .board;
    let hook = Arc::new(Hooked {
        inner: store.clone(),
        bump: Mutex::new(None),
        rename_on_get: Mutex::new(None),
        fail_setup_block_reads: std::sync::atomic::AtomicBool::new(false),
        plain: TicketService::new(
            store.clone(),
            store.clone(),
            clock.clone(),
            ids.clone(),
            signal.clone(),
        ),
        actor: person.clone(),
    });
    let tickets = Arc::new(TicketService::new(
        hook.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    ));
    let comments = Arc::new(CommentService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    ));
    let questions = Arc::new(QuestionService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    ));
    let worktrees = Arc::new(WorktreeService::new(
        tickets.clone(),
        store.clone(),
        store.clone(),
        runners.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    ));
    let reviews = Arc::new(ReviewService::new(ReviewParts {
        tickets: tickets.clone(),
        worktrees: store.clone(),
        runners: runners.clone(),
        reviews: store.clone(),
        clock: clock.clone(),
        ids: ids.clone(),
        signal: signal.clone(),
    }));
    let config = Arc::new(DispatcherConfigService::new(
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    ));
    let accounts = Arc::new(AccountService::new(
        store.clone(),
        Arc::new(MemorySecretStore::new()),
        Arc::new(NoFolders),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    ));
    let runs = RunService::new(
        store.clone(),
        machine.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
        std::time::Duration::from_millis(20),
    );
    let dispatch = Arc::new(DispatchRuns::new(
        runs.clone(),
        config.clone(),
        accounts.clone(),
        store.clone(),
        store.clone(),
        machine.clone(),
    ));
    let prompts = Arc::new(DispatcherPromptService::new(
        config.clone(),
        tickets.clone(),
        questions.clone(),
    ));
    let limits = Arc::new(FakeLimits::default());
    let notices = Arc::new(Notices::default());
    let pauses = Arc::new(InjectingPauses {
        inner: store.clone(),
        inject: Mutex::new(None),
    });
    let dispatcher = assemble(DispatcherDeps {
        config: config.clone(),
        states: store.clone(),
        log: store.clone(),
        accounts: accounts.clone(),
        limits: limits.clone(),
        pauses: pauses.clone(),
        samples: store.clone(),
        setup: setup.clone(),
        detached: detached.clone(),
        tracking: store.clone(),
        questions: questions.clone(),
        actors: store.clone(),
        boards: store.clone(),
        runners,
        runs: runs.clone(),
        dispatch,
        prompts: prompts.clone(),
        tickets: tickets.clone(),
        comments: comments.clone(),
        worktrees: worktrees.clone(),
        worktree_repository: store.clone(),
        merge_jobs: store.clone(),
        merge_git: git.clone(),
        reviews: reviews.clone(),
        clock: clock.clone(),
        ids,
        signal,
        local_time: clock.clone(),
        notifier: notices.clone(),
        interval: None,
    });
    Fixture {
        notices,
        hook,
        clock,
        machine,
        git,
        setup,
        detached,
        dispatcher,
        accounts,
        limits,
        pauses,
        config,
        tickets,
        comments,
        questions,
        worktrees,
        reviews,
        runs,
        prompts,
        local,
        person,
        board,
        store,
    }
}

const FORCE: WriteOptions = WriteOptions {
    by_person: false,
    force: true,
    allow_duplicate: true,
};

impl Fixture {
    /// A planned ticket with plan, handbook and acceptance.
    async fn ready(&self, title: &str, labels: &[&str]) -> String {
        let input = NewTicket {
            title: title.to_owned(),
            acceptance: Some(String::from("It works")),
            labels: labels.iter().map(|label| (*label).to_owned()).collect(),
            ..NewTicket::default()
        };
        let key = self
            .tickets
            .create(
                self.board.project(),
                self.board.id(),
                &input,
                &self.person,
                FORCE,
            )
            .await
            .unwrap()
            .ticket
            .key()
            .to_string();
        let version = self
            .tickets
            .get(&key)
            .await
            .unwrap()
            .view
            .ticket
            .record()
            .version;
        let patch = TicketPatch {
            plan: Some(String::from("Do it.")),
            handbook: Some(String::from("cargo test")),
            status: Some(String::from("planned")),
            needs_planning: Some(false),
            ..TicketPatch::default()
        };
        self.tickets
            .update(&key, &patch, &self.person, version, FORCE)
            .await
            .unwrap();
        key
    }

    async fn configure(&self, change: impl FnOnce(&mut nullradix_domain::DispatcherConfig)) {
        let view = self.config.get().await.unwrap();
        let mut config = view.config;
        change(&mut config);
        self.config.update(config, view.version).await.unwrap();
    }

    async fn enable(&self) {
        self.configure(|config| {
            config.enabled = true;
            config.review_before_merge = false;
        })
        .await;
    }

    fn advance(&self, minutes: i64) {
        self.clock.0.fetch_add(minutes * MINUTE, Ordering::SeqCst);
    }

    async fn record(&self, key: &str) -> nullradix_domain::TicketRecord {
        self.tickets
            .get(key)
            .await
            .unwrap()
            .view
            .ticket
            .record()
            .clone()
    }

    fn started_cwds(&self) -> Vec<String> {
        self.machine
            .specs
            .lock()
            .unwrap()
            .iter()
            .map(|spec| spec.cwd.to_string_lossy().replace('\\', "/"))
            .collect()
    }
}

#[tokio::test]
async fn a_ready_ticket_is_claimed_and_claude_starts_in_its_worktree() {
    let f = fixture().await;
    f.enable().await;
    let key = f.ready("Build the login", &["auto"]).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [key.clone()], "{report:?}");
    let record = f.record(&key).await;
    assert_eq!(record.assignee.as_deref(), Some("worker-ARC-0001"));
    assert_eq!(record.status, nullradix_domain::TicketStatus::InProgress);
    let worktree = record.worktree.clone().unwrap();
    assert_eq!(f.started_cwds(), [worktree]);
    let spec = f.machine.specs.lock().unwrap()[0].clone();
    assert!(spec.stdin.unwrap().contains("ARC-0001"));
    let comments = f.comments.list(&key).await.unwrap();
    let marker = comments.last().unwrap().comment.record().system_marker;
    assert_eq!(marker, Some(nullradix_domain::SystemMarker::Start));
    assert_eq!(comments.last().unwrap().author_kind, ActorKind::Dispatcher);
    // the next pass leaves a ticket with a run alone
    let again = f.dispatcher.pass.run().await;
    assert!(again.started.is_empty());
    assert_eq!(f.started_cwds().len(), 1);
}

#[tokio::test]
async fn tickets_that_are_not_ready_stay_and_the_status_says_why() {
    let f = fixture().await;
    f.enable().await;
    f.ready("No label", &[]).await;
    f.ready("Opted out", &["auto", "manual"]).await;
    let report = f.dispatcher.pass.run().await;
    assert!(report.started.is_empty());
    let status = f.dispatcher.service.status(None).await.unwrap();
    let reasons: Vec<&str> = status
        .tickets
        .iter()
        .map(|row| row.reason.as_str())
        .collect();
    assert_eq!(reasons, ["label \"auto\" is missing", "label \"manual\""]);
    assert!(f.started_cwds().is_empty());
}

#[tokio::test]
async fn the_limit_keeps_the_next_ticket_waiting() {
    let f = fixture().await;
    f.enable().await;
    let first = f.ready("First", &["auto"]).await;
    let second = f.ready("Second", &["auto"]).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [first]);
    assert_eq!(report.queued, [second.clone()]);
    let status = f.dispatcher.service.status(None).await.unwrap();
    assert_eq!((status.active, status.wip), (1, 1));
    let row = status.tickets.iter().find(|row| row.key == second).unwrap();
    assert_eq!(row.reason, "ready - waits for a free place");
    f.configure(|config| config.wip = 2).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [second]);
}

#[tokio::test]
async fn nothing_starts_while_off_paused_or_outside_the_window() {
    let f = fixture().await;
    f.ready("Build", &["auto"]).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.idle.as_deref(), Some("The dispatcher is off"));
    f.enable().await;
    f.dispatcher
        .service
        .pause(nullradix_app::dispatch::control::PauseFor::Minutes(15))
        .await
        .unwrap();
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.idle.as_deref(), Some("Paused"));
    assert!(report.started.is_empty());
    f.dispatcher.service.resume().await.unwrap();
    // the fake clock reads 2027-01-15 08:00 UTC, so a window of the afternoon is closed
    f.configure(|config| {
        config.window = Some(nullradix_domain::TimeWindow {
            from: String::from("13:00"),
            to: String::from("14:00"),
        });
    })
    .await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(
        report.idle.as_deref(),
        Some("Outside the time window 13:00-14:00")
    );
    assert!(f.started_cwds().is_empty());
    f.configure(|config| config.window = None).await;
    assert_eq!(f.dispatcher.pass.run().await.started.len(), 1);
}

#[tokio::test]
async fn the_switch_overrides_the_configuration() {
    let f = fixture().await;
    f.enable().await;
    f.dispatcher.service.set_enabled(false).await.unwrap();
    f.ready("Build", &["auto"]).await;
    assert!(f.dispatcher.pass.run().await.started.is_empty());
    let status = f.dispatcher.service.set_enabled(true).await.unwrap();
    assert!(status.enabled);
    assert_eq!(f.dispatcher.pass.run().await.started.len(), 1);
}

#[tokio::test]
async fn a_claim_without_a_run_is_given_back_after_five_minutes() {
    let f = fixture().await;
    let key = f.ready("Build", &["auto"]).await;
    let version = f.record(&key).await.version;
    let patch = TicketPatch {
        assignee: Some(Some(String::from("worker-ARC-0001"))),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&key, &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    f.advance(4);
    assert!(f.dispatcher.pass.run().await.released.is_empty());
    f.advance(2);
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.released, [key.clone()]);
    assert_eq!(f.record(&key).await.assignee, None);
}

#[tokio::test]
async fn a_started_ticket_without_news_is_given_back_after_the_stale_limit() {
    let f = fixture().await;
    f.enable().await;
    let key = f.ready("Build", &["auto"]).await;
    f.dispatcher.pass.run().await;
    f.dispatcher.service.set_enabled(false).await.unwrap();
    // the run is over and settled, yet the claim lingers
    let runs = f.store.clone();
    let active = nullradix_app::runs::RunRepository::active(runs.as_ref())
        .await
        .unwrap();
    for run in active {
        let ended = run.lost(f.clock.now()).unwrap();
        let settled = ended.settled(DispatchOutcome::Other).unwrap();
        nullradix_app::runs::RunRepository::update(runs.as_ref(), &settled, run.version())
            .await
            .unwrap();
    }
    f.advance(149);
    assert!(f.dispatcher.pass.run().await.released.is_empty());
    f.advance(2);
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.released, [key.clone()]);
    let record = f.record(&key).await;
    assert_eq!(record.assignee, None);
    assert_eq!(record.status, nullradix_domain::TicketStatus::Planned);
    assert!(record.branch.is_some());
    // the idle claim counts as an aborted attempt
    let bodies = f.abort_bodies(&key).await;
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0]
            .starts_with("[dispatch] abort: attempt 1/2: no sign of life for more than 150 min")
    );
}

#[tokio::test]
async fn setup_runs_before_claude_and_counts_for_the_limit_meanwhile() {
    let f = fixture().await;
    f.configure(|config| {
        config.enabled = true;
        config.worktree_setup = vec![WorktreeSetupStep {
            cwd: String::from("."),
            run: String::from("npm ci"),
        }];
    })
    .await;
    let gate = Arc::new(Notify::new());
    *f.setup.release.lock().unwrap() = Some(gate.clone());
    let key = f.ready("Build", &["auto"]).await;
    f.ready("Other", &["auto"]).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [key.clone()]);
    f.setup.entered.notified().await;
    assert!(f.started_cwds().is_empty(), "claude waits for the setup");
    let status = f.dispatcher.service.status(None).await.unwrap();
    assert_eq!(status.active, 1);
    // the second ticket finds no free place while the first sets up
    let report = f.dispatcher.pass.run().await;
    assert!(report.started.is_empty());
    assert_eq!(report.queued.len(), 1);
    gate.notify_one();
    f.dispatcher.starter.wait_idle().await;
    assert_eq!(f.started_cwds().len(), 1);
    let request = f.setup.runs.lock().unwrap()[0].clone();
    assert_eq!(request.steps[0].run, "npm ci");
    assert_eq!(request.root.replace('\\', "/"), f.started_cwds()[0]);
}

#[tokio::test]
async fn a_failed_setup_blocks_the_ticket_for_a_person_and_starts_no_run() {
    let f = fixture().await;
    f.configure(|config| {
        config.enabled = true;
        config.worktree_setup = vec![WorktreeSetupStep {
            cwd: String::from("web"),
            run: String::from("npm ci"),
        }];
    })
    .await;
    *f.setup.failure.lock().unwrap() = Some(SetupFailure {
        cwd: String::from("web"),
        run: String::from("npm ci"),
        code: Some(1),
        timed_out: false,
        output: String::from("ERESOLVE"),
    });
    let key = f.ready("Build", &["auto"]).await;
    f.dispatcher.pass.run().await;
    f.dispatcher.starter.wait_idle().await;
    assert!(f.started_cwds().is_empty());
    let record = f.record(&key).await;
    assert_eq!(record.status, nullradix_domain::TicketStatus::Blocked);
    assert_eq!(
        record.blocked_reason.as_deref(),
        Some("Setup failed: web$ npm ci")
    );
    assert_eq!(record.assignee, None);
    let comments = f.comments.list(&key).await.unwrap();
    let last = comments.last().unwrap();
    assert_eq!(
        last.comment.record().system_marker,
        Some(nullradix_domain::SystemMarker::Setup)
    );
    assert!(last.comment.body().contains("ERESOLVE"));
    let notices = f.notices.0.lock().unwrap().clone();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].kind, NoticeKind::Blocked);
    assert_eq!(notices[0].text, "Setup failed: web$ npm ci");
}

#[tokio::test]
async fn a_pass_waits_for_exclusive_work_and_callers_share_one_pass() {
    let f = fixture().await;
    f.enable().await;
    f.ready("Build", &["auto"]).await;
    let gate = Arc::new(Notify::new());
    let entered = Arc::new(Notify::new());
    let pass = f.dispatcher.pass.clone();
    let holder = {
        let (pass, gate, entered) = (pass.clone(), gate.clone(), entered.clone());
        tokio::spawn(async move {
            pass.exclusive(|| async move {
                entered.notify_one();
                gate.notified().await;
            })
            .await;
        })
    };
    entered.notified().await;
    let first = tokio::spawn({
        let pass = pass.clone();
        async move { pass.run().await }
    });
    let second = tokio::spawn({
        let pass = pass.clone();
        async move { pass.run().await }
    });
    tokio::task::yield_now().await;
    assert!(f.started_cwds().is_empty(), "no pass beside exclusive work");
    gate.notify_one();
    holder.await.unwrap();
    let (first, second) = (first.await.unwrap(), second.await.unwrap());
    assert_eq!(first.at, second.at);
    assert_eq!(f.started_cwds().len(), 1, "one pass served both callers");
}

#[tokio::test]
async fn the_state_survives_in_the_database_and_the_log_is_written() {
    let f = fixture().await;
    f.enable().await;
    let key = f.ready("Build", &["auto"]).await;
    f.dispatcher.pass.run().await;
    let state = nullradix_app::dispatch::state::DispatcherStateRepository::load(f.store.as_ref())
        .await
        .unwrap();
    let kept = nullradix_app::dispatch::pass::PassReport::from_json(
        state.record().last_pass.as_deref().unwrap(),
    )
    .unwrap();
    assert_eq!(kept.started, [key.clone()]);
    let lines = nullradix_app::dispatch::log::DispatcherLogRepository::recent(f.store.as_ref(), 10)
        .await
        .unwrap();
    assert!(
        lines
            .iter()
            .any(|line| line.subject.as_deref() == Some(key.as_str()))
    );
}

#[tokio::test]
async fn a_state_write_on_a_stale_version_changes_nothing() {
    use nullradix_app::dispatch::state::DispatcherStateRepository;
    let store = Store::in_memory().await.unwrap();
    let state = store.load().await.unwrap();
    let next = state.with_enabled(Some(true));
    let at = Timestamp::from_millis(START);
    assert!(
        store
            .save(&next, Version::from_stored(7), at)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.load().await.unwrap(), state);
    assert!(
        store
            .save(&next, state.record().version, at)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store.load().await.unwrap().record().enabled_override,
        Some(true)
    );
}

fn setup_config(config: &mut nullradix_domain::DispatcherConfig) {
    config.enabled = true;
    config.worktree_setup = vec![WorktreeSetupStep {
        cwd: String::from("."),
        run: String::from("npm ci"),
    }];
}

#[tokio::test]
async fn a_ticket_that_changed_during_setup_does_not_get_a_run() {
    let f = fixture().await;
    f.configure(setup_config).await;
    let gate = Arc::new(Notify::new());
    *f.setup.release.lock().unwrap() = Some(gate.clone());
    let key = f.ready("Build", &["auto"]).await;
    assert_eq!(f.dispatcher.pass.run().await.started, [key.clone()]);
    f.setup.entered.notified().await;
    // a person takes the ticket back while the setup runs
    f.tickets
        .release(&key, Some("planned"), &f.person, None)
        .await
        .unwrap();
    gate.notify_one();
    f.dispatcher.starter.wait_idle().await;
    assert!(f.started_cwds().is_empty(), "no run for a released ticket");
    let record = f.record(&key).await;
    assert_eq!(record.assignee, None);
    assert_eq!(record.status, nullradix_domain::TicketStatus::Planned);
    let (_, state) = f.worktrees.setup_state(&key).await.unwrap().unwrap();
    assert_eq!(state, SetupState::Done);
    let lines = nullradix_app::dispatch::log::DispatcherLogRepository::recent(f.store.as_ref(), 20)
        .await
        .unwrap();
    assert!(
        lines
            .iter()
            .any(|line| line.message.contains("changed during setup"))
    );
}

#[tokio::test]
async fn a_failed_start_never_undoes_a_manual_start_that_came_in_between() {
    let f = fixture().await;
    f.enable().await;
    let key = f.ready("Build", &["auto"]).await;
    let gate = Arc::new(Notify::new());
    *f.machine.hold_create.lock().unwrap() = Some(gate.clone());
    let pass = tokio::spawn({
        let pass = f.dispatcher.pass.clone();
        async move { pass.run().await }
    });
    f.machine.create_entered.notified().await;
    // a person starts the claimed ticket by hand while git works for the dispatcher
    f.worktrees
        .start(&key, f.local, ROOT, StartMode::New, false, &f.person)
        .await
        .unwrap();
    let before = f.record(&key).await;
    assert_eq!(before.status, nullradix_domain::TicketStatus::InProgress);
    gate.notify_one();
    let report = pass.await.unwrap();
    assert!(report.started.is_empty(), "{report:?}");
    assert_eq!(f.record(&key).await, before, "the manual start stays");
    assert!(f.started_cwds().is_empty());
}

#[tokio::test]
async fn a_second_ticket_waits_while_a_setup_runs_in_the_folder_it_would_join() {
    let f = fixture().await;
    f.configure(|config| {
        setup_config(config);
        config.wip = 2;
        config.join_worktrees = nullradix_domain::JoinWorktrees::SameSpec;
    })
    .await;
    let gate = Arc::new(Notify::new());
    *f.setup.release.lock().unwrap() = Some(gate.clone());
    let first = f.ready("Build", &["auto"]).await;
    let second = f.ready("Same folder", &["auto"]).await;
    assert_eq!(f.dispatcher.pass.run().await.started, [first.clone()]);
    f.setup.entered.notified().await;
    let report = f.dispatcher.pass.run().await;
    assert!(report.started.is_empty(), "{report:?}");
    assert_eq!(
        f.setup.runs.lock().unwrap().len(),
        1,
        "one setup per folder"
    );
    let waiting = f.record(&second).await;
    assert_eq!(waiting.assignee, None);
    gate.notify_one();
    f.dispatcher.starter.wait_idle().await;
    assert_eq!(f.setup.runs.lock().unwrap().len(), 1);
    assert_eq!(f.started_cwds().len(), 1);
}

#[tokio::test]
async fn a_pass_that_loses_its_caller_still_runs_to_the_end_and_frees_the_place() {
    use nullradix_app::dispatch::pass::{PassContext, PassPhase, PassReport, PassStep};
    struct Slow {
        entered: Notify,
        gate: Notify,
        starts: std::sync::atomic::AtomicUsize,
        finished: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl PassStep for Slow {
        async fn run(
            &self,
            _context: &PassContext,
            _report: &mut PassReport,
        ) -> Result<(), AppError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.gate.notified().await;
            self.finished.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let f = fixture().await;
    let step = Arc::new(Slow {
        entered: Notify::new(),
        gate: Notify::new(),
        starts: std::sync::atomic::AtomicUsize::new(0),
        finished: std::sync::atomic::AtomicUsize::new(0),
    });
    f.dispatcher
        .pass
        .set_step(PassPhase::RecorderFlush, step.clone());
    let caller = tokio::spawn({
        let pass = f.dispatcher.pass.clone();
        async move { pass.run().await }
    });
    step.entered.notified().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    // the next caller joins the pass that goes on
    let joined = tokio::spawn({
        let pass = f.dispatcher.pass.clone();
        async move { pass.run().await }
    });
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    step.gate.notify_one();
    let report = joined.await.unwrap();
    assert!(report.errors.is_empty());
    assert_eq!(step.starts.load(Ordering::SeqCst), 1, "one pass for both");
    assert_eq!(step.finished.load(Ordering::SeqCst), 1, "not cut off");
    // the place is free again: the next call is a new pass
    step.gate.notify_one();
    f.dispatcher.pass.run().await;
    assert_eq!(step.starts.load(Ordering::SeqCst), 2);
    assert_eq!(step.finished.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn shutdown_cancels_running_setups_and_gives_their_tickets_back() {
    let f = fixture().await;
    f.configure(setup_config).await;
    *f.setup.release.lock().unwrap() = Some(Arc::new(Notify::new()));
    let key = f.ready("Build", &["auto"]).await;
    assert_eq!(f.dispatcher.pass.run().await.started, [key.clone()]);
    f.setup.entered.notified().await;
    f.dispatcher
        .starter
        .shutdown(std::time::Duration::from_secs(30))
        .await;
    assert_eq!(f.dispatcher.starter.launching(), 0);
    assert!(f.started_cwds().is_empty());
    let record = f.record(&key).await;
    assert_eq!(record.assignee, None);
    assert_eq!(record.status, nullradix_domain::TicketStatus::Planned);
    let (_, state) = f.worktrees.setup_state(&key).await.unwrap().unwrap();
    assert_eq!(
        state,
        SetupState::Pending,
        "the next start runs the setup again"
    );
}

#[tokio::test]
async fn a_start_that_always_fails_counts_attempts_until_the_ticket_is_blocked() {
    let f = fixture().await;
    f.enable().await;
    f.machine.fail_create.store(true, Ordering::SeqCst);
    let key = f.ready("Build", &["auto"]).await;
    let first = f.dispatcher.pass.run().await;
    assert_eq!(first.errors.len(), 1, "{first:?}");
    let after_first = f.record(&key).await;
    assert_eq!(after_first.assignee, None, "the claim was given back");
    assert_eq!(after_first.status, TicketStatus::Planned);
    assert_eq!(
        f.abort_bodies(&key).await,
        [
            "[dispatch] abort: attempt 1/2: the worker could not be started: git is broken.\nThe ticket goes back to planned."
        ]
    );
    let second = f.dispatcher.pass.run().await;
    assert_eq!(second.errors.len(), 1, "{second:?}");
    let last = f.record(&key).await;
    assert_eq!(last.status, TicketStatus::Blocked);
    assert_eq!(last.assignee, None);
    assert_eq!(
        last.blocked_reason.as_deref(),
        Some("Automatic implementation aborted 2 times - please look at it")
    );
    let bodies = f.abort_bodies(&key).await;
    assert_eq!(bodies.len(), 2);
    assert!(bodies[1].contains("attempt 2/2"));
    assert!(bodies[1].contains("the ticket goes to blocked."));
    // a blocked ticket is left alone
    let third = f.dispatcher.pass.run().await;
    assert!(third.errors.is_empty(), "{third:?}");
    assert_eq!(f.record(&key).await, last);
}

#[tokio::test]
async fn a_failed_start_of_a_ticket_a_person_claimed_meanwhile_is_not_counted() {
    let f = fixture().await;
    f.enable().await;
    f.machine.fail_create.store(true, Ordering::SeqCst);
    let key = f.ready("Build", &["auto"]).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.abort_bodies(&key).await.len(), 1);
    let version = f.record(&key).await.version;
    let patch = TicketPatch {
        assignee: Some(Some(String::from("anna"))),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&key, &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    f.dispatcher.pass.run().await;
    assert_eq!(f.abort_bodies(&key).await.len(), 1);
    assert_eq!(f.record(&key).await.assignee.as_deref(), Some("anna"));
}

#[tokio::test]
async fn switching_to_the_same_value_and_pausing_to_the_same_moment_write_nothing() {
    use nullradix_app::dispatch::control::PauseFor;
    let f = fixture().await;
    let on = f.dispatcher.service.set_enabled(true).await.unwrap();
    let again = f.dispatcher.service.set_enabled(true).await.unwrap();
    assert_eq!(on.version, again.version);
    let until = Timestamp::from_millis(START + 60 * MINUTE);
    let paused = f
        .dispatcher
        .service
        .pause(PauseFor::Until(until))
        .await
        .unwrap();
    let same = f
        .dispatcher
        .service
        .pause(PauseFor::Until(until))
        .await
        .unwrap();
    assert_eq!(paused.version, same.version);
}

#[tokio::test]
async fn a_pause_may_not_reach_beyond_a_day() {
    use nullradix_app::dispatch::control::PauseFor;
    let f = fixture().await;
    let day = 24 * 60 * MINUTE;
    let limit = Timestamp::from_millis(START + day);
    assert!(
        f.dispatcher
            .service
            .pause(PauseFor::Until(limit))
            .await
            .is_ok()
    );
    let beyond = Timestamp::from_millis(START + day + 1);
    let refused = f
        .dispatcher
        .service
        .pause(PauseFor::Until(beyond))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "dispatcher-pause-invalid");
}

#[tokio::test]
async fn a_pass_removes_log_lines_older_than_thirty_days() {
    use nullradix_app::IdGenerator;
    use nullradix_app::dispatch::log::{DispatcherLogRepository, LogLevel, LogLine};
    let f = fixture().await;
    let ids = UuidV7Generator;
    let old = LogLine {
        id: ids.next_uuid(),
        at: Timestamp::from_millis(START - 31 * 24 * 60 * MINUTE),
        level: LogLevel::Info,
        message: String::from("ancient"),
        run: None,
        subject: None,
    };
    f.store.append(&old).await.unwrap();
    assert_eq!(f.store.recent(10).await.unwrap().len(), 1);
    f.dispatcher.pass.run().await;
    assert!(f.store.recent(10).await.unwrap().is_empty());
}

/// A pass step that waits for its test, or panics.
struct Gated {
    entered: Notify,
    gate: Notify,
    starts: std::sync::atomic::AtomicUsize,
    panics: bool,
}

#[async_trait]
impl nullradix_app::dispatch::pass::PassStep for Gated {
    async fn run(
        &self,
        _context: &nullradix_app::dispatch::pass::PassContext,
        _report: &mut nullradix_app::dispatch::pass::PassReport,
    ) -> Result<(), AppError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panics, "the step is broken");
        self.entered.notify_one();
        self.gate.notified().await;
        Ok(())
    }
}

fn gated(panics: bool) -> Arc<Gated> {
    Arc::new(Gated {
        entered: Notify::new(),
        gate: Notify::new(),
        starts: std::sync::atomic::AtomicUsize::new(0),
        panics,
    })
}

#[tokio::test]
async fn a_setup_that_fails_after_the_ticket_changed_does_not_block_it() {
    let f = fixture().await;
    f.configure(setup_config).await;
    *f.setup.failure.lock().unwrap() = Some(SetupFailure {
        cwd: String::from("."),
        run: String::from("npm ci"),
        code: Some(1),
        timed_out: false,
        output: String::from("ERESOLVE"),
    });
    let gate = Arc::new(Notify::new());
    *f.setup.release.lock().unwrap() = Some(gate.clone());
    let key = f.ready("Build", &["auto"]).await;
    assert_eq!(f.dispatcher.pass.run().await.started, [key.clone()]);
    f.setup.entered.notified().await;
    f.tickets
        .release(&key, Some("planned"), &f.person, None)
        .await
        .unwrap();
    gate.notify_one();
    f.dispatcher.starter.wait_idle().await;
    let record = f.record(&key).await;
    assert_eq!(record.status, nullradix_domain::TicketStatus::Planned);
    assert_eq!(record.blocked_reason, None);
    assert_eq!(record.assignee, None);
}

#[tokio::test]
async fn a_run_for_a_ticket_that_changed_while_it_started_is_stopped() {
    let f = fixture().await;
    f.enable().await;
    let key = f.ready("Build", &["auto"]).await;
    let gate = Arc::new(Notify::new());
    *f.machine.hold_start.lock().unwrap() = Some(gate.clone());
    let pass = tokio::spawn({
        let pass = f.dispatcher.pass.clone();
        async move { pass.run().await }
    });
    f.machine.start_entered.notified().await;
    f.tickets
        .release(&key, Some("planned"), &f.person, None)
        .await
        .unwrap();
    gate.notify_one();
    let report = pass.await.unwrap();
    assert!(report.started.is_empty(), "{report:?}");
    assert_eq!(f.machine.cancelled.lock().unwrap().len(), 1);
    let comments = f.comments.list(&key).await.unwrap();
    assert!(comments.iter().all(|line| {
        line.comment.record().system_marker != Some(nullradix_domain::SystemMarker::Start)
    }));
}

#[tokio::test]
async fn a_ticket_whose_worktree_is_being_set_up_is_not_claimed() {
    let f = fixture().await;
    f.configure(|config| {
        setup_config(config);
        config.wip = 2;
        config.join_worktrees = nullradix_domain::JoinWorktrees::SameSpec;
    })
    .await;
    let gate = Arc::new(Notify::new());
    *f.setup.release.lock().unwrap() = Some(gate.clone());
    let first = f.ready("Build", &["auto"]).await;
    assert_eq!(f.dispatcher.pass.run().await.started, [first.clone()]);
    f.setup.entered.notified().await;
    // a person joined the second ticket to that worktree and gave it back
    let second = f.ready("Same folder", &["auto"]).await;
    f.worktrees
        .start(
            &second,
            f.local,
            ROOT,
            StartMode::AutoJoin,
            false,
            &f.person,
        )
        .await
        .unwrap();
    f.tickets
        .release(&second, Some("planned"), &f.person, None)
        .await
        .unwrap();
    let before = f.record(&second).await;
    assert!(before.worktree.is_some());
    let report = f.dispatcher.pass.run().await;
    assert!(report.started.is_empty(), "{report:?}");
    assert_eq!(f.record(&second).await, before, "not even claimed");
    gate.notify_one();
    f.dispatcher.starter.wait_idle().await;
}

#[tokio::test]
async fn a_conflict_while_giving_back_a_stale_claim_is_skipped_not_reported() {
    let f = fixture().await;
    let key = f.ready("Build", &["auto"]).await;
    let version = f.record(&key).await.version;
    let patch = TicketPatch {
        assignee: Some(Some(String::from("worker-ARC-0001"))),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&key, &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    f.advance(6);
    *f.hook.bump.lock().unwrap() = Some(key.clone());
    let report = f.dispatcher.pass.run().await;
    assert!(report.released.is_empty(), "{report:?}");
    assert!(report.errors.is_empty(), "{report:?}");
    assert!(f.record(&key).await.assignee.is_some());
}

#[tokio::test]
async fn a_pass_that_panics_gives_a_waiter_a_report_with_the_error() {
    use nullradix_app::dispatch::pass::PassPhase;
    let f = fixture().await;
    let step = gated(true);
    f.dispatcher
        .pass
        .set_step(PassPhase::RecorderFlush, step.clone());
    let report = f.dispatcher.pass.run().await;
    assert!(!report.errors.is_empty(), "{report:?}");
    assert_eq!(step.starts.load(Ordering::SeqCst), 2, "retried once");
}

#[tokio::test]
async fn settle_waits_for_a_pass_that_lost_its_caller() {
    use nullradix_app::dispatch::pass::PassPhase;
    let f = fixture().await;
    let step = gated(false);
    f.dispatcher
        .pass
        .set_step(PassPhase::RecorderFlush, step.clone());
    let caller = tokio::spawn({
        let pass = f.dispatcher.pass.clone();
        async move { pass.run().await }
    });
    step.entered.notified().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let settled = tokio::spawn({
        let pass = f.dispatcher.pass.clone();
        async move { pass.settle().await }
    });
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(!settled.is_finished(), "the pass is still running");
    step.gate.notify_one();
    settled.await.unwrap();
    // nothing runs any more, so settling again returns at once
    f.dispatcher.pass.settle().await;
}

#[tokio::test]
async fn a_pass_that_was_under_way_cannot_add_a_setup_after_shutdown() {
    let f = fixture().await;
    f.configure(setup_config).await;
    let key = f.ready("Build", &["auto"]).await;
    let gate = Arc::new(Notify::new());
    *f.machine.hold_create.lock().unwrap() = Some(gate.clone());
    let pass = tokio::spawn({
        let pass = f.dispatcher.pass.clone();
        async move { pass.run().await }
    });
    f.machine.create_entered.notified().await;
    f.dispatcher
        .starter
        .shutdown(std::time::Duration::from_secs(30))
        .await;
    gate.notify_one();
    let report = pass.await.unwrap();
    assert!(report.started.is_empty(), "{report:?}");
    assert!(f.setup.runs.lock().unwrap().is_empty(), "no setup began");
    assert_eq!(f.dispatcher.starter.launching(), 0);
    let record = f.record(&key).await;
    assert_eq!(record.assignee, None);
    assert_eq!(record.status, nullradix_domain::TicketStatus::Planned);
}

impl Fixture {
    /// The runner reports the run of the ticket as ended cleanly.
    async fn finish_run(&self, key: &str) -> Id<Run> {
        let filter = RunFilter {
            subject_key: Some(key.to_owned()),
            ..RunFilter::default()
        };
        let runs = nullradix_app::runs::RunRepository::find(self.store.as_ref(), &filter, 1);
        let run = runs.await.unwrap().remove(0);
        let end = RunEnd {
            exit_code: Some(0),
            error: None,
        };
        let now = Timestamp::from_millis(START + 1);
        let next = run.finished(RunOutcome::Succeeded, end, now).unwrap();
        nullradix_app::runs::RunRepository::update(self.store.as_ref(), &next, run.version())
            .await
            .unwrap();
        run.id()
    }

    async fn run_of(&self, run: Id<Run>) -> Run {
        nullradix_app::runs::RunRepository::get(self.store.as_ref(), run)
            .await
            .unwrap()
            .unwrap()
    }

    async fn outcome(&self, run: Id<Run>) -> Option<DispatchOutcome> {
        self.run_of(run).await.outcome()
    }

    /// What the agent does with its ticket before it ends.
    async fn agent_changes(&self, key: &str, status: &str, reason: Option<&str>) {
        let version = self.record(key).await.version;
        let patch = TicketPatch {
            status: Some(status.to_owned()),
            blocked_reason: reason.map(|reason| Some(reason.to_owned())),
            ..TicketPatch::default()
        };
        self.tickets
            .update(key, &patch, &self.person, version, FORCE)
            .await
            .unwrap();
    }

    async fn start_tickets(&self, count: usize) -> Vec<String> {
        self.configure(|config| config.wip = i64::try_from(count.max(3)).unwrap())
            .await;
        let mut keys = Vec::new();
        for index in 0..count {
            keys.push(self.ready(&format!("Ticket {index}"), &["auto"]).await);
        }
        let report = self.dispatcher.pass.run().await;
        assert_eq!(report.started.len(), count, "{report:?}");
        keys
    }

    async fn comment_count(&self, key: &str) -> usize {
        self.comments.list(key).await.unwrap().len()
    }

    async fn markers(&self, key: &str, marker: SystemMarker) -> usize {
        self.comments
            .list(key)
            .await
            .unwrap()
            .iter()
            .filter(|line| line.comment.record().system_marker == Some(marker))
            .count()
    }

    async fn ask_as_agent(&self, key: &str) {
        let input = NewQuestion {
            title: String::from("Which database?"),
            tickets: vec![key.to_owned()],
            ..NewQuestion::default()
        };
        let author = Author {
            by_person: false,
            name: Some("worker-ARC-0001"),
            session: Some("s-1"),
        };
        self.questions
            .create(self.board.project(), &input, author, &self.person)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn finished_runs_are_settled_as_done_human_and_blocked() {
    let f = fixture().await;
    f.enable().await;
    let keys = f.start_tickets(3).await;
    f.agent_changes(&keys[0], "done", None).await;
    f.agent_changes(&keys[1], "blocked", Some("Human Interaction: look at it"))
        .await;
    f.agent_changes(&keys[2], "blocked", Some("no access"))
        .await;
    let mut runs = Vec::new();
    for key in &keys {
        runs.push(f.finish_run(key).await);
    }
    let mut before = Vec::new();
    for key in &keys {
        before.push(f.comment_count(key).await);
    }
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(runs[0]).await, Some(DispatchOutcome::Done));
    assert_eq!(f.outcome(runs[1]).await, Some(DispatchOutcome::Human));
    assert_eq!(f.outcome(runs[2]).await, Some(DispatchOutcome::Blocked));
    // the tickets stay as the agent left them; only the merge of the done one is noted
    assert_eq!(f.record(&keys[0]).await.status, TicketStatus::Done);
    before[0] += 1;
    let blocked = f.record(&keys[1]).await;
    assert_eq!(blocked.assignee.as_deref(), Some("worker-ARC-0002"));
    let mut after = Vec::new();
    for key in &keys {
        after.push(f.comment_count(key).await);
    }
    assert_eq!(after, before);
    // a second pass changes nothing
    let mut settled = Vec::new();
    for run in &runs {
        settled.push(f.run_of(*run).await.version());
    }
    f.dispatcher.pass.run().await;
    for (run, version) in runs.iter().zip(settled) {
        assert_eq!(f.run_of(*run).await.version(), version);
    }
}

#[tokio::test]
async fn a_run_that_ended_with_a_question_gives_the_ticket_back_with_one_comment() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.ask_as_agent(&key).await;
    let run = f.finish_run(&key).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Question));
    let record = f.record(&key).await;
    assert_eq!(record.status, TicketStatus::Planned);
    assert_eq!(record.assignee, None);
    assert!(record.worktree.is_some(), "branch and worktree stay");
    let comments = f.comments.list(&key).await.unwrap();
    let question = comments
        .iter()
        .find(|line| line.comment.record().system_marker == Some(SystemMarker::Question))
        .unwrap();
    assert!(
        question
            .comment
            .body()
            .starts_with("[dispatch] question: waiting on ARC-Q0001")
    );
    assert_eq!(question.comment.record().run, Some(run));
    assert_eq!(question.author_kind, ActorKind::Dispatcher);
    f.dispatcher.pass.run().await;
    assert_eq!(f.markers(&key, SystemMarker::Question).await, 1);
}

#[tokio::test]
async fn a_pass_that_stopped_after_the_question_comment_does_not_post_it_again() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.ask_as_agent(&key).await;
    let run = f.finish_run(&key).await;
    // the earlier pass wrote the comment and stopped before the ticket moved
    let actor = f.dispatcher.starter.actor().await.unwrap();
    let ticket_key = nullradix_domain::TicketKey::parse(&key).unwrap();
    f.comments
        .add_system(
            &ticket_key,
            "[dispatch] question: waiting on ARC-Q0001 - earlier pass",
            Some(SystemMarker::Question),
            Some(run),
            actor,
        )
        .await
        .unwrap();
    f.dispatcher.pass.run().await;
    assert_eq!(f.markers(&key, SystemMarker::Question).await, 1);
    assert_eq!(f.record(&key).await.status, TicketStatus::Planned);
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Question));
}

#[tokio::test]
async fn a_person_editing_the_ticket_while_it_is_given_back_postpones_the_outcome() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.ask_as_agent(&key).await;
    let run = f.finish_run(&key).await;
    // the second read is the one the release is based on
    *f.hook.rename_on_get.lock().unwrap() = Some((key.clone(), 2));
    let report = f.dispatcher.pass.run().await;
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(f.outcome(run).await, None, "left for the next pass");
    assert_eq!(f.record(&key).await.status, TicketStatus::InProgress);
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Question));
    assert_eq!(f.record(&key).await.status, TicketStatus::Planned);
    assert_eq!(f.markers(&key, SystemMarker::Question).await, 1);
}

#[tokio::test]
async fn a_ticket_a_person_took_or_deleted_is_settled_as_taken_or_gone() {
    let f = fixture().await;
    f.enable().await;
    let keys = f.start_tickets(2).await;
    let version = f.record(&keys[0]).await.version;
    let patch = TicketPatch {
        status: Some(String::from("done")),
        assignee: Some(Some(String::from("anna"))),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&keys[0], &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    let version = f.record(&keys[1]).await.version;
    f.tickets
        .delete(&keys[1], &f.person, Some(version))
        .await
        .unwrap();
    let taken = f.finish_run(&keys[0]).await;
    let gone = f.finish_run(&keys[1]).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(taken).await, Some(DispatchOutcome::Taken));
    assert_eq!(f.outcome(gone).await, Some(DispatchOutcome::Gone));
}

#[tokio::test]
async fn autopilot_accepts_a_ticket_that_waits_for_a_person() {
    let f = fixture().await;
    f.enable().await;
    f.configure(|config| config.autopilot.enabled = true).await;
    let key = f.start_tickets(1).await.remove(0);
    f.agent_changes(&key, "blocked", Some("Human Interaction: check the layout"))
        .await;
    let run = f.finish_run(&key).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Done));
    assert_eq!(f.record(&key).await.status, TicketStatus::Done);
    let accepted = "Autopilot: accepted automatically - Human Interaction: check the layout";
    let posted = f
        .comments
        .list(&key)
        .await
        .unwrap()
        .iter()
        .filter(|line| line.comment.body() == accepted)
        .count();
    assert_eq!(posted, 1);
    let marked = f
        .comments
        .list(&key)
        .await
        .unwrap()
        .iter()
        .filter(|line| {
            let record = line.comment.record();
            line.comment.body() == accepted
                && record.system_marker.is_none()
                && record.run == Some(run)
        })
        .count();
    assert_eq!(marked, 1);
    let comments = f.comment_count(&key).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.comment_count(&key).await, comments);
}

#[tokio::test]
async fn a_run_is_judged_by_the_agent_it_started_under() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.agent_changes(&key, "done", None).await;
    let run = f.finish_run(&key).await;
    assert_eq!(
        f.run_of(run).await.dispatch().unwrap().agent.as_deref(),
        Some("worker-ARC-0001")
    );
    f.configure(|config| config.agent_prefix = String::from("bot-"))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Done));
}

fn abort_of(category: AbortCategory, reason: &str) -> Abort {
    Abort {
        category,
        reason: reason.to_owned(),
    }
}

const TIMEOUT_REASON: &str = "time limit of 90 min reached, run stopped";

impl Fixture {
    async fn latest_run(&self, key: &str) -> Run {
        let filter = RunFilter {
            subject_key: Some(key.to_owned()),
            ..RunFilter::default()
        };
        nullradix_app::runs::RunRepository::find(self.store.as_ref(), &filter, 1)
            .await
            .unwrap()
            .remove(0)
    }

    /// The newest run of the ticket ends the way the runner reports it.
    async fn end_run(
        &self,
        key: &str,
        outcome: RunOutcome,
        exit_code: Option<i32>,
        abort: Option<Abort>,
    ) -> Id<Run> {
        let run = self.latest_run(key).await;
        let end = RunEnd {
            exit_code,
            error: None,
        };
        let result = RunResult {
            abort,
            ..RunResult::default()
        };
        let now = Timestamp::from_millis(self.clock.now().as_millis());
        let next = run.finished_with(outcome, end, Some(result), now).unwrap();
        nullradix_app::runs::RunRepository::update(self.store.as_ref(), &next, run.version())
            .await
            .unwrap();
        run.id()
    }

    async fn time_out(&self, key: &str) -> Id<Run> {
        let abort = abort_of(AbortCategory::Timeout, TIMEOUT_REASON);
        self.end_run(key, RunOutcome::TimedOut, None, Some(abort))
            .await
    }

    async fn marked(&self, key: &str, marker: SystemMarker) -> Vec<String> {
        self.comments
            .list(key)
            .await
            .unwrap()
            .iter()
            .filter(|line| line.comment.record().system_marker == Some(marker))
            .map(|line| line.comment.body().to_owned())
            .collect()
    }

    async fn abort_bodies(&self, key: &str) -> Vec<String> {
        self.marked(key, SystemMarker::Abort).await
    }

    async fn profile_of(&self, run: Id<Run>) -> nullradix_domain::RunProfile {
        self.run_of(run).await.dispatch().unwrap().profile.clone()
    }

    async fn stop_starting(&self) {
        self.dispatcher.service.set_enabled(false).await.unwrap();
    }
}

#[tokio::test]
async fn a_run_that_ran_out_of_time_is_an_aborted_attempt_and_the_ticket_goes_back() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    let run = f.time_out(&key).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Abort));
    let record = f.record(&key).await;
    assert_eq!(record.status, TicketStatus::Planned);
    assert_eq!(record.assignee, None);
    assert_eq!(
        f.abort_bodies(&key).await,
        [format!(
            "[dispatch] abort: attempt 1/2: {TIMEOUT_REASON}.\nThe ticket goes back to planned.\nRun: {run}"
        )]
    );
    // a second pass counts nothing again
    let version = f.run_of(run).await.version();
    f.dispatcher.pass.run().await;
    assert_eq!(f.run_of(run).await.version(), version);
    assert_eq!(f.abort_bodies(&key).await.len(), 1);
}

#[tokio::test]
async fn every_way_a_run_can_end_without_an_outcome_counts_with_its_own_reason() {
    let f = fixture().await;
    f.enable().await;
    let keys = f.start_tickets(5).await;
    f.stop_starting().await;
    let max_turns = "maximum number of turns (80) reached";
    let exit = "claude exited with code 2";
    let error = "claude reported an error (error_during_execution)";
    let context = "context full: prompt is too long";
    let none = "run ended without a result - the ticket is neither done nor blocked and has no open question";
    let cases = [
        (
            RunOutcome::Failed,
            Some(1),
            Some(abort_of(AbortCategory::MaxTurns, max_turns)),
            max_turns,
        ),
        (
            RunOutcome::Failed,
            Some(2),
            Some(abort_of(AbortCategory::ExitCode, exit)),
            exit,
        ),
        (
            RunOutcome::Failed,
            Some(0),
            Some(abort_of(AbortCategory::ClaudeError, error)),
            error,
        ),
        (
            RunOutcome::Failed,
            Some(1),
            Some(abort_of(AbortCategory::Context, context)),
            context,
        ),
        (RunOutcome::Succeeded, Some(0), None, none),
    ];
    let mut runs = Vec::new();
    for (key, (outcome, code, abort, _)) in keys.iter().zip(cases.iter().cloned()) {
        runs.push(f.end_run(key, outcome, code, abort).await);
    }
    f.dispatcher.pass.run().await;
    for ((key, run), case) in keys.iter().zip(runs).zip(&cases) {
        assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Abort), "{key}");
        let bodies = f.abort_bodies(key).await;
        assert_eq!(bodies.len(), 1, "{key}");
        assert!(
            bodies[0].starts_with(&format!("[dispatch] abort: attempt 1/2: {}.", case.3)),
            "{}",
            bodies[0]
        );
        assert_eq!(f.record(key).await.status, TicketStatus::Planned);
    }
}

#[tokio::test]
async fn a_vanished_run_is_an_aborted_attempt() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    let run = f.latest_run(&key).await;
    let lost = run.lost(f.clock.now()).unwrap();
    nullradix_app::runs::RunRepository::update(f.store.as_ref(), &lost, run.version())
        .await
        .unwrap();
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run.id()).await, Some(DispatchOutcome::Abort));
    assert!(
        f.abort_bodies(&key).await[0]
            .starts_with("[dispatch] abort: attempt 1/2: the run vanished without a result.")
    );
}

#[tokio::test]
async fn the_last_attempt_blocks_the_ticket_and_ends_as_a_final_abort() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    let first = f.time_out(&key).await;
    // the pass settles the first run and starts the second attempt
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [key.clone()], "{report:?}");
    assert_eq!(f.outcome(first).await, Some(DispatchOutcome::Abort));
    let exit = abort_of(AbortCategory::ExitCode, "claude exited with code 2");
    let second = f
        .end_run(&key, RunOutcome::Failed, Some(2), Some(exit))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(second).await, Some(DispatchOutcome::AbortFinal));
    let record = f.record(&key).await;
    assert_eq!(record.status, TicketStatus::Blocked);
    assert_eq!(record.assignee, None);
    assert_eq!(
        record.blocked_reason.as_deref(),
        Some("Automatic implementation aborted 2 times - please look at it")
    );
    let bodies = f.abort_bodies(&key).await;
    assert_eq!(bodies.len(), 2);
    assert!(bodies[1].starts_with("[dispatch] abort: attempt 2/2: claude exited with code 2."));
    assert!(bodies[1].contains("That was the last automatic attempt"));
    assert_eq!(f.notice_kinds(), [NoticeKind::Blocked]);
    // nothing starts again
    let report = f.dispatcher.pass.run().await;
    assert!(report.started.is_empty(), "{report:?}");
    assert_eq!(f.machine.specs.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn three_aborts_end_as_a_final_abort_and_each_start_climbs_the_ladder() {
    let f = fixture().await;
    f.configure(|config| {
        config.enabled = true;
        config.max_attempts = 3;
    })
    .await;
    let key = f.ready("Build", &["auto"]).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [key.clone()]);
    let first = f.latest_run(&key).await.id();
    assert_eq!(f.profile_of(first).await.name, "medium");
    f.time_out(&key).await;
    f.dispatcher.pass.run().await;
    let second = f.latest_run(&key).await.id();
    assert_ne!(second, first);
    let profile = f.profile_of(second).await;
    assert_eq!(
        (profile.name.as_str(), profile.effort.as_deref()),
        ("large", Some("max"))
    );
    let starts = f.marked(&key, SystemMarker::Start).await;
    assert_eq!(starts.len(), 2);
    assert!(starts[0].contains("profile medium"));
    assert!(starts[1].contains("profile large"));
    assert!(
        starts[1].contains("default profile medium, attempt 2: escalated to large"),
        "{}",
        starts[1]
    );
    f.time_out(&key).await;
    f.dispatcher.pass.run().await;
    // the top step repeats
    let third = f.latest_run(&key).await.id();
    assert_ne!(third, second);
    assert_eq!(f.profile_of(third).await.name, "large");
    f.time_out(&key).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(third).await, Some(DispatchOutcome::AbortFinal));
    assert_eq!(f.record(&key).await.status, TicketStatus::Blocked);
    let bodies = f.abort_bodies(&key).await;
    assert_eq!(bodies.len(), 3);
    assert!(bodies[2].contains("attempt 3/3"));
}

#[tokio::test]
async fn escalation_can_be_switched_off() {
    let f = fixture().await;
    f.configure(|config| {
        config.enabled = true;
        config.escalate = false;
        config.max_attempts = 3;
    })
    .await;
    let key = f.ready("Build", &["auto"]).await;
    f.dispatcher.pass.run().await;
    f.time_out(&key).await;
    f.dispatcher.pass.run().await;
    let second = f.latest_run(&key).await.id();
    assert_eq!(f.profile_of(second).await.name, "medium");
}

#[tokio::test]
async fn model_and_effort_labels_override_the_profile_and_are_named_in_the_start_comment() {
    let f = fixture().await;
    f.configure(|config| {
        config.enabled = true;
        config.wip = 3;
    })
    .await;
    let cheap = f
        .ready("Cheap", &["auto", "model:haiku", "effort:low"])
        .await;
    let other = f
        .ready("Other", &["auto", "model:opus", "effort:low"])
        .await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started.len(), 2, "{report:?}");
    let haiku = f.profile_of(f.latest_run(&cheap).await.id()).await;
    assert_eq!(haiku.model.as_deref(), Some("haiku"));
    assert_eq!(haiku.effort, None, "haiku knows no effort");
    let opus = f.profile_of(f.latest_run(&other).await.id()).await;
    assert_eq!(
        (
            opus.name.as_str(),
            opus.model.as_deref(),
            opus.effort.as_deref()
        ),
        ("medium", Some("opus"), Some("low"))
    );
    let args: Vec<Vec<String>> = f
        .machine
        .specs
        .lock()
        .unwrap()
        .iter()
        .map(|spec| spec.args.clone())
        .collect();
    let pair = |args: &[String], flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|at| args.get(at + 1).cloned())
    };
    // the specs come in the order the tickets started
    let started_cheap = pair(&args[0], "--model").as_deref() == Some("haiku");
    let (cheap_args, other_args) = if started_cheap {
        (&args[0], &args[1])
    } else {
        (&args[1], &args[0])
    };
    assert_eq!(pair(cheap_args, "--model").as_deref(), Some("haiku"));
    assert_eq!(pair(cheap_args, "--effort"), None);
    assert_eq!(pair(other_args, "--effort").as_deref(), Some("low"));
    let start = f.marked(&other, SystemMarker::Start).await.remove(0);
    assert!(
        start.contains("default profile medium, label model:opus, label effort:low"),
        "{start}"
    );
}

#[tokio::test]
async fn a_haiku_label_drops_the_effort_the_profile_names() {
    let f = fixture().await;
    f.enable().await;
    let key = f.ready("Cheap", &["auto", "model:haiku"]).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [key.clone()], "{report:?}");
    let profile = f.profile_of(f.latest_run(&key).await.id()).await;
    assert_eq!(profile.model.as_deref(), Some("haiku"));
    assert_eq!(profile.effort, None);
    let specs = f.machine.specs.lock().unwrap();
    assert!(specs[0].args.iter().any(|arg| arg == "haiku"));
    assert!(!specs[0].args.iter().any(|arg| arg == "--effort"));
}

#[tokio::test]
async fn an_invalid_model_label_keeps_the_ticket_from_starting() {
    let f = fixture().await;
    f.enable().await;
    let key = f.ready("Bad", &["auto", "model:gpt"]).await;
    let report = f.dispatcher.pass.run().await;
    assert!(report.started.is_empty(), "{report:?}");
    assert_eq!(f.record(&key).await.status, TicketStatus::Planned);
}

/// Answers the pause hook and notes what it was asked.
struct Pauses {
    answer: Option<DispatchOutcome>,
    asked: Mutex<Vec<(PauseKind, String)>>,
}

#[async_trait]
impl PauseHook for Pauses {
    async fn paused(
        &self,
        _run: &Run,
        ticket: &nullradix_domain::TicketRecord,
        kind: PauseKind,
        _config: &nullradix_domain::DispatcherConfig,
    ) -> Result<Option<DispatchOutcome>, AppError> {
        self.asked
            .lock()
            .unwrap()
            .push((kind, ticket.key.to_string()));
        Ok(self.answer)
    }
}

#[tokio::test]
async fn usage_limit_and_missing_claude_go_to_the_pause_hook() {
    let f = fixture().await;
    f.enable().await;
    let keys = f.start_tickets(2).await;
    f.stop_starting().await;
    let hook = Arc::new(Pauses {
        answer: Some(DispatchOutcome::PauseLimit),
        asked: Mutex::new(Vec::new()),
    });
    f.dispatcher.evaluate.set_pause_hook(hook.clone());
    let limit = abort_of(AbortCategory::UsageLimit, "usage limit: limit reached");
    let limit = f
        .end_run(&keys[0], RunOutcome::Failed, Some(1), Some(limit))
        .await;
    let missing = abort_of(
        AbortCategory::StartFailed,
        "claude could not be started: not found",
    );
    let missing = f
        .end_run(&keys[1], RunOutcome::Failed, Some(127), Some(missing))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(limit).await, Some(DispatchOutcome::PauseLimit));
    assert_eq!(f.outcome(missing).await, Some(DispatchOutcome::PauseLimit));
    let asked = hook.asked.lock().unwrap().clone();
    assert_eq!(
        asked,
        [
            (PauseKind::UsageLimit, keys[0].clone()),
            (PauseKind::ClaudeMissing, keys[1].clone())
        ]
    );
    // the hook took the runs: no attempt is counted and the tickets are left to it
    for key in &keys {
        assert!(f.abort_bodies(key).await.is_empty());
        assert_eq!(f.record(key).await.status, TicketStatus::InProgress);
    }
}

#[tokio::test]
async fn without_a_pause_hook_a_usage_limit_counts_no_attempt_and_the_ticket_waits() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.dispatcher.evaluate.set_pause_hook(Arc::new(NoPauseHook));
    let limit = abort_of(AbortCategory::UsageLimit, "usage limit: limit reached");
    let run = f
        .end_run(&key, RunOutcome::Failed, Some(1), Some(limit))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::PauseLimit));
    assert!(f.abort_bodies(&key).await.is_empty());
    let record = f.record(&key).await;
    assert_eq!(
        (record.status, record.assignee),
        (TicketStatus::Planned, None)
    );
    let starts = f.machine.specs.lock().unwrap().len();
    let report = f.dispatcher.pass.run().await;
    assert!(report.started.is_empty(), "{report:?}");
    // the default pause is 60 minutes
    f.advance(61);
    assert_eq!(f.dispatcher.pass.run().await.started, [key.clone()]);
    assert_eq!(f.machine.specs.lock().unwrap().len(), starts + 1);
    assert!(f.abort_bodies(&key).await.is_empty());
}

#[tokio::test]
async fn a_runner_that_cannot_start_counts_no_attempt_and_the_ticket_waits() {
    let f = fixture().await;
    f.enable().await;
    f.machine.fail_start.store(true, Ordering::SeqCst);
    let key = f.ready("Build", &["auto"]).await;
    let first = f.dispatcher.pass.run().await;
    assert_eq!(first.errors.len(), 1, "{first:?}");
    let run = f.latest_run(&key).await;
    assert!(f.abort_bodies(&key).await.is_empty());
    assert_eq!(
        f.record(&key).await.assignee,
        None,
        "the claim was given back"
    );
    let second = f.dispatcher.pass.run().await;
    assert!(
        second.started.is_empty() && second.errors.is_empty(),
        "{second:?}"
    );
    assert_eq!(
        f.outcome(run.id()).await,
        Some(DispatchOutcome::PauseClaudeMissing)
    );
    assert!(f.abort_bodies(&key).await.is_empty());
    // not Claude missing: the dispatcher keeps running and says nothing of it
    assert_eq!(f.paused_until().await, None);
    assert!(f.marked(&key, SystemMarker::Pause).await.is_empty());
    assert!(f.notice_kinds().is_empty());
    f.dispatcher.pass.run().await;
    assert_eq!(f.machine.specs.lock().unwrap().len(), 1, "not tried again");
    f.machine.fail_start.store(false, Ordering::SeqCst);
    f.advance(61);
    assert_eq!(f.dispatcher.pass.run().await.started, [key.clone()]);
    assert!(f.abort_bodies(&key).await.is_empty());
}

#[tokio::test]
async fn a_changed_ticket_ends_the_wait_after_a_start_that_failed() {
    let f = fixture().await;
    f.enable().await;
    f.dispatcher.evaluate.set_pause_hook(Arc::new(NoPauseHook));
    f.machine.fail_start.store(true, Ordering::SeqCst);
    let key = f.ready("Build", &["auto"]).await;
    f.dispatcher.pass.run().await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.machine.specs.lock().unwrap().len(), 1);
    f.machine.fail_start.store(false, Ordering::SeqCst);
    let version = f.record(&key).await.version;
    let patch = TicketPatch {
        title: Some(String::from("Build it")),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&key, &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    assert_eq!(f.dispatcher.pass.run().await.started, [key]);
}

#[tokio::test]
async fn a_failed_runner_start_is_never_settled_as_an_aborted_attempt() {
    let f = fixture().await;
    f.enable().await;
    f.machine.fail_start.store(true, Ordering::SeqCst);
    let key = f.ready("Build", &["auto"]).await;
    f.dispatcher.pass.run().await;
    let run = f.latest_run(&key).await;
    // a person claims the ticket before the pass settles the run
    let version = f.record(&key).await.version;
    let patch = TicketPatch {
        assignee: Some(Some(String::from("anna"))),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&key, &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    f.dispatcher.pass.run().await;
    let outcome = f.outcome(run.id()).await;
    assert_ne!(outcome, Some(DispatchOutcome::Abort));
    assert_ne!(outcome, Some(DispatchOutcome::AbortFinal));
    assert!(f.abort_bodies(&key).await.is_empty());
    assert_eq!(f.record(&key).await.assignee.as_deref(), Some("anna"));
}

impl Fixture {
    /// An older run of a ticket that ended already, as left by an earlier attempt.
    async fn old_run(
        &self,
        template: &Run,
        key: &str,
        id: u128,
        created: i64,
        abort: Option<Abort>,
        settled: Option<DispatchOutcome>,
    ) -> Id<Run> {
        let mut dispatch = template.dispatch().unwrap().clone();
        dispatch.subject_key = Some(key.to_owned());
        dispatch.agent = None;
        let label = nullradix_domain::RunLabel::parse("work").unwrap();
        let id = Id::from_uuid(uuid::Uuid::from_u128(id));
        let run = Run::new(id, label, Timestamp::from_millis(created)).with_dispatch(dispatch);
        let store = self.store.as_ref();
        nullradix_app::runs::RunRepository::insert(store, &run)
            .await
            .unwrap();
        let end = RunEnd {
            exit_code: None,
            error: None,
        };
        let result = RunResult {
            abort,
            ..RunResult::default()
        };
        let now = Timestamp::from_millis(created);
        let ended = run
            .finished_with(RunOutcome::TimedOut, end, Some(result), now)
            .unwrap();
        nullradix_app::runs::RunRepository::update(store, &ended, run.version())
            .await
            .unwrap();
        if let Some(outcome) = settled {
            let next = ended.settled(outcome).unwrap();
            nullradix_app::runs::RunRepository::update(store, &next, ended.version())
                .await
                .unwrap();
        }
        id
    }
}

#[tokio::test]
async fn an_old_run_whose_ticket_was_restarted_leaves_the_ticket_to_the_newer_run() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    let template = f.latest_run(&key).await;
    let old = f.time_out(&key).await;
    // the comment is written, then a person's edit makes the give-back conflict
    *f.hook.rename_on_get.lock().unwrap() = Some((key.clone(), 3));
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(old).await, None, "left for the next pass");
    assert_eq!(f.abort_bodies(&key).await.len(), 1);
    // the ticket was started again meanwhile and its newer run ended and is settled
    f.advance(1);
    let created = f.clock.now().as_millis();
    f.old_run(
        &template,
        &key,
        500,
        created,
        None,
        Some(DispatchOutcome::Other),
    )
    .await;
    let before = f.record(&key).await;
    assert_eq!(before.status, TicketStatus::InProgress);
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(old).await, Some(DispatchOutcome::Abort));
    assert_eq!(f.record(&key).await, before, "the newer claim is untouched");
    assert_eq!(f.abort_bodies(&key).await.len(), 1);
}

#[tokio::test]
async fn an_old_run_does_not_touch_a_ticket_whose_new_run_is_still_being_set_up() {
    let f = fixture().await;
    f.configure(|config| {
        setup_config(config);
        config.wip = 3;
    })
    .await;
    let first = f.ready("First", &["auto"]).await;
    assert_eq!(f.dispatcher.pass.run().await.started, [first.clone()]);
    f.dispatcher.starter.wait_idle().await;
    let template = f.latest_run(&first).await;
    let gate = Arc::new(Notify::new());
    *f.setup.release.lock().unwrap() = Some(gate.clone());
    let key = f.ready("Second", &["auto"]).await;
    assert_eq!(f.dispatcher.pass.run().await.started, [key.clone()]);
    assert!(f.dispatcher.starter.launching_keys().contains(&key));
    let timeout = abort_of(AbortCategory::Timeout, TIMEOUT_REASON);
    let old = f
        .old_run(&template, &key, 600, START - MINUTE, Some(timeout), None)
        .await;
    let before = f.record(&key).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(old).await, None, "left until the setup is over");
    assert!(f.abort_bodies(&key).await.is_empty());
    assert_eq!(f.record(&key).await, before);
    gate.notify_one();
    f.dispatcher.starter.wait_idle().await;
}

#[tokio::test]
async fn a_pass_that_stopped_after_the_abort_comment_finishes_the_ticket_without_a_second_comment()
{
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    let run = f.time_out(&key).await;
    // a person edits the ticket right after the comment: the give-back is based on a stale read
    *f.hook.rename_on_get.lock().unwrap() = Some((key.clone(), 3));
    let report = f.dispatcher.pass.run().await;
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(f.outcome(run).await, None, "left for the next pass");
    assert_eq!(f.abort_bodies(&key).await.len(), 1);
    assert_eq!(f.record(&key).await.status, TicketStatus::InProgress);
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Abort));
    assert_eq!(f.abort_bodies(&key).await.len(), 1, "counted once");
    assert_eq!(f.record(&key).await.status, TicketStatus::Planned);
    assert_eq!(f.record(&key).await.assignee, None);
}

#[tokio::test]
async fn a_pass_that_stopped_after_the_last_abort_comment_still_blocks_the_ticket() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    let first = f.time_out(&key).await;
    // the pass settles the first run and starts the second attempt
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(first).await, Some(DispatchOutcome::Abort));
    f.stop_starting().await;
    let second = f.time_out(&key).await;
    assert_ne!(second, first);
    *f.hook.rename_on_get.lock().unwrap() = Some((key.clone(), 3));
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(second).await, None);
    assert_eq!(f.abort_bodies(&key).await.len(), 2);
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(second).await, Some(DispatchOutcome::AbortFinal));
    assert_eq!(f.abort_bodies(&key).await.len(), 2);
    assert_eq!(f.record(&key).await.status, TicketStatus::Blocked);
}

#[tokio::test]
async fn a_person_who_took_the_ticket_is_not_counted_against_the_agent() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    let run = f.time_out(&key).await;
    let version = f.record(&key).await.version;
    let patch = TicketPatch {
        assignee: Some(Some(String::from("anna"))),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&key, &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Taken));
    assert!(f.abort_bodies(&key).await.is_empty());
}

#[tokio::test]
async fn a_run_that_never_spawned_is_settled_as_a_pause_and_counts_nothing() {
    let f = fixture().await;
    f.enable().await;
    f.dispatcher.evaluate.set_pause_hook(Arc::new(NoPauseHook));
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    let started = f.latest_run(&key).await;
    let dispatch = started.dispatch().unwrap().clone();
    let label = nullradix_domain::RunLabel::parse("work").unwrap();
    let id = Id::from_uuid(uuid::Uuid::from_u128(77));
    let run = Run::new(id, label, Timestamp::from_millis(START - 10)).with_dispatch(dispatch);
    nullradix_app::runs::RunRepository::insert(f.store.as_ref(), &run)
        .await
        .unwrap();
    let failed = run
        .failed_to_start(String::from("no"), Timestamp::from_millis(START - 5))
        .unwrap();
    nullradix_app::runs::RunRepository::update(f.store.as_ref(), &failed, run.version())
        .await
        .unwrap();
    // its start counted no attempt; the pass only records the end
    let ended = f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
    f.dispatcher.pass.run().await;
    assert_eq!(
        f.outcome(id).await,
        Some(DispatchOutcome::PauseClaudeMissing)
    );
    assert_eq!(f.outcome(ended).await, Some(DispatchOutcome::Abort));
    assert_eq!(
        f.abort_bodies(&key).await.len(),
        1,
        "only the run that ended"
    );
}

#[tokio::test]
async fn many_settled_aborts_do_not_crowd_a_later_done_run_out_of_the_batch() {
    let f = fixture().await;
    f.enable().await;
    let keys = f.start_tickets(2).await;
    f.stop_starting().await;
    f.agent_changes(&keys[1], "done", None).await;
    let started = f.latest_run(&keys[0]).await;
    let dispatch = started.dispatch().unwrap().clone();
    let mut old = Vec::new();
    for index in 0..55 {
        let label = nullradix_domain::RunLabel::parse("work").unwrap();
        let id = Id::from_uuid(uuid::Uuid::from_u128(1_000 + index));
        let run =
            Run::new(id, label, Timestamp::from_millis(START - 10)).with_dispatch(dispatch.clone());
        nullradix_app::runs::RunRepository::insert(f.store.as_ref(), &run)
            .await
            .unwrap();
        let end = RunEnd {
            exit_code: Some(1),
            error: None,
        };
        let result = RunResult {
            abort: Some(abort_of(
                AbortCategory::ExitCode,
                "claude exited with code 1",
            )),
            ..RunResult::default()
        };
        let failed = run
            .finished_with(
                RunOutcome::Failed,
                end,
                Some(result),
                Timestamp::from_millis(START - 5),
            )
            .unwrap();
        nullradix_app::runs::RunRepository::update(f.store.as_ref(), &failed, run.version())
            .await
            .unwrap();
        old.push(id);
    }
    let last = f
        .end_run(&keys[0], RunOutcome::Succeeded, Some(0), None)
        .await;
    let done = f.finish_run(&keys[1]).await;
    f.dispatcher.pass.run().await;
    let mut open = 0;
    for run in &old {
        open += usize::from(f.outcome(*run).await.is_none());
    }
    assert_eq!(open, 5, "a pass settles fifty runs");
    assert_eq!(f.outcome(done).await, None);
    f.dispatcher.pass.run().await;
    for run in &old {
        assert!(f.outcome(*run).await.is_some());
    }
    assert_eq!(f.outcome(done).await, Some(DispatchOutcome::Done));
    assert!(f.outcome(last).await.is_some());
}

const HOUR: i64 = 60 * MINUTE;

impl Fixture {
    async fn account(&self, name: &str) -> Account {
        self.accounts.create(name, None).await.unwrap()
    }

    async fn disable(&self, slug: &str) {
        let account = self.accounts.find(slug).await.unwrap();
        let patch = nullradix_app::accounts::AccountPatch {
            enabled: Some(false),
            ..Default::default()
        };
        self.accounts
            .update(account.id(), patch, account.version())
            .await
            .unwrap();
    }

    /// The default account is off, so the test accounts are the only choice.
    async fn with_accounts(&self, names: &[&str]) -> Vec<Account> {
        self.enable().await;
        self.disable("default").await;
        let mut accounts = Vec::new();
        for name in names {
            accounts.push(self.account(name).await);
        }
        accounts
    }

    async fn active_runs(&self) -> Vec<nullradix_domain::Run> {
        nullradix_app::runs::RunRepository::active(self.store.as_ref())
            .await
            .unwrap()
    }

    async fn run_accounts(&self) -> Vec<Option<uuid::Uuid>> {
        self.active_runs()
            .await
            .iter()
            .map(|run| run.dispatch().unwrap().account)
            .collect()
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "the readings take an optional reset"
    )]
    fn resets_in(hours: i64) -> Option<Timestamp> {
        Some(Timestamp::from_millis(START + hours * HOUR))
    }

    async fn pause(&self, account: &Account, until: Timestamp) {
        self.dispatcher
            .pauses
            .pause_account(account.id(), until, PauseReason::UsageLimit, None)
            .await
            .unwrap();
    }

    async fn status_row(&self, key: &str) -> String {
        let status = self.dispatcher.service.status(None).await.unwrap();
        status
            .tickets
            .iter()
            .find(|row| row.key == key)
            .unwrap()
            .reason
            .clone()
    }
}

#[tokio::test]
async fn the_run_starts_with_the_freest_account_and_the_start_comment_says_why() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    let (max, pro) = (&accounts[0], &accounts[1]);
    f.limits.set(max.id(), 80, 10, Fixture::resets_in(4));
    f.limits.set(pro.id(), 10, 5, Fixture::resets_in(4));
    let key = f.ready("Build", &["auto"]).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [key.clone()], "{report:?}");
    assert_eq!(f.run_accounts().await, [Some(pro.id().as_uuid())]);
    let spec = f.machine.specs.lock().unwrap()[0].clone();
    assert_eq!(spec.env.set["CLAUDE_CONFIG_DIR"], "/accounts/pro");
    let comments = f.comments.list(&key).await.unwrap();
    let body = comments.last().unwrap().comment.body().to_owned();
    assert!(
        body.contains(
            "\nAccount: pro (all accounts -> pro, most free capacity: max (80 %), pro (10 %); \
             skipped default: disabled; reserve 5 h 10 %, week 2 % (buffer))."
        ),
        "{body}"
    );
    let run = f.active_runs().await[0].id();
    let starts = UsageSampleRepository::starts(f.store.as_ref(), &[run])
        .await
        .unwrap();
    assert_eq!(starts[&run].five_hour, Some(10));
    assert_eq!(starts[&run].seven_day, Some(5));
}

#[tokio::test]
async fn with_no_account_free_the_ticket_waits_unclaimed_and_the_status_says_why() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    f.limits
        .set(accounts[0].id(), 95, 10, Fixture::resets_in(3));
    f.limits
        .set(accounts[1].id(), 20, 91, Fixture::resets_in(5));
    let key = f.ready("Build", &["auto"]).await;
    let other = f.ready("Other", &["auto"]).await;
    let report = f.dispatcher.pass.run().await;
    assert!(report.started.is_empty(), "{report:?}");
    assert!(report.queued.is_empty(), "no place is taken");
    let idle = report.idle.unwrap();
    assert!(
        idle.starts_with(&format!(
            "{key} waits - no account free (default: disabled; max: full (5 h 95 %) until "
        )),
        "{idle}"
    );
    assert!(idle.contains("; pro: full (week 91 %) until "), "{idle}");
    assert!(
        !idle.contains("free again"),
        "a disabled account has no end: {idle}"
    );
    let record = f.record(&key).await;
    assert_eq!(record.assignee, None);
    assert_eq!(record.status, nullradix_domain::TicketStatus::Planned);
    assert!(f.record(&other).await.assignee.is_none());
    assert!(f.started_cwds().is_empty());
    let row = f.status_row(&key).await;
    assert!(
        row.starts_with("waits - no account free (default: disabled; max: full"),
        "{row}"
    );
    let status = f.dispatcher.service.status(None).await.unwrap();
    assert_eq!(status.message, idle);
    // the limit resets: the next pass starts the ticket
    f.limits
        .set(accounts[0].id(), 10, 10, Fixture::resets_in(3));
    assert_eq!(f.dispatcher.pass.run().await.started, [key]);
}

#[tokio::test]
async fn a_waiting_ticket_does_not_take_the_place_of_a_ready_one() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    f.limits
        .set(accounts[0].id(), 10, 10, Fixture::resets_in(3));
    f.limits
        .set(accounts[1].id(), 99, 10, Fixture::resets_in(3));
    let pinned = f.ready("Pinned", &["auto", "account:pro"]).await;
    let free = f.ready("Free", &["auto", "account:max"]).await;
    // the only place goes to the ticket that has an account
    let status = f.dispatcher.service.status(None).await.unwrap();
    let rows: Vec<&str> = status
        .tickets
        .iter()
        .map(|row| row.reason.as_str())
        .collect();
    assert!(
        rows[0].starts_with("waits - no account free (pro: full"),
        "{rows:?}"
    );
    assert_eq!(rows[1], "ready");
    assert_eq!(f.dispatcher.pass.run().await.started, [free]);
    assert!(f.record(&pinned).await.assignee.is_none());
}

#[tokio::test]
async fn labels_and_profiles_limit_the_accounts_and_the_list_order_counts() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro", "Team"]).await;
    for account in &accounts {
        f.limits.set(account.id(), 10, 10, Fixture::resets_in(4));
    }
    f.limits
        .set(accounts[0].id(), 50, 10, Fixture::resets_in(4));
    f.configure(|config| {
        config.wip = 5;
        config.account_strategy = nullradix_domain::AccountStrategy::Order;
        let default = config.default_profile.clone();
        for profile in &mut config.profiles {
            if profile.name == default {
                profile.accounts = vec![String::from("team"), String::from("max")];
            }
        }
    })
    .await;
    let by_profile = f.ready("Profile", &["auto"]).await;
    let by_label = f.ready("Label", &["auto", "account:pro"]).await;
    let unknown = f.ready("Unknown", &["auto", "account:zed"]).await;
    let bad = f.ready("Bad", &["auto", "account:Max Private"]).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [by_profile, by_label], "{report:?}");
    let ids: Vec<_> = accounts.iter().map(|a| Some(a.id().as_uuid())).collect();
    let mut used = f.run_accounts().await;
    used.sort();
    let mut wanted = vec![ids[2], ids[1]];
    wanted.sort();
    assert_eq!(used, wanted, "the profile list puts team before max");
    assert!(
        f.status_row(&unknown)
            .await
            .contains("waits - no account free (zed: does not exist)")
    );
    assert_eq!(
        f.status_row(&bad).await,
        "label account:Max Private - not a valid account id"
    );
}

#[tokio::test]
async fn a_paused_account_is_skipped_until_it_resumes() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    let (max, pro) = (&accounts[0], &accounts[1]);
    f.limits.set(max.id(), 10, 10, Fixture::resets_in(4));
    f.limits.set(pro.id(), 60, 10, Fixture::resets_in(4));
    f.configure(|config| config.wip = 3).await;
    f.ready("First", &["auto"]).await;
    f.pause(max, Timestamp::from_millis(START + 2 * HOUR)).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.run_accounts().await, [Some(pro.id().as_uuid())]);
    let active = f.dispatcher.pauses.active().await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].until.as_millis(), START + 2 * HOUR);
    // resuming ends the pause before its time
    assert!(f.dispatcher.pauses.resume_account(max.id()).await.unwrap());
    assert!(!f.dispatcher.pauses.resume_account(max.id()).await.unwrap());
    f.ready("Second", &["auto"]).await;
    f.dispatcher.pass.run().await;
    assert!(f.run_accounts().await.contains(&Some(max.id().as_uuid())));
    // a pause ends by time alone
    f.pause(pro, Timestamp::from_millis(START + HOUR)).await;
    assert_eq!(f.dispatcher.pauses.active().await.unwrap().len(), 1);
    f.advance(61);
    assert!(f.dispatcher.pauses.active().await.unwrap().is_empty());
}

#[tokio::test]
async fn pauses_are_checked_and_resuming_the_dispatcher_ends_them_all() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    let pauses = &f.dispatcher.pauses;
    let past = Timestamp::from_millis(START - 1);
    let far = Timestamp::from_millis(START + 15 * 24 * HOUR);
    for until in [past, far] {
        let error = pauses
            .pause_account(accounts[0].id(), until, PauseReason::Manual, None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "account-pause-invalid");
    }
    let unknown = Id::from_uuid(uuid::Uuid::from_u128(9));
    let until = Timestamp::from_millis(START + HOUR);
    let error = pauses
        .pause_account(unknown, until, PauseReason::Manual, None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "account-not-found");
    f.pause(&accounts[0], until).await;
    f.pause(&accounts[1], until).await;
    f.dispatcher.service.resume().await.unwrap();
    assert!(pauses.active().await.unwrap().is_empty());
}

#[tokio::test]
async fn resuming_an_account_ends_a_dispatcher_pause_only_when_it_had_a_pause() {
    use nullradix_app::dispatch::control::PauseFor;
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max"]).await;
    f.dispatcher
        .service
        .pause(PauseFor::Minutes(30))
        .await
        .unwrap();
    f.dispatcher
        .pauses
        .resume_account(accounts[0].id())
        .await
        .unwrap();
    let status = f.dispatcher.service.status(None).await.unwrap();
    assert!(status.paused_until.is_some(), "no account pause, no resume");
    f.pause(&accounts[0], Timestamp::from_millis(START + HOUR))
        .await;
    f.dispatcher
        .pauses
        .resume_account(accounts[0].id())
        .await
        .unwrap();
    let status = f.dispatcher.service.status(None).await.unwrap();
    assert!(status.paused_until.is_none());
}

#[tokio::test]
async fn a_pause_during_the_usage_reading_keeps_the_run_off_that_account() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    let (max, pro) = (&accounts[0], &accounts[1]);
    f.limits.set(max.id(), 10, 10, Fixture::resets_in(4));
    f.limits.set(pro.id(), 50, 10, Fixture::resets_in(4));
    let gate = Arc::new(Notify::new());
    *f.limits.hold.lock().unwrap() = Some(gate.clone());
    f.ready("Build", &["auto"]).await;
    let pass = tokio::spawn({
        let pass = f.dispatcher.pass.clone();
        async move { pass.run().await }
    });
    f.limits.entered.notified().await;
    f.pause(max, Timestamp::from_millis(START + HOUR)).await;
    gate.notify_one();
    let report = pass.await.unwrap();
    assert_eq!(report.started.len(), 1, "{report:?}");
    assert_eq!(f.run_accounts().await, [Some(pro.id().as_uuid())]);
}

#[tokio::test]
async fn a_pause_that_lands_right_after_the_pauses_were_read_is_caught_by_the_check() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    let (max, pro) = (&accounts[0], &accounts[1]);
    f.limits.set(max.id(), 10, 10, Fixture::resets_in(4));
    f.limits.set(pro.id(), 50, 10, Fixture::resets_in(4));
    *f.pauses.inject.lock().unwrap() = Some(AccountPause {
        account: max.id(),
        until: Timestamp::from_millis(START + HOUR),
        reason: PauseReason::UsageLimit,
        run: None,
        created_at: Timestamp::from_millis(START),
    });
    let config = f.config.get().await.unwrap().config;
    let pick = f
        .dispatcher
        .accounts
        .pick(&AccountRequest::default(), &config, PickMode::Start)
        .await
        .unwrap();
    let AccountPick::Picked(chosen) = pick else {
        panic!("no account picked: {pick:?}");
    };
    assert_eq!(chosen.account, pro.id());
}

#[tokio::test]
async fn a_pause_during_the_setup_moves_the_run_to_another_account() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    let (max, pro) = (&accounts[0], &accounts[1]);
    f.configure(setup_config).await;
    f.limits.set(max.id(), 10, 10, Fixture::resets_in(4));
    f.limits.set(pro.id(), 50, 10, Fixture::resets_in(4));
    let gate = Arc::new(Notify::new());
    *f.setup.release.lock().unwrap() = Some(gate.clone());
    f.ready("Build", &["auto"]).await;
    assert_eq!(f.dispatcher.pass.run().await.started.len(), 1);
    f.setup.entered.notified().await;
    f.pause(max, Timestamp::from_millis(START + HOUR)).await;
    gate.notify_one();
    f.dispatcher.starter.wait_idle().await;
    assert_eq!(f.run_accounts().await, [Some(pro.id().as_uuid())]);
}

#[tokio::test]
async fn a_pause_during_the_setup_that_leaves_no_account_gives_the_claim_back() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max"]).await;
    f.configure(setup_config).await;
    f.limits
        .set(accounts[0].id(), 10, 10, Fixture::resets_in(4));
    let gate = Arc::new(Notify::new());
    *f.setup.release.lock().unwrap() = Some(gate.clone());
    let key = f.ready("Build", &["auto"]).await;
    f.dispatcher.pass.run().await;
    f.setup.entered.notified().await;
    f.pause(&accounts[0], Timestamp::from_millis(START + HOUR))
        .await;
    gate.notify_one();
    f.dispatcher.starter.wait_idle().await;
    assert!(f.run_accounts().await.is_empty());
    assert_eq!(f.record(&key).await.assignee, None);
}

#[tokio::test]
async fn only_a_disabled_default_account_leaves_nothing_to_start_with() {
    let f = fixture().await;
    f.enable().await;
    f.disable("default").await;
    let key = f.ready("Build", &["auto"]).await;
    let report = f.dispatcher.pass.run().await;
    assert!(report.started.is_empty());
    assert_eq!(
        report.idle.unwrap(),
        format!("{key} waits - no account free (default: disabled)")
    );
}

#[tokio::test]
async fn the_default_account_runs_with_the_servers_own_login() {
    let f = fixture().await;
    f.enable().await;
    let key = f.ready("Build", &["auto"]).await;
    assert_eq!(f.dispatcher.pass.run().await.started, [key]);
    let default = f.accounts.find("default").await.unwrap();
    assert_eq!(f.run_accounts().await, [Some(default.id().as_uuid())]);
    let spec = f.machine.specs.lock().unwrap()[0].clone();
    assert!(!spec.env.set.contains_key("CLAUDE_CONFIG_DIR"));
}

/// Starts `count` tickets at once and ends them with the given use of the account.
async fn finished_runs(f: &Fixture, account: &Account, count: usize, used: u32) {
    f.configure(|config| config.wip = 20).await;
    for n in 0..count {
        f.ready(&format!("History {n}"), &["auto"]).await;
    }
    let picker = &f.dispatcher.accounts;
    assert_eq!(f.dispatcher.pass.run().await.started.len(), count);
    f.advance(10);
    f.limits
        .set(account.id(), 10 + used, 20 + used, Fixture::resets_in(4));
    for run in f.active_runs().await {
        picker.record_end(run.id(), account.id()).await;
        let ended = run.lost(f.clock.now()).unwrap();
        nullradix_app::runs::RunRepository::update(f.store.as_ref(), &ended, run.version())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn the_reserve_comes_from_the_history_of_finished_runs_instead_of_the_buffer() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max"]).await;
    let max = &accounts[0];
    f.limits.set(max.id(), 10, 20, Fixture::resets_in(4));
    // a limit of its own makes the usage get read although there is one account
    let patch = nullradix_app::accounts::AccountPatch {
        limit_five_hour: Some(Some(90)),
        ..Default::default()
    };
    f.accounts
        .update(max.id(), patch, max.version())
        .await
        .unwrap();
    finished_runs(&f, max, 5, 6).await;
    // 84 % used: the buffer of 10 would make the account tight, the measured 6 does not
    f.limits.set(max.id(), 84, 20, Fixture::resets_in(4));
    let key = f.ready("Next", &["auto"]).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [key.clone()], "{report:?}");
    let comments = f.comments.list(&key).await.unwrap();
    let body = comments.last().unwrap().comment.body().to_owned();
    assert!(
        body.contains("reserve 5 h 6 %, week 6 % (from 5 runs profile:"),
        "{body}"
    );
    assert!(body.contains("); duration about 10 min)."), "{body}");
    // at 85 % the run under way still needs 5 of its 6 points, so a new one does not fit
    f.limits.set(max.id(), 85, 20, Fixture::resets_in(4));
    f.configure(|config| config.wip = 21).await;
    let blocked = f.ready("Blocked", &["auto"]).await;
    let row = f.status_row(&blocked).await;
    assert!(row.contains("tight (5 h 85 % + 11 % expected"), "{row}");
}

#[tokio::test]
async fn starting_tickets_reads_the_cached_limits_and_never_asks_afresh() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    f.configure(|config| config.wip = 20).await;
    f.limits
        .set(accounts[0].id(), 10, 10, Fixture::resets_in(4));
    f.limits
        .set(accounts[1].id(), 20, 10, Fixture::resets_in(4));
    for n in 0..3 {
        f.ready(&format!("Build {n}"), &["auto"]).await;
    }
    assert_eq!(f.dispatcher.pass.run().await.started.len(), 3);
    let asked = f.limits.fresh.lock().unwrap().clone();
    assert!(!asked.is_empty());
    assert!(asked.iter().all(|fresh| !fresh), "{asked:?}");
}

#[tokio::test]
async fn two_starts_with_setup_in_one_pass_do_not_share_a_nearly_full_account() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    let (max, pro) = (&accounts[0], &accounts[1]);
    f.configure(setup_config).await;
    f.configure(|config| config.wip = 20).await;
    // the buffer of 10 fits once on max (75 + 10) but not twice (75 + 10 + 10 > 90)
    f.limits.set(max.id(), 75, 10, Fixture::resets_in(4));
    f.limits.set(pro.id(), 78, 10, Fixture::resets_in(4));
    let gate = Arc::new(Notify::new());
    *f.setup.release.lock().unwrap() = Some(gate.clone());
    f.ready("First", &["auto"]).await;
    f.ready("Second", &["auto"]).await;
    assert_eq!(f.dispatcher.pass.run().await.started.len(), 2);
    // on the single-threaded runtime both setups wait at the gate once they are listed
    while f.setup.runs.lock().unwrap().len() < 2 {
        tokio::task::yield_now().await;
    }
    gate.notify_waiters();
    f.dispatcher.starter.wait_idle().await;
    let mut used = f.run_accounts().await;
    used.sort();
    let mut both = vec![Some(max.id().as_uuid()), Some(pro.id().as_uuid())];
    both.sort();
    assert_eq!(used, both);
}

#[tokio::test]
async fn a_single_account_folder_without_a_login_waits_for_one() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max"]).await;
    f.limits.logged_out.lock().unwrap().insert(accounts[0].id());
    let key = f.ready("Build", &["auto"]).await;
    let report = f.dispatcher.pass.run().await;
    assert!(report.started.is_empty(), "{report:?}");
    let idle = report.idle.unwrap();
    assert!(idle.contains("max: not logged in"), "{idle}");
    assert!(f.status_row(&key).await.contains("not logged in"));
    // no reading of the limits was asked for with one account and no ceiling
    assert!(f.limits.fresh.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_status_reads_the_accounts_once_for_all_its_tickets() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    f.limits
        .set(accounts[0].id(), 10, 10, Fixture::resets_in(4));
    f.limits
        .set(accounts[1].id(), 20, 10, Fixture::resets_in(4));
    for n in 0..3 {
        f.ready(&format!("Build {n}"), &["auto"]).await;
    }
    f.limits.fresh.lock().unwrap().clear();
    f.dispatcher.service.status(None).await.unwrap();
    assert_eq!(f.limits.fresh.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn start_and_end_rows_older_than_the_retention_go_with_the_next_poll() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max"]).await;
    let max = &accounts[0];
    f.limits.set(max.id(), 10, 20, Fixture::resets_in(4));
    let patch = nullradix_app::accounts::AccountPatch {
        limit_five_hour: Some(Some(90)),
        ..Default::default()
    };
    f.accounts
        .update(max.id(), patch, max.version())
        .await
        .unwrap();
    finished_runs(&f, max, 1, 6).await;
    let samples = f.store.as_ref();
    assert_eq!(
        UsageSampleRepository::history(samples, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    let later = f.clock.now().as_millis() + 91 * 24 * 60 * MINUTE;
    let poll = nullradix_app::dispatch::accounts::NewSample {
        id: uuid::Uuid::now_v7(),
        account: max.id(),
        run: None,
        phase: nullradix_app::dispatch::accounts::SamplePhase::Poll,
        usage: nullradix_domain::UsageSnapshot {
            five_hour: Some(1),
            ..Default::default()
        },
        at: Timestamp::from_millis(later),
    };
    assert!(
        UsageSampleRepository::record_poll(samples, &poll)
            .await
            .unwrap()
    );
    assert!(
        UsageSampleRepository::history(samples, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_settled_run_writes_one_end_sample_even_when_the_pass_repeats() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max"]).await;
    let max = &accounts[0];
    f.limits.set(max.id(), 10, 20, Fixture::resets_in(4));
    let key = f.start_tickets(1).await.remove(0);
    f.agent_changes(&key, "done", None).await;
    let run = f.finish_run(&key).await;
    let samples = f.store.as_ref();
    assert!(
        UsageSampleRepository::history(samples, 10)
            .await
            .unwrap()
            .is_empty()
    );
    f.limits.set(max.id(), 16, 25, Fixture::resets_in(4));
    f.dispatcher.pass.run().await;
    assert!(f.outcome(run).await.is_some());
    let history = UsageSampleRepository::history(samples, 10).await.unwrap();
    assert_eq!(history.len(), 1, "{history:?}");
    f.dispatcher.pass.run().await;
    assert_eq!(
        UsageSampleRepository::history(samples, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn a_setup_that_panics_still_frees_its_launch_entry() {
    let f = fixture().await;
    f.configure(|config| {
        config.enabled = true;
        config.worktree_setup = vec![WorktreeSetupStep {
            cwd: String::from("web"),
            run: String::from("npm ci"),
        }];
    })
    .await;
    *f.setup.panics.lock().unwrap() = true;
    f.ready("Build", &["auto"]).await;
    f.dispatcher.pass.run().await;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.dispatcher.starter.wait_idle(),
    )
    .await
    .expect("the launch entry is gone");
    assert_eq!(f.dispatcher.starter.launching(), 0);
}

fn limit_abort() -> Abort {
    abort_of(AbortCategory::UsageLimit, "usage limit: limit reached")
}

impl Fixture {
    async fn paused_until(&self) -> Option<Timestamp> {
        self.dispatcher
            .service
            .status(None)
            .await
            .unwrap()
            .paused_until
    }

    async fn resume_and_enable(&self) {
        self.dispatcher.service.resume().await.unwrap();
        self.enable().await;
    }

    fn notice_kinds(&self) -> Vec<NoticeKind> {
        self.notices
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|notice| notice.kind)
            .collect()
    }

    fn notice_texts(&self) -> Vec<String> {
        self.notices
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|notice| notice.text.clone())
            .collect()
    }

    async fn account_of(&self, key: &str) -> uuid::Uuid {
        self.latest_run(key)
            .await
            .dispatch()
            .unwrap()
            .account
            .unwrap()
    }
}

#[tokio::test]
async fn a_usage_limit_pauses_that_account_and_the_next_run_uses_the_other() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    f.limits.set(accounts[0].id(), 10, 10, None);
    f.limits.set(accounts[1].id(), 10, 10, None);
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    let limited = f.account_of(&key).await;
    let run = f
        .end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::PauseLimit));
    // no reading names a reset, so the pause lasts the configured minutes
    let pauses = f.dispatcher.pauses.active().await.unwrap();
    assert_eq!(pauses.len(), 1);
    assert_eq!(pauses[0].account.as_uuid(), limited);
    assert_eq!(pauses[0].until.as_millis(), START + 60 * MINUTE);
    assert_eq!(pauses[0].run, Some(run));
    assert_eq!(f.paused_until().await, None, "the other account is free");
    assert!(f.abort_bodies(&key).await.is_empty());
    assert_eq!(f.record(&key).await.status, TicketStatus::Planned);
    let body = f.marked(&key, SystemMarker::Pause).await.remove(0);
    assert!(
        body.starts_with("[dispatch] pause: usage limit of account "),
        "{body}"
    );
    assert!(body.contains("another free account"), "{body}");
    assert_eq!(f.notice_kinds(), [NoticeKind::Paused]);
    f.dispatcher.service.set_enabled(true).await.unwrap();
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started.len(), 1, "{report:?}");
    assert_ne!(f.account_of(&key).await, limited);
}

#[tokio::test]
async fn the_pause_lasts_until_the_reset_the_limits_name() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    let limited = f.account_of(&key).await;
    for account in &accounts {
        f.limits.set(account.id(), 95, 10, Fixture::resets_in(3));
    }
    f.end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
        .await;
    f.dispatcher.pass.run().await;
    let pauses = f.dispatcher.pauses.active().await.unwrap();
    let pause = pauses
        .iter()
        .find(|pause| pause.account.as_uuid() == limited)
        .unwrap();
    assert_eq!(pause.until.as_millis(), START + 3 * HOUR);
}

#[tokio::test]
async fn when_every_account_is_paused_the_dispatcher_pauses_until_the_first_returns() {
    let f = fixture().await;
    let accounts = f.with_accounts(&["Max", "Pro"]).await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    let limited = f.account_of(&key).await;
    let other = accounts
        .iter()
        .find(|account| account.id().as_uuid() != limited)
        .unwrap();
    f.pause(other, Timestamp::from_millis(START + 20 * MINUTE))
        .await;
    f.end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(
        f.paused_until().await.map(Timestamp::as_millis),
        Some(START + 20 * MINUTE)
    );
    let body = f.marked(&key, SystemMarker::Pause).await.remove(0);
    assert!(body.contains("All accounts are paused"), "{body}");
    // the account's own time (09:00) differs from the dispatcher's (08:20)
    assert!(body.contains(" paused until 09:00. All"), "{body}");
    assert!(body.contains("nothing new until 08:20."), "{body}");
}

#[tokio::test]
async fn with_one_account_a_usage_limit_pauses_the_whole_dispatcher() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    let run = f
        .end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::PauseLimit));
    assert_eq!(
        f.paused_until().await.map(Timestamp::as_millis),
        Some(START + 60 * MINUTE)
    );
    let body = f.marked(&key, SystemMarker::Pause).await.remove(0);
    assert!(
        body.starts_with("[dispatch] pause: usage limit reached, ticket back to planned."),
        "{body}"
    );
    assert!(body.contains("Does not count as an attempt."), "{body}");
}

#[tokio::test]
async fn the_third_usage_limit_in_a_row_counts_as_an_attempt() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    for round in 0..3 {
        let run = f
            .end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
            .await;
        f.dispatcher.pass.run().await;
        let expected = if round < 2 {
            DispatchOutcome::PauseLimit
        } else {
            DispatchOutcome::Abort
        };
        assert_eq!(f.outcome(run).await, Some(expected), "round {round}");
        if round < 2 {
            f.resume_and_enable().await;
            f.dispatcher.pass.run().await;
        }
    }
    assert_eq!(f.abort_bodies(&key).await.len(), 1);
    assert_eq!(f.marked(&key, SystemMarker::Pause).await.len(), 2);
}

#[tokio::test]
async fn missing_claude_pauses_the_dispatcher_and_counts_no_attempt() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    let missing = abort_of(
        AbortCategory::StartFailed,
        "claude could not be started: not found",
    );
    let run = f
        .end_run(&key, RunOutcome::Failed, Some(127), Some(missing))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(
        f.outcome(run).await,
        Some(DispatchOutcome::PauseClaudeMissing)
    );
    assert_eq!(
        f.paused_until().await.map(Timestamp::as_millis),
        Some(START + 60 * MINUTE)
    );
    assert_eq!(f.record(&key).await.status, TicketStatus::Planned);
    assert!(f.abort_bodies(&key).await.is_empty());
    let body = f.marked(&key, SystemMarker::Pause).await.remove(0);
    assert!(
        body.contains("Claude Code was not found (command \"claude\")"),
        "{body}"
    );
    assert_eq!(f.notice_kinds(), [NoticeKind::Paused]);
}

#[tokio::test]
async fn a_manual_resume_while_the_reset_is_read_is_not_undone() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    f.dispatcher
        .service
        .pause(nullradix_app::dispatch::control::PauseFor::Minutes(120))
        .await
        .unwrap();
    f.end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
        .await;
    let gate = Arc::new(Notify::new());
    *f.limits.hold.lock().unwrap() = Some(gate.clone());
    let pass = f.dispatcher.pass.clone();
    let running = tokio::spawn(async move { pass.run().await });
    f.limits.entered.notified().await;
    f.dispatcher.service.resume().await.unwrap();
    gate.notify_one();
    running.await.unwrap();
    assert_eq!(f.paused_until().await, None);
    let pauses = f.dispatcher.pauses.active().await.unwrap();
    assert_eq!(pauses.len(), 1, "the account stays paused");
    assert_eq!(pauses[0].until.as_millis(), START + 60 * MINUTE);
    let body = f.marked(&key, SystemMarker::Pause).await.remove(0);
    assert!(body.contains("changed by hand"), "{body}");
    assert_eq!(f.record(&key).await.status, TicketStatus::Planned);
}

#[tokio::test]
async fn questions_and_blocked_tickets_are_noticed() {
    let f = fixture().await;
    f.enable().await;
    let keys = f.start_tickets(2).await;
    f.stop_starting().await;
    f.ask_as_agent(&keys[0]).await;
    f.agent_changes(&keys[1], "blocked", Some("no access"))
        .await;
    f.finish_run(&keys[0]).await;
    f.finish_run(&keys[1]).await;
    f.dispatcher.pass.run().await;
    let mut kinds = f.notice_kinds();
    kinds.sort_by_key(|kind| kind.as_str());
    assert_eq!(kinds, [NoticeKind::Blocked, NoticeKind::Question]);
}

#[tokio::test]
async fn a_pause_never_shortens_a_longer_pause_a_person_set() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    f.dispatcher
        .service
        .pause(nullradix_app::dispatch::control::PauseFor::Minutes(20 * 60))
        .await
        .unwrap();
    f.end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(
        f.paused_until().await.map(Timestamp::as_millis),
        Some(START + 20 * HOUR)
    );
    let body = f.marked(&key, SystemMarker::Pause).await.remove(0);
    assert!(!body.contains("changed by hand"), "{body}");
}

#[tokio::test]
async fn the_shown_pause_date_follows_the_real_utc_offset() {
    // (offset in minutes, minutes to wait first, the pause as shown)
    let cases = [
        (-600, 0, "2027-01-15 22:00"),
        (14 * 60, 0, "2027-01-16 22:00"),
        (-300, 0, "2027-01-16 03:00"),
        (600, 14 * 60, "2027-01-17 00:00"),
        (0, 0, "2027-01-16 08:00"),
    ];
    for (offset, wait, shown) in cases {
        let f = fixture().await;
        f.clock.1.store(offset, Ordering::SeqCst);
        f.advance(wait);
        f.enable().await;
        let key = f.start_tickets(1).await.remove(0);
        f.stop_starting().await;
        let account = f.accounts.find("default").await.unwrap();
        f.limits.set(account.id(), 95, 10, Fixture::resets_in(30));
        f.end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
            .await;
        f.dispatcher.pass.run().await;
        let body = f.marked(&key, SystemMarker::Pause).await.remove(0);
        let expected = format!("nothing new until {shown}.");
        assert!(body.contains(&expected), "offset {offset}: {body}");
    }
}

#[tokio::test]
async fn a_limit_notice_names_the_longer_pause_that_holds() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    f.dispatcher
        .service
        .pause(nullradix_app::dispatch::control::PauseFor::Minutes(20 * 60))
        .await
        .unwrap();
    f.end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
        .await;
    f.dispatcher.pass.run().await;
    let body = f.marked(&key, SystemMarker::Pause).await.remove(0);
    assert!(
        body.contains("nothing new until 2027-01-16 04:00."),
        "{body}"
    );
    assert_eq!(
        f.notice_texts(),
        ["usage limit default - pause until 2027-01-16 04:00"]
    );
}

#[tokio::test]
async fn a_missing_claude_notice_names_the_longer_pause_that_holds() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    f.dispatcher
        .service
        .pause(nullradix_app::dispatch::control::PauseFor::Minutes(20 * 60))
        .await
        .unwrap();
    let missing = abort_of(
        AbortCategory::StartFailed,
        "claude could not be started: not found",
    );
    f.end_run(&key, RunOutcome::Failed, Some(127), Some(missing))
        .await;
    f.dispatcher.pass.run().await;
    let body = f.marked(&key, SystemMarker::Pause).await.remove(0);
    assert!(
        body.contains("nothing new until 2027-01-16 04:00, as paused by hand."),
        "{body}"
    );
    assert_eq!(
        f.notice_texts(),
        ["Claude Code not found - paused until 2027-01-16 04:00"]
    );
}

#[tokio::test]
async fn limit_pauses_of_another_account_do_not_count_in_the_streak() {
    let f = fixture().await;
    f.with_accounts(&["Max", "Pro"]).await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    let mut runs = Vec::new();
    for round in 0..3 {
        runs.push(
            f.end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
                .await,
        );
        f.dispatcher.pass.run().await;
        assert_eq!(
            f.outcome(runs[round]).await,
            Some(DispatchOutcome::PauseLimit),
            "round {round}"
        );
        if round < 2 {
            if round == 1 {
                // both accounts are paused now; resuming ends the pauses of both
                f.dispatcher.service.resume().await.unwrap();
            }
            f.dispatcher.service.set_enabled(true).await.unwrap();
            let report = f.dispatcher.pass.run().await;
            assert_eq!(report.started.len(), 1, "{report:?}");
            f.stop_starting().await;
        }
    }
    let first = f.run_of(runs[0]).await.dispatch().unwrap().account;
    let second = f.run_of(runs[1]).await.dispatch().unwrap().account;
    assert_ne!(first, second, "the second run used the other account");
    assert!(f.abort_bodies(&key).await.is_empty());
}

#[tokio::test]
async fn a_reset_beyond_a_day_keeps_the_account_paused_past_the_dispatcher_pause() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.stop_starting().await;
    let account = f.accounts.find("default").await.unwrap();
    f.limits.set(account.id(), 95, 10, Fixture::resets_in(30));
    f.end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
        .await;
    f.dispatcher.pass.run().await;
    let pauses = f.dispatcher.pauses.active().await.unwrap();
    assert_eq!(pauses.len(), 1);
    assert_eq!(pauses[0].until.as_millis(), START + 30 * HOUR);
    assert_eq!(
        f.paused_until().await.map(Timestamp::as_millis),
        Some(START + 24 * HOUR)
    );
    let body = f.marked(&key, SystemMarker::Pause).await.remove(0);
    assert!(
        body.contains("nothing new until 2027-01-16 08:00."),
        "{body}"
    );
    // the dispatcher is free again, the account is not
    f.advance(25 * 60);
    f.dispatcher.service.set_enabled(true).await.unwrap();
    let waiting = f.dispatcher.pass.run().await;
    assert!(waiting.started.is_empty(), "{waiting:?}");
    assert_eq!(f.record(&key).await.status, TicketStatus::Planned);
    f.limits.set(account.id(), 10, 10, None);
    f.advance(6 * 60);
    assert_eq!(f.dispatcher.pass.run().await.started, [key]);
}

#[tokio::test]
async fn a_read_that_fails_after_a_setup_block_still_shows_the_notice() {
    let f = fixture().await;
    f.configure(|config| {
        config.enabled = true;
        config.worktree_setup = vec![WorktreeSetupStep {
            cwd: String::from("web"),
            run: String::from("npm ci"),
        }];
    })
    .await;
    *f.setup.failure.lock().unwrap() = Some(SetupFailure {
        cwd: String::from("web"),
        run: String::from("npm ci"),
        code: Some(1),
        timed_out: false,
        output: String::new(),
    });
    let key = f.ready("Build", &["auto"]).await;
    f.hook.fail_setup_block_reads.store(true, Ordering::SeqCst);
    f.dispatcher.pass.run().await;
    f.dispatcher.starter.wait_idle().await;
    f.hook.fail_setup_block_reads.store(false, Ordering::SeqCst);
    assert_eq!(
        f.record(&key).await.status,
        nullradix_domain::TicketStatus::Blocked
    );
    let notices = f.notices.0.lock().unwrap().clone();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].title, "Build");
}

fn tool_use(id: &str, command: &str) -> String {
    serde_json::json!({ "type": "assistant", "message": { "content": [
        { "type": "tool_use", "id": id, "name": "Bash", "input": { "command": command } },
    ] } })
    .to_string()
}

fn tool_result(id: &str, content: &str, is_error: bool) -> String {
    serde_json::json!({ "type": "user", "message": { "content": [
        { "type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error },
    ] } })
    .to_string()
}

fn changed(path: &str, status: &str, additions: u64, deletions: u64) -> ChangedFile {
    ChangedFile {
        path: path.to_owned(),
        old_path: None,
        status: status.to_owned(),
        additions,
        deletions,
        binary: false,
    }
}

impl Fixture {
    async fn run_id(&self, key: &str) -> Id<Run> {
        let filter = RunFilter {
            subject_key: Some(key.to_owned()),
            ..RunFilter::default()
        };
        nullradix_app::runs::RunRepository::find(self.store.as_ref(), &filter, 1)
            .await
            .unwrap()
            .remove(0)
            .id()
    }

    /// The run printed these lines on its standard output.
    fn print(&self, run: Id<Run>, lines: &[String]) {
        let lines = lines
            .iter()
            .enumerate()
            .map(|(index, text)| OutputLine {
                seq: i64::try_from(index).unwrap() + 1,
                stream: OutputStream::Stdout,
                text: text.clone(),
                continues: false,
            })
            .collect();
        self.machine.outputs.lock().unwrap().insert(run, lines);
    }

    fn git_says(&self, files: Vec<ChangedFile>) {
        *self.machine.changes.lock().unwrap() = Some(BranchChanges {
            target: String::from("main"),
            base: String::from("abc123"),
            files,
            truncated: false,
        });
    }

    async fn review_on(&self) {
        self.configure(|config| {
            config.enabled = true;
            config.review_before_merge = true;
        })
        .await;
    }

    /// A ticket the agent finished, with the run ended and these tests in its output.
    async fn finished_with_tests(&self) -> (String, Id<Run>) {
        let key = self.start_tickets(1).await.remove(0);
        let run = self.run_id(&key).await;
        self.print(
            run,
            &[
                tool_use("t1", "npx vitest run"),
                tool_result("t1", "7 passed", false),
                tool_use("t2", "npm run lint"),
                tool_result("t2", "1 error", true),
            ],
        );
        self.git_says(vec![
            changed("src/a.ts", "M", 3, 1),
            changed("src/b.ts", "A", 5, 0),
        ]);
        self.agent_changes(&key, "done", None).await;
        self.finish_run(&key).await;
        (key, run)
    }

    /// Writes the review of the ticket's worktree as an earlier pass would have.
    async fn earlier_review(&self, key: &str, run: Id<Run>, at: i64) {
        let ticket = self.record(key).await;
        let entry =
            nullradix_app::worktrees::WorktreeRepository::of_ticket(self.store.as_ref(), ticket.id)
                .await
                .unwrap()
                .unwrap();
        let earlier = nullradix_domain::Review::pending(
            Id::from_uuid(uuid::Uuid::now_v7()),
            entry.worktree.id(),
            Some(run),
            String::from("ticket/earlier"),
            Timestamp::from_millis(at),
        );
        nullradix_app::review::ReviewRepository::hold(self.store.as_ref(), &earlier, &[])
            .await
            .unwrap();
    }
}

// ---- research ----

const AGENT_Q1: &str = "research-ARC-Q0001";

impl Fixture {
    async fn open_question(&self, title: &str) -> String {
        let input = NewQuestion {
            title: title.to_owned(),
            ..NewQuestion::default()
        };
        self.questions
            .create(
                self.board.project(),
                &input,
                Author::default(),
                &self.person,
            )
            .await
            .unwrap()
            .view
            .row
            .question
            .record()
            .key
            .to_string()
    }

    async fn research_runs(&self, key: &str) -> Vec<Run> {
        let filter = RunFilter {
            subject_key: Some(key.to_owned()),
            kind: Some(nullradix_domain::RunKind::Research),
            ..RunFilter::default()
        };
        nullradix_app::runs::RunRepository::find(self.store.as_ref(), &filter, 20)
            .await
            .unwrap()
    }

    /// The research agent saves its result with `research submit`.
    async fn submit_research(&self, key: &str, agent: &str, recommended: bool) {
        let option = |title: &str, recommended| nullradix_domain::OptionDraft {
            title: title.to_owned(),
            text: String::from("Do this."),
            recommended,
            ..nullradix_domain::OptionDraft::default()
        };
        let input = nullradix_app::questions::NewResearch {
            context: Some(String::from("Today the code does X.")),
            options: vec![option("Keep", recommended), option("Change", false)],
        };
        let author = Author {
            by_person: false,
            name: Some(agent),
            session: Some("s-research"),
        };
        self.questions
            .add_research(key, &input, author, &self.person)
            .await
            .unwrap();
    }

    async fn tracked(&self, key: &str) -> Option<nullradix_app::dispatch::tracking::Tracked> {
        use nullradix_app::dispatch::tracking::{AutoRunRepository, TrackKind};
        let detail = self.questions.get(key).await.unwrap();
        let id = detail.view.row.question.record().id.as_uuid();
        AutoRunRepository::get(self.store.as_ref(), TrackKind::Research, id)
            .await
            .unwrap()
    }

    async fn error_code(&self, key: &str) -> &'static str {
        self.dispatcher
            .service
            .start_research(key)
            .await
            .unwrap_err()
            .code()
    }

    async fn other_project_question(&self) {
        let projects = nullradix_app::projects::ProjectService::new(
            self.store.clone(),
            self.clock.clone(),
            Arc::new(nullradix_app::UuidV7Generator),
            ChangeSignal::new(),
        );
        let other = projects.create("OTH", "Others").await.unwrap();
        let input = NewQuestion {
            title: String::from("Where?"),
            ..NewQuestion::default()
        };
        self.questions
            .create(other.id(), &input, Author::default(), &self.person)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_person_starts_the_research_of_an_open_question_in_a_detached_worktree() {
    let f = fixture().await;
    // by hand it runs whatever the switch says, and it takes no place of the ticket limit
    let key = f.open_question("Which database?").await;
    assert_eq!(key, "ARC-Q0001");
    let run = f.dispatcher.service.start_research("arc-q1").await.unwrap();
    let dispatch = run.dispatch().unwrap();
    assert_eq!(dispatch.kind, nullradix_domain::RunKind::Research);
    assert_eq!(dispatch.trigger, nullradix_domain::RunTrigger::Manual);
    assert_eq!(dispatch.subject_key.as_deref(), Some("ARC-Q0001"));
    assert_eq!(dispatch.agent.as_deref(), Some(AGENT_Q1));
    let folder = format!("{ROOT}-worktrees/research-ARC-Q0001");
    assert_eq!(dispatch.cwd.replace('\\', "/"), folder);
    let created = f.detached.created.lock().unwrap().clone();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].root, ROOT);
    assert_eq!(created[0].at, "HEAD");
    assert_eq!(created[0].purpose, DetachedPurpose::Research);
    let spec = f.machine.specs.lock().unwrap()[0].clone();
    let prompt = spec.stdin.unwrap();
    assert!(prompt.contains("ARC-Q0001") && prompt.contains("Which database?"));
    assert!(prompt.contains(AGENT_Q1));
    let at = |flag: &str| spec.args.iter().position(|arg| arg == flag).unwrap();
    assert!(at("--disallowedTools") < at("--allowedTools"));
    assert!(spec.args.iter().any(|arg| arg == "Write(./research.json)"));
    assert!(!spec.args.iter().any(|arg| arg == "Bash(*)"));
    assert!(spec.env.remove.contains(&String::from("NULLRADIX_TICKET")));
    let tracked = f.tracked(&key).await.unwrap();
    assert_eq!(tracked.count, 1);
    assert_eq!(tracked.auto_at, None);
    // it does not count for the limit of tickets
    let status = f.dispatcher.service.status(None).await.unwrap();
    assert_eq!(status.active, 0);
}

#[tokio::test]
async fn a_research_is_refused_for_the_reasons_legacy_gave() {
    let f = fixture().await;
    let key = f.open_question("Which database?").await;
    assert_eq!(f.error_code("ARC-Q0099").await, "question-not-found");
    assert_eq!(f.error_code("ARC-0001").await, "question-id-invalid");
    // a question of a project without a board with a repository
    f.other_project_question().await;
    assert_eq!(f.error_code("OTH-Q0001").await, "board-repo-missing");
    // a decided question
    let decided = f.open_question("Decided").await;
    let answer = nullradix_app::questions::Answer {
        text: Some(String::from("Yes")),
        ..nullradix_app::questions::Answer::default()
    };
    f.questions
        .answer(&decided, &answer, Author::default(), &f.person, None)
        .await
        .unwrap();
    assert_eq!(f.error_code(&decided).await, "question-not-open");
    // a folder in the way stays and the start fails
    f.detached.busy.store(true, Ordering::SeqCst);
    assert_eq!(f.error_code(&key).await, "research-worktree-busy");
    f.detached.busy.store(false, Ordering::SeqCst);
    assert!(f.machine.specs.lock().unwrap().is_empty());
    // a pause holds it back
    f.dispatcher
        .service
        .pause(nullradix_app::dispatch::control::PauseFor::Minutes(30))
        .await
        .unwrap();
    assert_eq!(f.error_code(&key).await, "dispatch-paused");
    f.dispatcher.service.resume().await.unwrap();
    // a second start of a question that is being researched
    f.dispatcher.service.start_research(&key).await.unwrap();
    assert_eq!(f.error_code(&key).await, "research-running");
    assert_eq!(f.research_runs(&key).await.len(), 1);
}

#[tokio::test]
async fn a_failed_start_removes_its_worktree_again() {
    let f = fixture().await;
    let key = f.open_question("Which database?").await;
    f.machine.fail_start.store(true, Ordering::SeqCst);
    assert_eq!(f.error_code(&key).await, "claude-missing");
    let removed = f.detached.removed.lock().unwrap().clone();
    assert_eq!(removed, [format!("{ROOT}-worktrees/research-ARC-Q0001")]);
    assert!(f.detached.folders.lock().unwrap().is_empty());
    assert_eq!(f.tracked(&key).await, None);
}

#[tokio::test]
async fn two_starts_of_one_question_make_one_run() {
    let f = fixture().await;
    let key = f.open_question("Which database?").await;
    let gate = Arc::new(Notify::new());
    *f.machine.hold_start.lock().unwrap() = Some(gate.clone());
    let first = {
        let service = f.dispatcher.service.clone();
        let key = key.clone();
        tokio::spawn(async move { service.start_research(&key).await })
    };
    f.machine.start_entered.notified().await;
    let second = {
        let service = f.dispatcher.service.clone();
        let key = key.clone();
        tokio::spawn(async move { service.start_research(&key).await })
    };
    tokio::task::yield_now().await;
    gate.notify_one();
    let first = first.await.unwrap();
    let second = second.await.unwrap();
    assert!(first.is_ok(), "{first:?}");
    assert_eq!(second.unwrap_err().code(), "research-running");
    assert_eq!(f.research_runs(&key).await.len(), 1);
    assert_eq!(f.detached.created.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_stored_research_settles_the_run_and_removes_the_worktree() {
    let f = fixture().await;
    let key = f.open_question("Which database?").await;
    f.dispatcher.service.start_research(&key).await.unwrap();
    f.submit_research(&key, AGENT_Q1, false).await;
    let run = f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(
        f.outcome(run).await,
        Some(DispatchOutcome::Research),
        "{report:?}"
    );
    assert!(f.detached.folders.lock().unwrap().is_empty());
    let detail = f.questions.get(&key).await.unwrap();
    assert_eq!(detail.research.len(), 1);
    assert_eq!(detail.research[0].options.len(), 2);
    assert!(detail.comments.is_empty());
}

#[tokio::test]
async fn a_run_that_leaves_no_research_comments_the_question_once() {
    let f = fixture().await;
    let key = f.open_question("Which database?").await;
    f.dispatcher.service.start_research(&key).await.unwrap();
    // a research of someone else in the meantime is not the agent's
    f.submit_research(&key, "someone-else", false).await;
    let abort = abort_of(AbortCategory::Timeout, TIMEOUT_REASON);
    let run = f
        .end_run(&key, RunOutcome::TimedOut, None, Some(abort))
        .await;
    f.dispatcher.pass.run().await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Other));
    let detail = f.questions.get(&key).await.unwrap();
    assert_eq!(detail.comments.len(), 1);
    let comment = detail.comments[0].comment.record();
    assert_eq!(comment.system_marker, Some(SystemMarker::ResearchEmpty));
    assert_eq!(comment.run, Some(run));
    assert_eq!(detail.comments[0].author_kind, ActorKind::Dispatcher);
    let body = comment.body.as_str();
    assert!(
        body.starts_with("[dispatch] research-empty: time limit of 90 min reached, run stopped."),
        "{body}"
    );
    let record = detail.view.row.question.record();
    assert_eq!(record.state, nullradix_domain::QuestionState::Open);
    assert!(f.detached.folders.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_usage_limit_in_a_research_pauses_the_dispatcher_and_leaves_the_question_to_start_again()
{
    let f = fixture().await;
    let key = f.open_question("Which database?").await;
    f.dispatcher.service.start_research(&key).await.unwrap();
    let run = f
        .end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
        .await;
    f.dispatcher.pass.run().await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::PauseLimit));
    assert_eq!(
        f.paused_until().await.map(Timestamp::as_millis),
        Some(START + 60 * MINUTE)
    );
    let detail = f.questions.get(&key).await.unwrap();
    assert_eq!(detail.comments.len(), 1);
    let comment = detail.comments[0].comment.record();
    assert_eq!(comment.system_marker, Some(SystemMarker::ResearchEmpty));
    let body = comment.body.as_str();
    assert!(
        body.starts_with("[dispatch] research-empty: usage limit reached, research not stored."),
        "{body}"
    );
    assert!(
        body.contains("Start the research again afterwards."),
        "{body}"
    );
    assert_eq!(f.notice_kinds(), [NoticeKind::Paused]);
    assert!(f.detached.folders.lock().unwrap().is_empty());
    // the pause starts nothing new
    assert_eq!(f.research_runs(&key).await.len(), 1);
}

#[tokio::test]
async fn missing_claude_in_a_research_pauses_the_dispatcher() {
    let f = fixture().await;
    let key = f.open_question("Which database?").await;
    f.dispatcher.service.start_research(&key).await.unwrap();
    let missing = abort_of(
        AbortCategory::StartFailed,
        "claude could not be started: not found",
    );
    let run = f
        .end_run(&key, RunOutcome::Failed, Some(127), Some(missing))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(
        f.outcome(run).await,
        Some(DispatchOutcome::PauseClaudeMissing)
    );
    assert_eq!(
        f.paused_until().await.map(Timestamp::as_millis),
        Some(START + 60 * MINUTE)
    );
    let detail = f.questions.get(&key).await.unwrap();
    assert_eq!(detail.comments.len(), 1);
    let body = detail.comments[0].comment.record().body.as_str();
    assert!(
        body.contains("Claude Code was not found (command \"claude\")"),
        "{body}"
    );
    assert_eq!(f.notice_kinds(), [NoticeKind::Paused]);
}

#[tokio::test]
async fn a_worktree_that_stays_is_removed_by_a_later_pass_but_not_while_its_run_lives() {
    let f = fixture().await;
    let key = f.open_question("Which database?").await;
    f.dispatcher.service.start_research(&key).await.unwrap();
    let folder = format!("{ROOT}-worktrees/research-ARC-Q0001");
    // the run is alive: the folder is not an orphan
    f.dispatcher.pass.run().await;
    assert!(f.detached.folders.lock().unwrap().contains(&folder));
    f.detached.keep.store(true, Ordering::SeqCst);
    let run = f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Other));
    assert!(f.detached.folders.lock().unwrap().contains(&folder));
    f.detached.keep.store(false, Ordering::SeqCst);
    f.dispatcher.pass.run().await;
    assert!(f.detached.folders.lock().unwrap().is_empty());
    // a new research of the question may start again
    assert!(f.dispatcher.service.start_research(&key).await.is_ok());
}

impl Fixture {
    async fn auto_research(&self, max_per_day: i64, wip: i64) {
        self.configure(|config| {
            config.research.auto = true;
            config.research.max_per_day = max_per_day;
            config.research.wip = wip;
        })
        .await;
    }
}

#[tokio::test]
async fn the_automation_takes_up_only_questions_that_came_after_it_was_switched_on() {
    let f = fixture().await;
    let old = f.open_question("Old").await;
    f.advance(1);
    f.auto_research(5, 5).await;
    let report = f.dispatcher.pass.run().await;
    assert!(report.research.is_empty(), "{report:?}");
    f.advance(1);
    let fresh = f.open_question("Fresh").await;
    // the dispatcher is off: the automation does not need the switch
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.research, [fresh.clone()], "{report:?}");
    assert!(f.research_runs(&old).await.is_empty());
    let tracked = f.tracked(&fresh).await.unwrap();
    assert_eq!(tracked.count, 1);
    assert!(tracked.auto_at.is_some());
    // already under way or done: no second start
    assert!(f.dispatcher.pass.run().await.research.is_empty());
    f.end_run(&fresh, RunOutcome::Succeeded, Some(0), None)
        .await;
    f.dispatcher.pass.run().await;
    assert!(f.dispatcher.pass.run().await.research.is_empty());
    assert_eq!(f.research_runs(&fresh).await.len(), 1);
    // switched off and on again: the clock starts anew
    f.configure(|config| config.research.auto = false).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    f.auto_research(5, 5).await;
    f.dispatcher.pass.run().await;
    assert!(f.research_runs(&old).await.is_empty());
}

#[tokio::test]
async fn the_automation_keeps_to_its_places_and_to_the_starts_of_a_day() {
    let f = fixture().await;
    f.advance(1);
    f.auto_research(2, 1).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    let first = f.open_question("First").await;
    f.advance(1);
    let second = f.open_question("Second").await;
    f.advance(1);
    let third = f.open_question("Third").await;
    // one place: the oldest goes first
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.research, [first.clone()], "{report:?}");
    assert!(f.dispatcher.pass.run().await.research.is_empty());
    f.end_run(&first, RunOutcome::Succeeded, Some(0), None)
        .await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.research, [second.clone()], "{report:?}");
    f.end_run(&second, RunOutcome::Succeeded, Some(0), None)
        .await;
    // two starts today are the cap
    assert!(f.dispatcher.pass.run().await.research.is_empty());
    assert!(f.research_runs(&third).await.is_empty());
    f.advance(24 * 60);
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.research, [third], "{report:?}");
}

#[tokio::test]
async fn a_change_of_the_clock_inside_a_day_does_not_reset_the_starts_of_the_day() {
    let f = fixture().await;
    f.advance(1);
    f.auto_research(2, 1).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    let first = f.open_question("First").await;
    f.advance(1);
    let second = f.open_question("Second").await;
    f.advance(1);
    let third = f.open_question("Third").await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.research, [first.clone()], "{report:?}");
    f.end_run(&first, RunOutcome::Succeeded, Some(0), None)
        .await;
    // daylight saving moves the local clock by an hour, still the same calendar day
    f.clock.1.store(60, Ordering::SeqCst);
    f.advance(1);
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.research, [second.clone()], "{report:?}");
    f.end_run(&second, RunOutcome::Succeeded, Some(0), None)
        .await;
    assert!(f.dispatcher.pass.run().await.research.is_empty());
    assert!(f.research_runs(&third).await.is_empty());
}

#[tokio::test]
async fn a_manual_research_keeps_a_question_out_of_the_automation() {
    let f = fixture().await;
    f.advance(1);
    f.auto_research(5, 5).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    let key = f.open_question("By hand").await;
    f.dispatcher.service.start_research(&key).await.unwrap();
    f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
    f.dispatcher.pass.run().await;
    assert!(f.dispatcher.pass.run().await.research.is_empty());
    assert_eq!(f.research_runs(&key).await.len(), 1);
}

#[tokio::test]
async fn the_automation_stands_down_while_paused_or_outside_the_window() {
    let f = fixture().await;
    f.advance(1);
    f.auto_research(5, 5).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    let key = f.open_question("Later").await;
    f.dispatcher
        .service
        .pause(nullradix_app::dispatch::control::PauseFor::Minutes(15))
        .await
        .unwrap();
    assert!(f.dispatcher.pass.run().await.research.is_empty());
    f.dispatcher.service.resume().await.unwrap();
    f.configure(|config| {
        config.window = Some(nullradix_domain::TimeWindow {
            from: String::from("13:00"),
            to: String::from("14:00"),
        });
    })
    .await;
    assert!(f.dispatcher.pass.run().await.research.is_empty());
    f.configure(|config| config.window = None).await;
    assert_eq!(f.dispatcher.pass.run().await.research, [key]);
}

#[tokio::test]
async fn a_repository_fault_pauses_the_automation_for_thirty_minutes() {
    let f = fixture().await;
    f.advance(1);
    f.auto_research(5, 5).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    let key = f.open_question("Broken").await;
    f.detached.fail_create.store(true, Ordering::SeqCst);
    let report = f.dispatcher.pass.run().await;
    assert!(report.research.is_empty());
    assert_eq!(report.errors.len(), 1, "{report:?}");
    assert!(report.errors[0].contains("git is broken"), "{report:?}");
    // within the pause nothing is tried, so nothing is reported again
    f.detached.fail_create.store(false, Ordering::SeqCst);
    f.advance(29);
    let report = f.dispatcher.pass.run().await;
    assert!(
        report.research.is_empty() && report.errors.is_empty(),
        "{report:?}"
    );
    f.advance(2);
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.research, [key], "{report:?}");
}

#[tokio::test]
async fn projects_without_a_repository_are_never_candidates() {
    let f = fixture().await;
    f.advance(1);
    f.auto_research(5, 5).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    f.other_project_question().await;
    f.advance(1);
    let key = f.open_question("Fine").await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.research, [key], "{report:?}");
    assert!(report.errors.is_empty(), "{report:?}");
}

impl Fixture {
    async fn autopilot_question(&self) -> (String, String) {
        self.configure(|config| config.autopilot.enabled = true)
            .await;
        let ticket = self.ready("Build", &["auto"]).await;
        self.ask_as_agent(&ticket).await;
        (ticket, String::from("ARC-Q0001"))
    }
}

#[tokio::test]
async fn a_finished_ticket_is_held_for_review_with_the_tests_of_its_run() {
    let f = fixture().await;
    f.review_on().await;
    let (key, run) = f.finished_with_tests().await;
    let report = f.dispatcher.pass.run().await;
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Review));
    let record = f.record(&key).await;
    assert_eq!(record.status, TicketStatus::Blocked);
    assert_eq!(
        record.blocked_reason.as_deref(),
        Some("Human Interaction: review before merge")
    );
    assert_eq!(record.assignee, None);
    let comments = f.comments.list(&key).await.unwrap();
    let held: Vec<_> = comments
        .iter()
        .filter(|line| line.comment.record().system_marker == Some(SystemMarker::Review))
        .collect();
    assert_eq!(held.len(), 1);
    assert_eq!(
        held[0].comment.body(),
        "[dispatch] review: waiting for approval - 2 files changed (+8/-1), tests: 1 green, 1 red.\nAccept it or return it to the agent in the review panel."
    );
    assert_eq!(held[0].comment.record().run, Some(run));
    assert_eq!(held[0].author_kind, ActorKind::Dispatcher);
    let notices = f.notices.0.lock().unwrap().clone();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].kind, NoticeKind::Review);
    assert_eq!(notices[0].ticket.as_deref(), Some(key.as_str()));
    assert_eq!(
        notices[0].text,
        "waiting for approval - 2 files changed (+8/-1)"
    );
    let view = f.reviews.view(&key).await.unwrap();
    assert!(view.available, "{view:?}");
    assert_eq!(view.state, Some(nullradix_domain::ReviewState::Pending));
    assert_eq!(view.run, Some(run));
    assert_eq!(view.target.as_deref(), Some("main"));
    assert_eq!(view.files.len(), 2);
    assert_eq!(view.tests.len(), 2);
    assert_eq!(view.tests[0].command, "npx vitest run");
    assert_eq!(view.tests[0].ok, Some(true));
    assert_eq!(view.tests[1].ok, Some(false));
    assert_eq!(view.tests[1].output, "1 error");
    assert_eq!(view.tests_at, Some(Timestamp::from_millis(START)));
    // a second pass adds nothing
    let comments = f.comment_count(&key).await;
    let version = f.run_of(run).await.version();
    f.dispatcher.pass.run().await;
    assert_eq!(f.comment_count(&key).await, comments);
    assert_eq!(f.run_of(run).await.version(), version);
    assert_eq!(f.notices.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn the_review_reads_the_branch_in_the_checkout_of_the_ticket_repository() {
    let f = fixture().await;
    f.review_on().await;
    let (key, _) = f.finished_with_tests().await;
    f.dispatcher.pass.run().await;
    f.reviews.view(&key).await.unwrap();
    let asked = f.machine.asked.lock().unwrap().clone();
    assert!(!asked.is_empty());
    assert!(asked.iter().all(|request| request.root == ROOT));
    assert!(
        asked
            .iter()
            .all(|request| request.branch.starts_with("ticket/ARC-0001"))
    );
}

#[tokio::test]
async fn without_the_review_setting_or_under_autopilot_a_finished_ticket_stays_done() {
    let f = fixture().await;
    f.review_on().await;
    f.configure(|config| config.review_before_merge = false)
        .await;
    let (key, run) = f.finished_with_tests().await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Done));
    assert_eq!(f.record(&key).await.status, TicketStatus::Done);
    assert_eq!(f.markers(&key, SystemMarker::Review).await, 0);

    let g = fixture().await;
    g.review_on().await;
    g.configure(|config| config.autopilot.enabled = true).await;
    let (key, run) = g.finished_with_tests().await;
    g.dispatcher.pass.run().await;
    assert_eq!(g.outcome(run).await, Some(DispatchOutcome::Done));
    assert_eq!(g.record(&key).await.status, TicketStatus::Done);
    assert_eq!(g.markers(&key, SystemMarker::Review).await, 0);
}

#[tokio::test]
async fn a_pass_that_stopped_after_the_review_was_written_finishes_the_hold_once() {
    let f = fixture().await;
    f.review_on().await;
    let (key, run) = f.finished_with_tests().await;
    f.earlier_review(&key, run, START - 1).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Review));
    assert_eq!(f.record(&key).await.status, TicketStatus::Blocked);
    assert_eq!(f.markers(&key, SystemMarker::Review).await, 1);
    // the review the first pass wrote stays as it was
    let view = f.reviews.view(&key).await.unwrap();
    assert_eq!(view.tests_at, Some(Timestamp::from_millis(START - 1)));
    assert!(view.tests.is_empty());
}

#[tokio::test]
async fn a_held_run_with_its_comment_is_not_blocked_again_after_a_person_accepted() {
    let f = fixture().await;
    f.review_on().await;
    let (key, run) = f.finished_with_tests().await;
    f.earlier_review(&key, run, START - 1).await;
    let ticket = f.record(&key).await;
    f.comments
        .add_system(
            &ticket.key,
            "[dispatch] review: waiting for approval",
            Some(SystemMarker::Review),
            Some(run),
            &f.person,
        )
        .await
        .unwrap();
    // the ticket is done: a person accepted it before the pass settled the run
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Review));
    assert_eq!(f.record(&key).await.status, TicketStatus::Done);
    assert_eq!(f.markers(&key, SystemMarker::Review).await, 1);
}
impl Fixture {
    /// A ticket whose first pass was lost after blocking it, with the comment as given.
    async fn lost_after_blocking(&self, commented: bool) -> (String, Id<Run>) {
        let key = self.start_tickets(1).await.remove(0);
        let run = self.run_id(&key).await;
        self.agent_changes(
            &key,
            "blocked",
            Some("Human Interaction: review before merge"),
        )
        .await;
        let version = self.record(&key).await.version;
        let patch = TicketPatch {
            assignee: Some(None),
            ..TicketPatch::default()
        };
        self.tickets
            .update(&key, &patch, &self.person, version, FORCE)
            .await
            .unwrap();
        self.earlier_review(&key, run, START).await;
        if commented {
            let ticket = self.record(&key).await;
            self.comments
                .add_system(
                    &ticket.key,
                    "[dispatch] review: waiting for approval",
                    Some(SystemMarker::Review),
                    Some(run),
                    &self.person,
                )
                .await
                .unwrap();
        }
        self.finish_run(&key).await;
        (key, run)
    }
}

#[tokio::test]
async fn a_pass_that_stopped_after_commenting_the_hold_settles_the_run_as_held() {
    let f = fixture().await;
    f.review_on().await;
    let (key, run) = f.lost_after_blocking(true).await;
    let comments = f.comment_count(&key).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Review));
    assert_eq!(f.record(&key).await.status, TicketStatus::Blocked);
    assert_eq!(f.comment_count(&key).await, comments);
}

#[tokio::test]
async fn a_pass_that_stopped_after_blocking_the_ticket_posts_the_missing_comment() {
    let f = fixture().await;
    f.review_on().await;
    let (key, run) = f.lost_after_blocking(false).await;
    assert_eq!(f.markers(&key, SystemMarker::Review).await, 0);
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Review));
    assert_eq!(f.record(&key).await.status, TicketStatus::Blocked);
    assert_eq!(f.markers(&key, SystemMarker::Review).await, 1);
    // a second pass adds nothing
    f.dispatcher.pass.run().await;
    assert_eq!(f.markers(&key, SystemMarker::Review).await, 1);
}

#[tokio::test]
async fn a_ticket_a_person_accepted_after_a_lost_pass_is_never_blocked_again() {
    let f = fixture().await;
    f.review_on().await;
    let (key, run) = f.lost_after_blocking(false).await;
    // the lost pass blocked the ticket and noted it, but never commented
    nullradix_app::review::ReviewRepository::mark_blocked(
        f.store.as_ref(),
        run,
        Timestamp::from_millis(START),
    )
    .await
    .unwrap();
    // a person accepts the ticket meanwhile
    let version = f.record(&key).await.version;
    let patch = TicketPatch {
        status: Some(TicketStatus::Done.as_str().to_owned()),
        blocked_reason: Some(None),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&key, &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    let report = f.dispatcher.pass.run().await;
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Review));
    assert_eq!(f.record(&key).await.status, TicketStatus::Done);
    // the history is complete, once
    assert_eq!(f.markers(&key, SystemMarker::Review).await, 1);
    assert_eq!(f.notices.0.lock().unwrap().len(), 1);
    f.dispatcher.pass.run().await;
    assert_eq!(f.record(&key).await.status, TicketStatus::Done);
    assert_eq!(f.markers(&key, SystemMarker::Review).await, 1);
    assert_eq!(f.notices.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_person_changing_the_ticket_while_it_is_held_does_not_stop_the_hold() {
    let f = fixture().await;
    f.review_on().await;
    let (key, run) = f.finished_with_tests().await;
    // reads: the settle, the check for the comment, then the first try to block the ticket
    *f.hook.rename_on_get.lock().unwrap() = Some((key.clone(), 3));
    let report = f.dispatcher.pass.run().await;
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Review));
    let record = f.record(&key).await;
    assert_eq!(record.status, TicketStatus::Blocked);
    assert_eq!(record.title.as_str(), "Renamed");
    assert_eq!(f.markers(&key, SystemMarker::Review).await, 1);
}

#[tokio::test]
async fn a_later_run_of_the_worktree_replaces_the_review_that_still_waits() {
    let f = fixture().await;
    f.review_on().await;
    let (key, run) = f.finished_with_tests().await;
    f.dispatcher.pass.run().await;
    let ticket = f.record(&key).await;
    let entry =
        nullradix_app::worktrees::WorktreeRepository::of_ticket(f.store.as_ref(), ticket.id)
            .await
            .unwrap()
            .unwrap();
    let later = nullradix_domain::Review::pending(
        Id::from_uuid(uuid::Uuid::now_v7()),
        entry.worktree.id(),
        None,
        String::from("ticket/later"),
        Timestamp::from_millis(START + 5),
    );
    nullradix_app::review::ReviewRepository::hold(f.store.as_ref(), &later, &[])
        .await
        .unwrap();
    let latest =
        nullradix_app::review::ReviewRepository::latest(f.store.as_ref(), entry.worktree.id())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(latest.review.id(), later.id());
    assert!(latest.tests.is_empty());
    // the replaced review is gone, so its run has none
    assert!(
        !nullradix_app::review::ReviewRepository::held(f.store.as_ref(), run)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn the_review_view_says_why_the_changes_cannot_be_shown() {
    let f = fixture().await;
    f.review_on().await;
    let idle = f.ready("Not started", &[]).await;
    let view = f.reviews.view(&idle).await.unwrap();
    assert!(!view.available);
    assert_eq!(view.reason.as_deref(), Some("the ticket has no branch"));
    let error = f.reviews.patch(&idle, "src/a.ts").await.unwrap_err();
    assert_eq!(error.code(), "review-not-available");
    let error = f.reviews.file(&idle, "src/a.ts").await.unwrap_err();
    assert_eq!(error.code(), "review-not-available");
    let missing = f.reviews.view("ARC-0099").await.unwrap_err();
    assert_eq!(missing.code(), "ticket-not-found");

    let (key, _) = f.finished_with_tests().await;
    f.dispatcher.pass.run().await;
    *f.machine.changes.lock().unwrap() = None;
    let view = f.reviews.view(&key).await.unwrap();
    assert!(!view.available);
    assert!(
        view.reason
            .as_deref()
            .is_some_and(|reason| reason.contains("cannot be computed"))
    );
    // the tests stay visible while git cannot say what changed
    assert_eq!(view.tests.len(), 2);
    assert_eq!(
        f.reviews.patch(&key, "src/a.ts").await.unwrap_err().code(),
        "review-file-unknown"
    );
}

#[tokio::test]
async fn paths_that_leave_the_repository_never_reach_the_runner() {
    let f = fixture().await;
    f.review_on().await;
    let (key, _) = f.finished_with_tests().await;
    f.dispatcher.pass.run().await;
    f.machine.files.lock().unwrap().insert(
        String::from("../secret"),
        BranchFile {
            size: 1,
            binary: false,
            text: Some(String::from("x")),
            truncated: false,
        },
    );
    for path in ["../secret", "/etc/passwd", "a//b", ""] {
        assert_eq!(
            f.reviews.file(&key, path).await.unwrap_err().code(),
            "review-file-unknown",
            "{path:?}"
        );
        assert_eq!(
            f.reviews.patch(&key, path).await.unwrap_err().code(),
            "review-file-unknown",
            "{path:?}"
        );
    }
}

#[tokio::test]
async fn the_patch_and_the_file_of_a_held_ticket_come_from_the_runner() {
    let f = fixture().await;
    f.review_on().await;
    let (key, _) = f.finished_with_tests().await;
    f.dispatcher.pass.run().await;
    f.machine.patches.lock().unwrap().insert(
        String::from("src/a.ts"),
        FilePatch {
            patch: String::from("@@ -1 +1 @@\n-a\n+b\n"),
            truncated: false,
        },
    );
    f.machine.files.lock().unwrap().insert(
        String::from("src/a.ts"),
        BranchFile {
            size: 2,
            binary: false,
            text: Some(String::from("b\n")),
            truncated: false,
        },
    );
    let patch = f.reviews.patch(&key, "src/a.ts").await.unwrap();
    assert!(patch.patch.contains("+b"));
    let file = f.reviews.file(&key, "src/a.ts").await.unwrap();
    assert_eq!(file.text.as_deref(), Some("b\n"));
}

#[tokio::test]
async fn an_autopilot_question_is_researched_first_and_answered_with_the_recommendation() {
    let f = fixture().await;
    let (ticket, key) = f.autopilot_question().await;
    // neither the switch of the research automation nor the daily cap are needed
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.research, [key.clone()], "{report:?}");
    f.submit_research(&key, AGENT_Q1, true).await;
    f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
    f.dispatcher.pass.run().await;
    let detail = f.questions.get(&key).await.unwrap();
    let record = detail.view.row.question.record();
    assert_eq!(record.state, nullradix_domain::QuestionState::Answered);
    let decision = record.decision.as_ref().unwrap();
    assert!(decision.by_autopilot());
    assert_eq!(
        decision.reason.as_deref(),
        Some("Autopilot: recommendation of the research adopted")
    );
    let notes = f
        .comments
        .list(&ticket)
        .await
        .unwrap()
        .iter()
        .filter(|line| line.comment.body().starts_with("Autopilot: question"))
        .count();
    assert_eq!(notes, 1);
}

#[tokio::test]
async fn without_a_recommendation_the_autopilot_tries_larger_and_then_blocks_the_ticket() {
    let f = fixture().await;
    let (ticket, key) = f.autopilot_question().await;
    f.advance(1);
    f.dispatcher.pass.run().await;
    let first = f.research_runs(&key).await.remove(0);
    f.submit_research(&key, AGENT_Q1, false).await;
    f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
    // the pass that settles the first research starts the second on a higher profile
    f.advance(1);
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.research, [key.clone()], "{report:?}");
    let runs = f.research_runs(&key).await;
    assert_eq!(runs.len(), 2);
    let second = runs.iter().find(|run| run.id() != first.id()).unwrap();
    let low = f.profile_of(first.id()).await.name;
    let high = f.profile_of(second.id()).await.name;
    assert_ne!(low, high);
    f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
    f.dispatcher.pass.run().await;
    let record = f.record(&ticket).await;
    assert_eq!(record.status, TicketStatus::Blocked);
    assert_eq!(
        record.blocked_reason.as_deref(),
        Some("Autopilot: no recommendation for ARC-Q0001")
    );
    // no third research
    f.advance(1);
    assert!(f.dispatcher.pass.run().await.research.is_empty());
    assert_eq!(f.research_runs(&key).await.len(), 2);
}

impl Fixture {
    async fn autopilot_tries(&self, key: &str) -> u32 {
        use nullradix_app::dispatch::tracking::{AutoRunRepository, TrackKind};
        let detail = self.questions.get(key).await.unwrap();
        let id = detail.view.row.question.record().id.as_uuid();
        AutoRunRepository::get(self.store.as_ref(), TrackKind::AutopilotResearch, id)
            .await
            .unwrap()
            .map_or(0, |row| row.count)
    }

    /// A ticket held for its review after one pass.
    async fn held_for_review(&self) -> String {
        self.review_on().await;
        let (key, _) = self.finished_with_tests().await;
        let report = self.dispatcher.pass.run().await;
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!(self.record(&key).await.status, TicketStatus::Blocked);
        key
    }

    async fn review_state(&self, key: &str) -> Option<nullradix_domain::ReviewState> {
        nullradix_app::review::ReviewRepository::state_of(self.store.as_ref(), key)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn a_limit_in_an_autopilot_research_counts_no_try_and_blocks_nothing() {
    let f = fixture().await;
    let (ticket, key) = f.autopilot_question().await;
    f.advance(1);
    f.dispatcher.pass.run().await;
    let run = f
        .end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::PauseLimit));
    assert_eq!(f.autopilot_tries(&key).await, 0);
    assert_ne!(f.record(&ticket).await.status, TicketStatus::Blocked);
}

#[tokio::test]
async fn an_autopilot_research_is_counted_once_however_often_it_is_settled() {
    let f = fixture().await;
    let (_, key) = f.autopilot_question().await;
    f.advance(1);
    f.dispatcher.pass.run().await;
    f.submit_research(&key, AGENT_Q1, false).await;
    f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.autopilot_tries(&key).await, 1);
    // a pass that lost its work after the count settles the same run again
    let run = f.research_runs(&key).await.remove(0);
    let config = f.config.get().await.unwrap().config;
    let pauses = nullradix_app::dispatch::pause::NoPauseHook;
    f.dispatcher
        .research
        .settle(&run, &config, &pauses)
        .await
        .unwrap();
    assert_eq!(f.autopilot_tries(&key).await, 1);
}

#[tokio::test]
async fn an_autopilot_that_gives_up_tells_the_person_it_is_stuck() {
    let f = fixture().await;
    let (ticket, key) = f.autopilot_question().await;
    for _ in 0..2 {
        f.advance(1);
        f.dispatcher.pass.run().await;
        f.submit_research(&key, AGENT_Q1, false).await;
        f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
        f.advance(1);
        f.dispatcher.pass.run().await;
    }
    assert_eq!(f.record(&ticket).await.status, TicketStatus::Blocked);
    assert!(f.notice_kinds().contains(&NoticeKind::Stuck));
}

#[tokio::test]
async fn questions_of_people_are_not_autopilot_questions() {
    let f = fixture().await;
    f.configure(|config| config.autopilot.enabled = true).await;
    let ticket = f.ready("Build", &["auto"]).await;
    let input = NewQuestion {
        title: String::from("Asked by a person"),
        tickets: vec![ticket],
        ..NewQuestion::default()
    };
    f.questions
        .create(f.board.project(), &input, Author::default(), &f.person)
        .await
        .unwrap();
    assert!(f.dispatcher.pass.run().await.research.is_empty());
}

#[tokio::test]
async fn accepting_a_held_ticket_decides_its_review_and_a_repeat_changes_nothing() {
    use nullradix_domain::ReviewState;
    let f = fixture().await;
    let key = f.held_for_review().await;
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Pending));
    let view = f
        .dispatcher
        .decisions
        .accept(&key, &f.person, None, true)
        .await
        .unwrap();
    assert_eq!(view.ticket.record().status, TicketStatus::Done);
    assert_eq!(view.ticket.record().blocked_reason, None);
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Approved));
    // a stale version after the accept is no reason to fail: the review is decided
    let again = f
        .dispatcher
        .decisions
        .accept(&key, &f.person, Some(Version::INITIAL), true)
        .await
        .unwrap();
    assert_eq!(again.ticket.record().version, view.ticket.record().version);
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Approved));
    // a return after the accept finds nothing to return
    let error = f
        .dispatcher
        .decisions
        .return_to_agent(&key, "more", &f.person, None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "return-not-possible");
}

#[tokio::test]
async fn an_accept_of_a_done_ticket_with_a_lost_decision_decides_it_now() {
    use nullradix_domain::ReviewState;
    let f = fixture().await;
    let key = f.held_for_review().await;
    // the ticket was accepted but the decision never reached the review
    f.worktrees
        .accept(&key, &f.person, None, true)
        .await
        .unwrap();
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Pending));
    f.dispatcher
        .decisions
        .accept(&key, &f.person, None, true)
        .await
        .unwrap();
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Approved));
}

#[tokio::test]
async fn an_accept_of_a_ticket_that_waits_for_nothing_is_refused() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    let error = f
        .dispatcher
        .decisions
        .accept(&key, &f.person, None, true)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "accept-not-possible");
}

#[tokio::test]
async fn returning_a_held_ticket_notes_it_and_the_next_run_gets_the_feedback() {
    use nullradix_domain::ReviewState;
    let f = fixture().await;
    let key = f.held_for_review().await;
    let view = f
        .dispatcher
        .decisions
        .return_to_agent(&key, "  Please test\nthe error case.  ", &f.person, None)
        .await
        .unwrap();
    let record = view.ticket.record();
    assert_eq!(record.status, TicketStatus::Planned);
    assert_eq!(
        (record.blocked_reason.clone(), record.assignee.clone()),
        (None, None)
    );
    assert!(record.branch.is_some() && record.worktree.is_some());
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Returned));
    let comments = f.comments.list(&key).await.unwrap();
    let noted: Vec<_> = comments
        .iter()
        .filter(|line| line.comment.record().system_marker == Some(SystemMarker::ReviewReturned))
        .collect();
    assert_eq!(noted.len(), 1);
    assert_eq!(
        noted[0].comment.body(),
        "[dispatch] review-returned (by anna): Please test\nthe error case."
    );
    assert_eq!(noted[0].author_kind, ActorKind::Dispatcher);
    // a repeat with the same feedback changes nothing, other words are refused
    let again = f
        .dispatcher
        .decisions
        .return_to_agent(&key, "Please test\nthe error case.", &f.person, None)
        .await
        .unwrap();
    assert_eq!(again.ticket.record().version, record.version);
    let other = f
        .dispatcher
        .decisions
        .return_to_agent(&key, "other words", &f.person, None)
        .await
        .unwrap_err();
    assert_eq!(other.code(), "return-not-possible");
    let stale = f
        .dispatcher
        .decisions
        .return_to_agent(
            &key,
            "Please test\nthe error case.",
            &f.person,
            Some(Version::INITIAL),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    assert_eq!(f.markers(&key, SystemMarker::ReviewReturned).await, 1);
    // the next pass starts the agent again with the feedback in its instruction
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [key.clone()], "{report:?}");
    let spec = f.machine.specs.lock().unwrap().last().cloned().unwrap();
    let prompt = spec.stdin.unwrap();
    assert!(
        prompt.contains("- Please test\n  the error case."),
        "{prompt}"
    );
    assert!(!prompt.contains("other words"));
    // once the agent started, the feedback is spent
    let spent = f.prompts.work_prompt(&key, "ticket/x").await.unwrap();
    assert!(!spent.contains("Please test"), "{spent}");
}

impl Fixture {
    /// Ticket `guest` works in the worktree of ticket `host` from now on.
    async fn share_worktree(&self, host: &str, guest: &str) {
        use nullradix_app::worktrees::WorktreeRepository;
        let host = self.record(host).await;
        let guest = self.record(guest).await;
        let entry = WorktreeRepository::of_ticket(self.store.as_ref(), host.id)
            .await
            .unwrap()
            .unwrap();
        let mut write = nullradix_app::tickets::TicketRepository::begin(self.store.as_ref())
            .await
            .unwrap();
        let now = Timestamp::from_millis(START);
        write.release_link(guest.id, now).await.unwrap();
        write
            .join_worktree(entry.worktree.id(), guest.id, now)
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    /// The agent of the ticket finishes; the next pass holds it for review.
    async fn finish_for_review(&self, key: &str) {
        self.git_says(vec![changed("src/a.ts", "M", 1, 1)]);
        self.agent_changes(key, "done", None).await;
        self.finish_run(key).await;
        let report = self.dispatcher.pass.run().await;
        assert!(report.errors.is_empty(), "{report:?}");
    }
}

#[tokio::test]
async fn a_ticket_blocked_for_something_else_leaves_the_review_of_a_worktree_mate_alone() {
    use nullradix_domain::ReviewState;
    let f = fixture().await;
    f.review_on().await;
    let keys = f.start_tickets(2).await;
    f.share_worktree(&keys[0], &keys[1]).await;
    f.finish_for_review(&keys[1]).await;
    assert_eq!(f.review_state(&keys[1]).await, Some(ReviewState::Pending));
    f.agent_changes(&keys[0], "blocked", Some("Human Interaction: look at it"))
        .await;
    let view = f
        .dispatcher
        .decisions
        .accept(&keys[0], &f.person, None, true)
        .await
        .unwrap();
    assert_eq!(view.ticket.record().status, TicketStatus::Done);
    assert_eq!(f.review_state(&keys[0]).await, None);
    assert_eq!(f.review_state(&keys[1]).await, Some(ReviewState::Pending));
}

#[tokio::test]
async fn a_repeated_accept_leaves_the_new_review_of_a_worktree_mate_alone() {
    use nullradix_domain::ReviewState;
    let f = fixture().await;
    f.review_on().await;
    let keys = f.start_tickets(2).await;
    f.share_worktree(&keys[0], &keys[1]).await;
    f.finish_for_review(&keys[0]).await;
    f.dispatcher
        .decisions
        .accept(&keys[0], &f.person, None, true)
        .await
        .unwrap();
    assert_eq!(f.review_state(&keys[0]).await, Some(ReviewState::Approved));
    f.finish_for_review(&keys[1]).await;
    assert_eq!(f.review_state(&keys[1]).await, Some(ReviewState::Pending));
    f.dispatcher
        .decisions
        .accept(&keys[0], &f.person, None, true)
        .await
        .unwrap();
    assert_eq!(f.review_state(&keys[0]).await, Some(ReviewState::Approved));
    assert_eq!(f.review_state(&keys[1]).await, Some(ReviewState::Pending));
}

struct FlakyHook {
    calls: std::sync::atomic::AtomicUsize,
    fail: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl nullradix_app::dispatch::review_decide::AcceptHook for FlakyHook {
    async fn accepted(&self, _ticket: &nullradix_domain::TicketRecord) -> Result<(), AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(AppError::conflict("hook-failed", "the merge failed"));
        }
        Ok(())
    }
}

#[tokio::test]
async fn an_accept_retries_the_hook_that_failed_after_the_decision() {
    use nullradix_app::dispatch::review_decide::{DecisionParts, ReviewDecisions};
    use nullradix_domain::ReviewState;
    let f = fixture().await;
    let key = f.held_for_review().await;
    let hook = Arc::new(FlakyHook {
        calls: std::sync::atomic::AtomicUsize::new(0),
        fail: std::sync::atomic::AtomicBool::new(true),
    });
    let decisions = ReviewDecisions::new(DecisionParts {
        pass: f.dispatcher.pass.clone(),
        tickets: f.tickets.clone(),
        comments: f.comments.clone(),
        worktrees: f.worktrees.clone(),
        reviews: f.reviews.clone(),
        runs: f.runs.clone(),
        starter: f.dispatcher.starter.clone(),
    })
    .with_hook(hook.clone());
    let error = decisions
        .accept(&key, &f.person, None, true)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "hook-failed");
    assert_eq!(f.record(&key).await.status, TicketStatus::Done);
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Approved));
    decisions.accept(&key, &f.person, None, true).await.unwrap();
    assert_eq!(hook.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_repeated_return_needs_the_same_feedback_since_the_last_start() {
    let f = fixture().await;
    let key = f.held_for_review().await;
    let decisions = &f.dispatcher.decisions;
    decisions
        .return_to_agent(&key, "Add a test", &f.person, None)
        .await
        .unwrap();
    // the agent starts and a person plans the ticket again
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [key.clone()], "{report:?}");
    f.agent_changes(&key, "planned", None).await;
    let error = decisions
        .return_to_agent(&key, "Add a test", &f.person, None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "return-not-possible");
}

#[tokio::test]
async fn accepting_after_a_person_edited_the_blocked_reason_decides_its_own_review() {
    use nullradix_domain::ReviewState;
    let f = fixture().await;
    let key = f.held_for_review().await;
    f.agent_changes(
        &key,
        "blocked",
        Some("Human Interaction: look at this first"),
    )
    .await;
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Pending));
    let view = f
        .dispatcher
        .decisions
        .accept(&key, &f.person, None, true)
        .await
        .unwrap();
    assert_eq!(view.ticket.record().status, TicketStatus::Done);
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Approved));
}

#[tokio::test]
async fn a_repeated_return_decides_the_review_whose_decision_was_lost() {
    use nullradix_domain::ReviewState;
    let f = fixture().await;
    let key = f.held_for_review().await;
    let record = f.record(&key).await;
    let author = f.dispatcher.starter.actor().await.unwrap();
    // the ticket went back to planned with the comment, but the decision was lost
    f.comments
        .add_system(
            &record.key,
            &nullradix_domain::returned_comment("anna", "Add a test"),
            Some(SystemMarker::ReviewReturned),
            None,
            author,
        )
        .await
        .unwrap();
    f.agent_changes(&key, "planned", None).await;
    assert_eq!(f.record(&key).await.status, TicketStatus::Planned);
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Pending));
    f.dispatcher
        .decisions
        .return_to_agent(&key, "Add a test", &f.person, None)
        .await
        .unwrap();
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Returned));
    assert_eq!(f.markers(&key, SystemMarker::ReviewReturned).await, 1);
}

#[tokio::test]
async fn a_return_that_failed_after_its_comment_notes_the_feedback_once() {
    let f = fixture().await;
    let key = f.held_for_review().await;
    let record = f.record(&key).await;
    let author = f.dispatcher.starter.actor().await.unwrap();
    // what a first try left behind before its move to planned failed
    f.comments
        .add_system(
            &record.key,
            &nullradix_domain::returned_comment("anna", "Add a test"),
            Some(SystemMarker::ReviewReturned),
            None,
            author,
        )
        .await
        .unwrap();
    let view = f
        .dispatcher
        .decisions
        .return_to_agent(&key, "Add a test", &f.person, None)
        .await
        .unwrap();
    assert_eq!(view.ticket.record().status, TicketStatus::Planned);
    assert_eq!(f.markers(&key, SystemMarker::ReviewReturned).await, 1);
}

#[tokio::test]
async fn a_return_is_refused_without_feedback_over_the_cap_or_when_nothing_waits() {
    let f = fixture().await;
    let key = f.held_for_review().await;
    let decisions = &f.dispatcher.decisions;
    for (text, code) in [
        (String::from("  \n "), "feedback-empty"),
        ("x".repeat(10_001), "value-too-long"),
    ] {
        let error = decisions
            .return_to_agent(&key, &text, &f.person, None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), code);
    }
    let stale = decisions
        .return_to_agent(&key, "x", &f.person, Some(Version::INITIAL))
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    assert_eq!(f.markers(&key, SystemMarker::ReviewReturned).await, 0);
    assert_eq!(f.record(&key).await.status, TicketStatus::Blocked);
    let other = f.ready("Not held", &[]).await;
    let error = decisions
        .return_to_agent(&other, "x", &f.person, None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "return-not-possible");
    // exactly at the cap goes through
    decisions
        .return_to_agent(&key, &"x".repeat(10_000), &f.person, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_decision_waits_for_the_pass_that_is_under_way() {
    use nullradix_domain::ReviewState;
    let f = fixture().await;
    let key = f.held_for_review().await;
    f.configure(|config| config.wip = 3).await;
    f.ready("Second", &["auto"]).await;
    // the pass stops while it starts the second ticket
    let gate = Arc::new(Notify::new());
    *f.machine.hold_start.lock().unwrap() = Some(gate.clone());
    let pass = {
        let pass = f.dispatcher.pass.clone();
        tokio::spawn(async move { pass.run().await })
    };
    f.machine.start_entered.notified().await;
    let decisions = f.dispatcher.decisions.clone();
    let person = f.person.clone();
    let name = key.clone();
    let accept = tokio::spawn(async move { decisions.accept(&name, &person, None, true).await });
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(!accept.is_finished(), "the accept ran inside the pass");
    assert_eq!(f.record(&key).await.status, TicketStatus::Blocked);
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Pending));
    gate.notify_one();
    pass.await.unwrap();
    accept.await.unwrap().unwrap();
    assert_eq!(f.record(&key).await.status, TicketStatus::Done);
    assert_eq!(f.review_state(&key).await, Some(ReviewState::Approved));
}

impl Fixture {
    /// A ticket the agent finished and whose run ended, with no review in the way.
    async fn finished_ticket(&self) -> String {
        self.enable().await;
        let key = self.start_tickets(1).await.remove(0);
        self.agent_changes(&key, "done", None).await;
        self.finish_run(&key).await;
        key
    }

    async fn jobs_of(&self, key: &str) -> Vec<nullradix_app::dispatch::merge_jobs::StoredMergeJob> {
        let id = self.tickets.get(key).await.unwrap().view.ticket.id();
        self.dispatcher.merges.of_ticket(id).await.unwrap()
    }

    async fn bodies(&self, key: &str) -> Vec<String> {
        self.comments
            .list(key)
            .await
            .unwrap()
            .iter()
            .map(|line| line.comment.body().to_owned())
            .collect()
    }
}

const AGENT_P1: &str = "planner-ARC-0001";

impl Fixture {
    /// A backlog ticket that needs planning.
    async fn unplanned(&self, title: &str, priority: Option<&str>) -> String {
        let input = NewTicket {
            title: title.to_owned(),
            priority: priority.map(str::to_owned),
            ..NewTicket::default()
        };
        self.tickets
            .create(
                self.board.project(),
                self.board.id(),
                &input,
                &self.person,
                FORCE,
            )
            .await
            .unwrap()
            .ticket
            .key()
            .to_string()
    }

    async fn plan_runs(&self, key: &str) -> Vec<Run> {
        let filter = RunFilter {
            subject_key: Some(key.to_owned()),
            kind: Some(nullradix_domain::RunKind::Plan),
            ..RunFilter::default()
        };
        nullradix_app::runs::RunRepository::find(self.store.as_ref(), &filter, 20)
            .await
            .unwrap()
    }

    async fn plan_error(&self, key: &str) -> &'static str {
        self.dispatcher
            .service
            .start_planning(key)
            .await
            .unwrap_err()
            .code()
    }

    /// The planning agent saves its plan with `plan import`.
    async fn write_plan(&self, key: &str, agent: &str) {
        let author = Author {
            by_person: false,
            name: Some(agent),
            session: Some("s-plan"),
        };
        let actor = self.comments.author(author, &self.person).await.unwrap();
        let input = nullradix_app::tickets::PlanInput {
            plan: Some(String::from("Do it in two steps.")),
            handbook: Some(String::from("cargo test")),
            ..nullradix_app::tickets::PlanInput::default()
        };
        self.tickets.plan(key, &input, &actor).await.unwrap();
    }

    async fn plan_tracked(&self, key: &str) -> Option<nullradix_app::dispatch::tracking::Tracked> {
        use nullradix_app::dispatch::tracking::{AutoRunRepository, TrackKind};
        let id = self.record(key).await.id.as_uuid();
        AutoRunRepository::get(self.store.as_ref(), TrackKind::Planning, id)
            .await
            .unwrap()
    }

    async fn auto_planning(&self, wip: i64) {
        self.configure(|config| {
            config.planning.auto = true;
            config.planning.wip = wip;
        })
        .await;
    }
}

#[tokio::test]
async fn a_finished_ticket_without_review_is_merged_and_cleaned_up_in_the_same_pass() {
    let f = fixture().await;
    let key = f.finished_ticket().await;
    let before = f.record(&key).await;
    let branch = before.branch.clone().unwrap();
    let report = f.dispatcher.pass.run().await;
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(report.merged, vec![key.clone()]);
    let requests = f.git.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].branch, branch);
    assert_eq!(requests[0].target, None);
    assert_eq!(
        requests[0].message,
        format!("Merge {key} \"Ticket 0\" ({branch})")
    );
    // the branch goes by compare-and-swap at the tip that was merged
    let deleted = f.git.deleted.lock().unwrap().clone();
    assert_eq!(deleted.len(), 1);
    assert_eq!(
        (deleted[0].tip.as_str(), deleted[0].into.as_str()),
        ("tip0001", "main")
    );
    let after = f.record(&key).await;
    assert_eq!((after.branch, after.status), (None, TicketStatus::Done));
    let jobs = f.jobs_of(&key).await;
    assert_eq!(jobs.len(), 1);
    let job = jobs[0].job.record();
    assert_eq!(job.phase, nullradix_domain::MergePhase::Merged);
    assert_eq!(job.merged_commit.as_deref(), Some("abc1234"));
    assert_eq!(jobs[0].branch, branch);
    let note = f.bodies(&key).await.pop().unwrap();
    assert!(
        note.starts_with("[dispatch] merged into main (abc1234). Branch ticket/"),
        "{note}"
    );
    assert_eq!(f.markers(&key, SystemMarker::Merged).await, 1);
    assert!(
        f.notices
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|notice| notice.kind == NoticeKind::Merged)
    );
    // nothing is left to do on the next pass
    let comments = f.comment_count(&key).await;
    let again = f.dispatcher.pass.run().await;
    assert!(
        again.merged.is_empty() && again.errors.is_empty(),
        "{again:?}"
    );
    assert_eq!(f.comment_count(&key).await, comments);
    assert_eq!(f.git.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn the_ticket_target_and_the_squash_strategy_reach_the_merge() {
    let f = fixture().await;
    f.configure(|config| config.merge_strategy = nullradix_domain::MergeStrategy::Squash)
        .await;
    f.enable().await;
    let key = f.ready("Targeted", &["auto"]).await;
    f.tickets
        .update(
            &key,
            &TicketPatch {
                target_branch: Some(Some(String::from("release/1"))),
                ..TicketPatch::default()
            },
            &f.person,
            f.record(&key).await.version,
            FORCE,
        )
        .await
        .unwrap();
    f.dispatcher.pass.run().await;
    f.agent_changes(&key, "done", None).await;
    f.finish_run(&key).await;
    *f.git.created.lock().unwrap() = Some(String::from("origin/release/1"));
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.merged, vec![key.clone()], "{report:?}");
    let request = f.git.requests.lock().unwrap().remove(0);
    assert_eq!(request.target.as_deref(), Some("release/1"));
    assert_eq!(request.strategy, nullradix_domain::MergeStrategy::Squash);
    assert!(
        request
            .message
            .ends_with("(squash of ticket/ARC-0001-targeted)"),
        "{}",
        request.message
    );
    let created = f.bodies(&key).await.into_iter().any(|body| {
        body == "[dispatch] merge: target branch release/1 created from origin/release/1."
    });
    assert!(created);
    assert_eq!(
        f.jobs_of(&key).await[0].job.record().strategy,
        nullradix_domain::MergeStrategy::Squash
    );
}

#[tokio::test]
async fn a_ticket_held_for_review_queues_its_merge_when_a_person_accepts() {
    let f = fixture().await;
    let key = f.held_for_review().await;
    assert!(f.jobs_of(&key).await.is_empty());
    let report = f.dispatcher.pass.run().await;
    assert!(report.merged.is_empty());
    f.dispatcher
        .decisions
        .accept(&key, &f.person, None, true)
        .await
        .unwrap();
    let queued = f.jobs_of(&key).await;
    assert_eq!(queued.len(), 1);
    assert_eq!(
        queued[0].job.record().phase,
        nullradix_domain::MergePhase::Pending
    );
    // a repeat of the accept queues nothing twice
    f.dispatcher
        .decisions
        .accept(&key, &f.person, None, true)
        .await
        .unwrap();
    assert_eq!(f.jobs_of(&key).await.len(), 1);
    let merged = f.dispatcher.pass.run().await;
    assert_eq!(merged.merged, vec![key.clone()], "{merged:?}");
    assert_eq!(f.record(&key).await.branch, None);
}

#[tokio::test]
async fn a_conflict_stops_the_job_with_a_state_a_merge_session_can_pick_up() {
    let f = fixture().await;
    let key = f.finished_ticket().await;
    f.git
        .merges
        .lock()
        .unwrap()
        .push_back(MergeOutcome::Conflict {
            files: vec![String::from("a.rs"), String::from("b.rs")],
        });
    let report = f.dispatcher.pass.run().await;
    assert!(
        report.merged.is_empty() && report.errors.is_empty(),
        "{report:?}"
    );
    let jobs = f.jobs_of(&key).await;
    let job = jobs[0].job.record();
    assert_eq!(job.phase, nullradix_domain::MergePhase::Waiting);
    assert!(job.conflict_seen);
    assert_eq!(job.problem.as_deref(), Some("merge conflict in a.rs, b.rs"));
    // the ticket keeps its branch and stays done; nothing is blocked yet
    let record = f.record(&key).await;
    assert_eq!(record.status, TicketStatus::Done);
    assert!(record.branch.is_some());
    assert_eq!(f.markers(&key, SystemMarker::MergeConflict).await, 1);
    assert!(f.git.deleted.lock().unwrap().is_empty());
    // later passes leave the conflict alone instead of merging and undoing it again
    f.dispatcher.pass.run().await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.git.requests.lock().unwrap().len(), 1);
    assert_eq!(f.markers(&key, SystemMarker::MergeConflict).await, 1);
}

#[tokio::test]
async fn a_refused_merge_blocks_the_tickets_with_a_merge_reason() {
    let f = fixture().await;
    let key = f.finished_ticket().await;
    f.git
        .merges
        .lock()
        .unwrap()
        .push_back(MergeOutcome::Refused {
            reason: String::from("changes are staged\nin the checkout"),
        });
    let report = f.dispatcher.pass.run().await;
    assert!(
        report.merged.is_empty() && report.errors.is_empty(),
        "{report:?}"
    );
    let record = f.record(&key).await;
    assert_eq!(record.status, TicketStatus::Blocked);
    assert_eq!(
        record.blocked_reason.as_deref(),
        Some("Merge: changes are staged in the checkout")
    );
    assert!(record.branch.is_some());
    let jobs = f.jobs_of(&key).await;
    assert_eq!(
        jobs[0].job.record().phase,
        nullradix_domain::MergePhase::Failed
    );
    assert_eq!(f.markers(&key, SystemMarker::Merge).await, 1);
    assert!(
        f.notices
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|notice| notice.kind == NoticeKind::Blocked)
    );
    // a failed job is not processed again
    f.dispatcher.pass.run().await;
    assert_eq!(f.git.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_branch_that_cannot_be_deleted_keeps_the_links_and_says_so() {
    let f = fixture().await;
    let key = f.finished_ticket().await;
    *f.git.deletion.lock().unwrap() = Some(BranchDeletion::Kept(String::from(
        "branch b stays: it has new commits since the merge",
    )));
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.merged, vec![key.clone()], "{report:?}");
    assert!(f.record(&key).await.branch.is_some());
    let note = f.bodies(&key).await.pop().unwrap();
    assert!(
        note.contains("Cleanup incomplete:\nbranch b stays"),
        "{note}"
    );
    assert_eq!(
        f.jobs_of(&key).await[0].job.record().phase,
        nullradix_domain::MergePhase::Merged
    );
}

#[tokio::test]
async fn a_branch_that_is_gone_but_merged_earlier_is_finished_without_merging() {
    let f = fixture().await;
    let key = f.finished_ticket().await;
    f.git.gone.store(true, Ordering::SeqCst);
    *f.git.earlier.lock().unwrap() = Some(nullradix_app::dispatch::merge_git::MergedEarlier {
        into: String::from("main"),
        commit: String::from("1234abc"),
    });
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.merged, vec![key.clone()], "{report:?}");
    assert!(f.git.requests.lock().unwrap().is_empty());
    assert!(f.git.deleted.lock().unwrap().is_empty());
    assert_eq!(f.record(&key).await.branch, None);
    assert_eq!(
        f.jobs_of(&key).await[0]
            .job
            .record()
            .merged_commit
            .as_deref(),
        Some("1234abc")
    );
}

#[tokio::test]
async fn a_merge_blocked_ticket_is_done_again_once_its_branch_merged() {
    let f = fixture().await;
    let key = f.finished_ticket().await;
    f.git
        .merges
        .lock()
        .unwrap()
        .push_back(MergeOutcome::Refused {
            reason: String::from("no"),
        });
    f.dispatcher.pass.run().await;
    assert_eq!(f.record(&key).await.status, TicketStatus::Blocked);
    // a person fixes the checkout and moves the ticket back to done, then it is queued again
    f.agent_changes(&key, "done", None).await;
    let record = f.record(&key).await;
    assert!(f.dispatcher.merges.queue(&record).await.unwrap());
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.merged, vec![key.clone()], "{report:?}");
    let after = f.record(&key).await;
    assert_eq!(
        (after.status, after.blocked_reason),
        (TicketStatus::Done, None)
    );
    assert_eq!(f.jobs_of(&key).await.len(), 2);
}

#[tokio::test]
async fn a_ticket_that_is_open_holds_the_merge_back_and_says_so_once() {
    let f = fixture().await;
    f.enable().await;
    let first = f.ready("First", &["auto"]).await;
    let second = f.ready("Second", &["auto"]).await;
    f.worktrees
        .start(&first, f.local, ROOT, StartMode::New, false, &f.person)
        .await
        .unwrap();
    f.worktrees
        .start(
            &second,
            f.local,
            ROOT,
            StartMode::JoinTicket(first.clone()),
            false,
            &f.person,
        )
        .await
        .unwrap();
    f.agent_changes(&first, "done", None).await;
    let record = f.record(&first).await;
    let branch = record.branch.clone().unwrap();
    assert!(f.dispatcher.merges.queue(&record).await.unwrap());
    let report = f.dispatcher.pass.run().await;
    assert!(
        report.merged.is_empty() && report.errors.is_empty(),
        "{report:?}"
    );
    let jobs = f.jobs_of(&first).await;
    assert_eq!(
        jobs[0].job.record().phase,
        nullradix_domain::MergePhase::Waiting
    );
    let waiting = f.bodies(&first).await.pop().unwrap();
    assert_eq!(
        waiting,
        format!("[dispatch] merge-waiting: waiting for {second} before {branch} is merged.")
    );
    f.dispatcher.pass.run().await;
    assert_eq!(f.markers(&first, SystemMarker::MergeWait).await, 1);
    assert!(f.git.requests.lock().unwrap().is_empty());
    // the second ticket finishes: one merge for the branch, named after both tickets
    f.agent_changes(&second, "done", None).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.merged.len(), 2, "{report:?}");
    let requests = f.git.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].message,
        format!("Merge {first}, {second} ({branch})")
    );
    assert_eq!(f.record(&second).await.branch, None);
}

#[tokio::test]
async fn a_pass_that_overlaps_an_accept_queues_one_job() {
    let f = fixture().await;
    let key = f.finished_ticket().await;
    let record = f.record(&key).await;
    let (first, second) = tokio::join!(
        f.dispatcher.merges.queue(&record),
        f.dispatcher.merges.queue(&record)
    );
    assert_eq!(u8::from(first.unwrap()) + u8::from(second.unwrap()), 1);
    assert_eq!(f.jobs_of(&key).await.len(), 1);
}

#[tokio::test]
async fn catch_up_queues_a_finished_autopilot_ticket_nobody_queued() {
    let f = fixture().await;
    f.configure(|config| config.autopilot.enabled = true).await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.agent_changes(&key, "done", None).await;
    let run = f.finish_run(&key).await;
    // the run is settled elsewhere, so the done gate never saw it
    f.runs
        .settle_outcome(run, DispatchOutcome::Done)
        .await
        .unwrap();
    assert!(f.jobs_of(&key).await.is_empty());
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.merged, vec![key.clone()], "{report:?}");
    assert_eq!(f.jobs_of(&key).await.len(), 1);
    // a branch that is in the target already is left alone
    let second = f.ready("Second", &["auto"]).await;
    f.dispatcher.pass.run().await;
    f.agent_changes(&second, "done", None).await;
    let run = f.finish_run(&second).await;
    f.runs
        .settle_outcome(run, DispatchOutcome::Done)
        .await
        .unwrap();
    f.git.integrated.store(true, Ordering::SeqCst);
    let again = f.dispatcher.pass.run().await;
    assert!(again.merged.is_empty(), "{again:?}");
    assert!(f.jobs_of(&second).await.is_empty());
}

#[tokio::test]
async fn catch_up_asks_git_nothing_while_the_branch_has_an_open_job() {
    let f = fixture().await;
    f.configure(|config| config.autopilot.enabled = true).await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.agent_changes(&key, "done", None).await;
    let run = f.finish_run(&key).await;
    f.runs
        .settle_outcome(run, DispatchOutcome::Done)
        .await
        .unwrap();
    // the conflict leaves the job open and standing still
    f.git
        .merges
        .lock()
        .unwrap()
        .push_back(MergeOutcome::Conflict {
            files: vec![String::from("a.txt")],
        });
    f.dispatcher.pass.run().await;
    assert_eq!(f.jobs_of(&key).await.len(), 1);
    f.git.probes.store(0, Ordering::SeqCst);
    let report = f.dispatcher.pass.run().await;
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(f.git.probes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_ticket_reopened_during_the_merge_keeps_its_branch_and_status() {
    let f = fixture().await;
    f.enable().await;
    let first = f.ready("First", &["auto"]).await;
    let second = f.ready("Second", &["auto"]).await;
    f.worktrees
        .start(&first, f.local, ROOT, StartMode::New, false, &f.person)
        .await
        .unwrap();
    f.worktrees
        .start(
            &second,
            f.local,
            ROOT,
            StartMode::JoinTicket(first.clone()),
            false,
            &f.person,
        )
        .await
        .unwrap();
    f.agent_changes(&second, "blocked", Some("Merge: an earlier problem"))
        .await;
    f.agent_changes(&first, "done", None).await;
    let record = f.record(&first).await;
    assert!(f.dispatcher.merges.queue(&record).await.unwrap());
    // a person moves the blocked ticket back to work while git merges
    let gate = Arc::new(Notify::new());
    *f.git.hold_merge.lock().unwrap() = Some(gate.clone());
    let pass = {
        let pass = f.dispatcher.pass.clone();
        tokio::spawn(async move { pass.run().await })
    };
    f.git.merge_entered.notified().await;
    f.agent_changes(&second, TicketStatus::InProgress.as_str(), None)
        .await;
    gate.notify_one();
    let report = pass.await.unwrap();
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(f.record(&first).await.branch, None);
    let reopened = f.record(&second).await;
    assert_eq!(reopened.status, TicketStatus::InProgress);
    assert!(reopened.branch.is_some());
}

#[tokio::test]
async fn catch_up_does_nothing_with_autopilot_off() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    f.agent_changes(&key, "done", None).await;
    let run = f.finish_run(&key).await;
    f.runs
        .settle_outcome(run, DispatchOutcome::Done)
        .await
        .unwrap();
    let report = f.dispatcher.pass.run().await;
    assert!(report.merged.is_empty(), "{report:?}");
    assert!(f.jobs_of(&key).await.is_empty());
}

#[tokio::test]
async fn a_folder_that_cannot_be_cleaned_up_blocks_and_any_other_failure_is_tried_again() {
    let f = fixture().await;
    let key = f.finished_ticket().await;
    *f.git.fail_merge.lock().unwrap() = Some(AppError::unavailable("runner-failed", "boom"));
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.errors.len(), 1, "{report:?}");
    assert_eq!(f.record(&key).await.status, TicketStatus::Done);
    assert_eq!(
        f.jobs_of(&key).await[0].job.record().phase,
        nullradix_domain::MergePhase::Pending
    );
    *f.git.fail_merge.lock().unwrap() = Some(AppError::invalid(
        "merge-abort-failed",
        "the folder is still in the merge state",
    ));
    let report = f.dispatcher.pass.run().await;
    assert!(report.errors.is_empty(), "{report:?}");
    let record = f.record(&key).await;
    assert_eq!(record.status, TicketStatus::Blocked);
    assert_eq!(
        record.blocked_reason.as_deref(),
        Some("Merge: the folder is still in the merge state")
    );
}

#[tokio::test]
async fn a_person_starts_the_planning_of_a_ticket_in_a_detached_worktree() {
    let f = fixture().await;
    let key = f.unplanned("Plan me", None).await;
    assert_eq!(key, "ARC-0001");
    // by hand it runs whatever the switch says, and it takes no place of the ticket limit
    let run = f.dispatcher.service.start_planning("arc-1").await.unwrap();
    let dispatch = run.dispatch().unwrap();
    assert_eq!(dispatch.kind, nullradix_domain::RunKind::Plan);
    assert_eq!(dispatch.trigger, nullradix_domain::RunTrigger::Manual);
    assert_eq!(dispatch.subject_key.as_deref(), Some("ARC-0001"));
    assert_eq!(dispatch.agent.as_deref(), Some(AGENT_P1));
    let folder = format!("{ROOT}-worktrees/plan-ARC-0001");
    assert_eq!(dispatch.cwd.replace('\\', "/"), folder);
    let created = f.detached.created.lock().unwrap().clone();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].root, ROOT);
    assert_eq!(created[0].at, "HEAD");
    assert_eq!(created[0].purpose, DetachedPurpose::Plan);
    let spec = f.machine.specs.lock().unwrap()[0].clone();
    let prompt = spec.stdin.unwrap();
    assert!(prompt.contains("ARC-0001") && prompt.contains("Plan me"));
    assert!(prompt.contains(&format!(
        "plan import ARC-0001 --file plan.json --author {AGENT_P1}"
    )));
    let at = |flag: &str| spec.args.iter().position(|arg| arg == flag).unwrap();
    assert!(at("--disallowedTools") < at("--allowedTools"));
    for tool in [
        "Write(./plan.json)",
        "Bash(nullradix plan import:*)",
        "Bash(nullradix question list:*)",
    ] {
        assert!(spec.args.iter().any(|arg| arg == tool), "{tool}");
    }
    assert!(!spec.args.iter().any(|arg| arg == "Bash(*)"));
    // the ticket is claimed for the agent while the run goes
    let claim = f.record(&key).await.planning_claim.unwrap();
    assert_eq!(claim.0, AGENT_P1);
    let tracked = f.plan_tracked(&key).await.unwrap();
    assert_eq!(tracked.count, 1);
    assert_eq!(tracked.auto_at, None);
    let status = f.dispatcher.service.status(None).await.unwrap();
    assert_eq!(status.active, 0);
    assert_eq!(status.planning, 1);
}

#[tokio::test]
async fn a_planning_is_refused_for_the_reasons_legacy_gave() {
    let f = fixture().await;
    let key = f.unplanned("Plan me", None).await;
    assert_eq!(f.plan_error("ARC-0099").await, "ticket-not-found");
    assert_eq!(f.plan_error("ARC-Q1").await, "ticket-id-invalid");
    // a ticket that is done
    let done = f.unplanned("Done already", None).await;
    let version = f.record(&done).await.version;
    let patch = TicketPatch {
        status: Some(String::from("done")),
        acceptance: Some(String::from("Seen.")),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&done, &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    assert_eq!(f.plan_error(&done).await, "plan-not-possible");
    // a ticket that waits on a question
    let asked = f.unplanned("Asks", None).await;
    let input = NewQuestion {
        title: String::from("Which?"),
        tickets: vec![asked.clone()],
        ..NewQuestion::default()
    };
    f.questions
        .create(f.board.project(), &input, Author::default(), &f.person)
        .await
        .unwrap();
    assert_eq!(f.plan_error(&asked).await, "plan-waiting-question");
    // another agent holds the planning claim
    let claimed = f.unplanned("Claimed", None).await;
    f.tickets
        .claim_planning(&claimed, "someone-else", &f.person)
        .await
        .unwrap();
    assert_eq!(f.plan_error(&claimed).await, "plan-claimed");
    assert!(f.plan_runs(&claimed).await.is_empty());
    // a folder in the way stays, the start fails and the claim goes back
    f.detached.busy.store(true, Ordering::SeqCst);
    assert_eq!(f.plan_error(&key).await, "plan-worktree-busy");
    f.detached.busy.store(false, Ordering::SeqCst);
    assert_eq!(f.record(&key).await.planning_claim, None);
    assert!(f.machine.specs.lock().unwrap().is_empty());
    // a pause holds it back
    f.dispatcher
        .service
        .pause(nullradix_app::dispatch::control::PauseFor::Minutes(30))
        .await
        .unwrap();
    assert_eq!(f.plan_error(&key).await, "dispatch-paused");
    f.dispatcher.service.resume().await.unwrap();
    // a second start of a ticket that is being planned
    f.dispatcher.service.start_planning(&key).await.unwrap();
    assert_eq!(f.plan_error(&key).await, "plan-running");
    assert_eq!(f.plan_runs(&key).await.len(), 1);
}

#[tokio::test]
async fn a_ticket_that_is_worked_on_or_on_a_board_without_a_repository_is_refused() {
    let f = fixture().await;
    let key = f.unplanned("Worked on", None).await;
    let version = f.record(&key).await.version;
    let patch = TicketPatch {
        status: Some(String::from("in-progress")),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&key, &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    assert_eq!(f.plan_error(&key).await, "plan-not-possible");
    // a board of another project has no repository
    let bare = f.bare_board_ticket().await;
    assert_eq!(f.plan_error(&bare).await, "plan-no-repo");
    assert!(f.claim_of(&bare).await.is_none());
}

#[tokio::test]
async fn an_ended_unsettled_work_run_does_not_hold_back_planning() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    // a live work run of the ticket blocks planning
    let version = f.record(&key).await.version;
    let patch = TicketPatch {
        status: Some(String::from("planned")),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&key, &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    assert_eq!(f.plan_error(&key).await, "plan-not-possible");
    // the same run ended but not yet settled does not
    f.finish_run(&key).await;
    f.dispatcher.service.start_planning(&key).await.unwrap();
    assert_eq!(f.plan_runs(&key).await.len(), 1);
}

#[tokio::test]
async fn a_failed_planning_start_removes_its_worktree_and_claim_again() {
    let f = fixture().await;
    let key = f.unplanned("Plan me", None).await;
    f.machine.fail_start.store(true, Ordering::SeqCst);
    assert_eq!(f.plan_error(&key).await, "claude-missing");
    let removed = f.detached.removed.lock().unwrap().clone();
    assert_eq!(removed, [format!("{ROOT}-worktrees/plan-ARC-0001")]);
    assert!(f.detached.folders.lock().unwrap().is_empty());
    assert!(f.claim_of(&key).await.is_none());
    assert_eq!(f.plan_tracked(&key).await, None);
}

#[tokio::test]
async fn two_starts_of_one_ticket_make_one_planning_run() {
    let f = fixture().await;
    let key = f.unplanned("Plan me", None).await;
    let gate = Arc::new(Notify::new());
    *f.machine.hold_start.lock().unwrap() = Some(gate.clone());
    let first = {
        let service = f.dispatcher.service.clone();
        let key = key.clone();
        tokio::spawn(async move { service.start_planning(&key).await })
    };
    f.machine.start_entered.notified().await;
    let second = {
        let service = f.dispatcher.service.clone();
        let key = key.clone();
        tokio::spawn(async move { service.start_planning(&key).await })
    };
    tokio::task::yield_now().await;
    gate.notify_one();
    let first = first.await.unwrap();
    let second = second.await.unwrap();
    assert!(first.is_ok(), "{first:?}");
    assert_eq!(second.unwrap_err().code(), "plan-running");
    assert_eq!(f.plan_runs(&key).await.len(), 1);
    assert_eq!(f.detached.created.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_saved_plan_settles_the_run_and_removes_the_worktree() {
    let f = fixture().await;
    let key = f.unplanned("Plan me", None).await;
    f.dispatcher.service.start_planning(&key).await.unwrap();
    f.write_plan(&key, AGENT_P1).await;
    let run = f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(
        f.outcome(run).await,
        Some(DispatchOutcome::Planned),
        "{report:?}"
    );
    assert!(f.detached.folders.lock().unwrap().is_empty());
    let record = f.record(&key).await;
    assert_eq!(record.status, TicketStatus::Planned);
    assert!(!record.needs_planning);
    assert_eq!(record.planning_claim, None);
    assert_eq!(record.plan, "Do it in two steps.");
    assert!(f.marked(&key, SystemMarker::PlanEmpty).await.is_empty());
}

#[tokio::test]
async fn a_run_that_leaves_no_plan_comments_the_ticket_once_and_gives_the_claim_back() {
    let f = fixture().await;
    let key = f.unplanned("Plan me", None).await;
    f.dispatcher.service.start_planning(&key).await.unwrap();
    // a plan of someone else in the meantime is not the agent's
    f.write_plan(&key, "someone-else").await;
    let abort = abort_of(AbortCategory::Timeout, TIMEOUT_REASON);
    let run = f
        .end_run(&key, RunOutcome::TimedOut, None, Some(abort))
        .await;
    f.dispatcher.pass.run().await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Other));
    let bodies = f.marked(&key, SystemMarker::PlanEmpty).await;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(
        bodies[0].starts_with("[dispatch] plan-empty: time limit of 90 min reached, run stopped."),
        "{bodies:?}"
    );
    assert!(bodies[0].contains("\"Plan\" on the ticket starts a new one"));
    let comments = f.comments.list(&key).await.unwrap();
    let line = comments
        .iter()
        .find(|line| line.comment.record().system_marker == Some(SystemMarker::PlanEmpty))
        .unwrap();
    assert_eq!(line.comment.record().run, Some(run));
    assert_eq!(line.author_kind, ActorKind::Dispatcher);
    assert!(f.claim_of(&key).await.is_none());
    assert!(f.detached.folders.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_planning_that_asks_a_question_settles_as_a_question_without_a_comment() {
    let f = fixture().await;
    let key = f.unplanned("Plan me", None).await;
    f.dispatcher.service.start_planning(&key).await.unwrap();
    let input = NewQuestion {
        title: String::from("Which database?"),
        tickets: vec![key.clone()],
        ..NewQuestion::default()
    };
    f.questions
        .create(f.board.project(), &input, Author::default(), &f.person)
        .await
        .unwrap();
    let run = f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Question));
    assert!(f.marked(&key, SystemMarker::PlanEmpty).await.is_empty());
    assert!(f.claim_of(&key).await.is_none());
    assert!(f.detached.folders.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_usage_limit_in_a_planning_pauses_the_dispatcher_and_leaves_the_ticket_to_plan_again() {
    let f = fixture().await;
    let key = f.unplanned("Plan me", None).await;
    f.dispatcher.service.start_planning(&key).await.unwrap();
    let run = f
        .end_run(&key, RunOutcome::Failed, Some(1), Some(limit_abort()))
        .await;
    f.dispatcher.pass.run().await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::PauseLimit));
    assert_eq!(
        f.paused_until().await.map(Timestamp::as_millis),
        Some(START + 60 * MINUTE)
    );
    let bodies = f.marked(&key, SystemMarker::PlanEmpty).await;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(
        bodies[0].starts_with("[dispatch] plan-empty: usage limit reached, plan not saved."),
        "{bodies:?}"
    );
    assert!(bodies[0].contains("Start the planning again afterwards."));
    assert_eq!(f.notice_kinds(), [NoticeKind::Paused]);
    assert!(f.detached.folders.lock().unwrap().is_empty());
    assert_eq!(f.plan_runs(&key).await.len(), 1);
}

#[tokio::test]
async fn missing_claude_in_a_planning_pauses_the_dispatcher() {
    let f = fixture().await;
    let key = f.unplanned("Plan me", None).await;
    f.dispatcher.service.start_planning(&key).await.unwrap();
    let missing = abort_of(
        AbortCategory::StartFailed,
        "claude could not be started: not found",
    );
    let run = f
        .end_run(&key, RunOutcome::Failed, Some(127), Some(missing))
        .await;
    f.dispatcher.pass.run().await;
    assert_eq!(
        f.outcome(run).await,
        Some(DispatchOutcome::PauseClaudeMissing)
    );
    let bodies = f.marked(&key, SystemMarker::PlanEmpty).await;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(
        bodies[0].contains("Claude Code was not found (command \"claude\")"),
        "{bodies:?}"
    );
    assert_eq!(f.notice_kinds(), [NoticeKind::Paused]);
}

#[tokio::test]
async fn a_plan_worktree_that_stays_is_removed_by_a_later_pass_but_not_while_its_run_lives() {
    let f = fixture().await;
    let key = f.unplanned("Plan me", None).await;
    f.dispatcher.service.start_planning(&key).await.unwrap();
    let folder = format!("{ROOT}-worktrees/plan-ARC-0001");
    f.dispatcher.pass.run().await;
    assert!(f.detached.folders.lock().unwrap().contains(&folder));
    f.detached.keep.store(true, Ordering::SeqCst);
    let run = f.end_run(&key, RunOutcome::Succeeded, Some(0), None).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, Some(DispatchOutcome::Other));
    assert!(f.detached.folders.lock().unwrap().contains(&folder));
    f.detached.keep.store(false, Ordering::SeqCst);
    f.dispatcher.pass.run().await;
    assert!(f.detached.folders.lock().unwrap().is_empty());
    assert!(f.dispatcher.service.start_planning(&key).await.is_ok());
}

#[tokio::test]
async fn a_research_folder_is_no_plan_orphan_and_a_plan_folder_no_research_orphan() {
    let f = fixture().await;
    let research = format!("{ROOT}-worktrees/research-ARC-Q0001");
    let plan = format!("{ROOT}-worktrees/plan-ARC-0009");
    f.detached.folders.lock().unwrap().insert(research.clone());
    f.detached.folders.lock().unwrap().insert(plan.clone());
    f.dispatcher.pass.run().await;
    assert!(f.detached.folders.lock().unwrap().is_empty());
    let mut removed = f.detached.removed.lock().unwrap().clone();
    removed.sort();
    assert_eq!(removed, [plan, research]);
}

#[tokio::test]
async fn the_planning_automation_takes_up_only_tickets_newer_than_its_switch() {
    let f = fixture().await;
    let old = f.unplanned("Old", None).await;
    f.advance(1);
    f.auto_planning(5).await;
    let report = f.dispatcher.pass.run().await;
    assert!(report.planned.is_empty(), "{report:?}");
    f.advance(1);
    let fresh = f.unplanned("Fresh", None).await;
    // the dispatcher is off: the automation does not need the switch
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.planned, [fresh.clone()], "{report:?}");
    assert!(f.plan_runs(&old).await.is_empty());
    let tracked = f.plan_tracked(&fresh).await.unwrap();
    assert_eq!(tracked.count, 1);
    assert!(tracked.auto_at.is_some());
    // under way or tried: no second start, not even after a run without a plan
    assert!(f.dispatcher.pass.run().await.planned.is_empty());
    f.end_run(&fresh, RunOutcome::Succeeded, Some(0), None)
        .await;
    f.dispatcher.pass.run().await;
    assert!(f.dispatcher.pass.run().await.planned.is_empty());
    assert_eq!(f.plan_runs(&fresh).await.len(), 1);
    // a changed old ticket counts as new
    f.advance(1);
    let version = f.record(&old).await.version;
    let patch = TicketPatch {
        description: Some(String::from("Now with a description.")),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&old, &patch, &f.person, version, FORCE)
        .await
        .unwrap();
    assert_eq!(f.dispatcher.pass.run().await.planned, [old.clone()]);
    // switched off and on again: the clock starts anew
    f.end_run(&old, RunOutcome::Succeeded, Some(0), None).await;
    f.dispatcher.pass.run().await;
    f.configure(|config| config.planning.auto = false).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    f.auto_planning(5).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    let again = f.unplanned("Again", None).await;
    assert_eq!(f.dispatcher.pass.run().await.planned, [again]);
}

#[tokio::test]
async fn the_planning_automation_goes_by_priority_then_number_within_its_places() {
    let f = fixture().await;
    f.advance(1);
    f.auto_planning(1).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    let low = f.unplanned("Low", Some("low")).await;
    let normal = f.unplanned("Normal", None).await;
    let urgent = f.unplanned("Urgent", Some("urgent")).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.planned, [urgent.clone()], "{report:?}");
    // one place: nothing more while it runs
    assert!(f.dispatcher.pass.run().await.planned.is_empty());
    f.end_run(&urgent, RunOutcome::Succeeded, Some(0), None)
        .await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.planned, [normal.clone()], "{report:?}");
    f.end_run(&normal, RunOutcome::Succeeded, Some(0), None)
        .await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.planned, [low], "{report:?}");
}

#[tokio::test]
async fn the_planning_automation_plans_only_while_the_ticket_limit_has_room() {
    let f = fixture().await;
    f.advance(1);
    f.auto_planning(3).await;
    f.configure(|config| {
        config.enabled = true;
        config.review_before_merge = false;
        config.wip = 1;
    })
    .await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    let work = f.ready("Work", &["auto"]).await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.started, [work.clone()], "{report:?}");
    f.advance(1);
    let key = f.unplanned("Plan me", None).await;
    // the only place of the limit is taken by the work run
    let report = f.dispatcher.pass.run().await;
    assert!(report.planned.is_empty(), "{report:?}");
    f.end_run(&work, RunOutcome::Succeeded, Some(0), None).await;
    f.agent_changes(&work, "done", None).await;
    // the pass settles the work run and plans in the place it freed
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.planned, [key], "{report:?}");
    // the plan run takes no place of the limit
    let status = f.dispatcher.service.status(None).await.unwrap();
    assert_eq!(status.active, 0);
    assert_eq!(status.planning, 1);
}

#[tokio::test]
async fn the_planning_automation_marks_ticket_faults_and_waits_after_machine_faults() {
    let f = fixture().await;
    f.advance(1);
    f.auto_planning(5).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    // a live claim of another agent keeps a ticket out of the candidates
    let claimed = f.unplanned("Claimed", None).await;
    f.tickets
        .claim_planning(&claimed, "someone-else", &f.person)
        .await
        .unwrap();
    let report = f.dispatcher.pass.run().await;
    assert!(
        report.planned.is_empty() && report.errors.is_empty(),
        "{report:?}"
    );
    let key = f.unplanned("Broken", None).await;
    f.detached.fail_create.store(true, Ordering::SeqCst);
    let report = f.dispatcher.pass.run().await;
    assert!(report.planned.is_empty());
    assert_eq!(report.errors.len(), 1, "{report:?}");
    assert!(report.errors[0].contains("git is broken"), "{report:?}");
    // within the pause nothing is tried, so nothing is reported again
    f.detached.fail_create.store(false, Ordering::SeqCst);
    f.advance(29);
    let report = f.dispatcher.pass.run().await;
    assert!(
        report.planned.is_empty() && report.errors.is_empty(),
        "{report:?}"
    );
    // the claim of the other agent has run out by now, and its ticket is a candidate again
    f.advance(2);
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.planned, [claimed, key], "{report:?}");
}

#[tokio::test]
async fn the_planning_automation_stands_down_while_paused_or_outside_the_window() {
    let f = fixture().await;
    f.advance(1);
    f.auto_planning(5).await;
    f.dispatcher.pass.run().await;
    f.advance(1);
    let key = f.unplanned("Later", None).await;
    f.dispatcher
        .service
        .pause(nullradix_app::dispatch::control::PauseFor::Minutes(15))
        .await
        .unwrap();
    assert!(f.dispatcher.pass.run().await.planned.is_empty());
    f.dispatcher.service.resume().await.unwrap();
    f.configure(|config| {
        config.window = Some(nullradix_domain::TimeWindow {
            from: String::from("13:00"),
            to: String::from("14:00"),
        });
    })
    .await;
    assert!(f.dispatcher.pass.run().await.planned.is_empty());
    f.configure(|config| config.window = None).await;
    assert_eq!(f.dispatcher.pass.run().await.planned, [key]);
}

#[tokio::test]
async fn a_ticket_being_planned_is_not_started_for_work() {
    let f = fixture().await;
    f.enable().await;
    let key = f.ready("Replan me", &["auto"]).await;
    f.dispatcher.service.start_planning(&key).await.unwrap();
    let report = f.dispatcher.pass.run().await;
    assert!(report.started.is_empty(), "{report:?}");
    assert_eq!(f.plan_runs(&key).await.len(), 1);
}

impl Fixture {
    async fn claim_of(&self, key: &str) -> Option<(String, Timestamp)> {
        self.record(key).await.planning_claim
    }
}

impl Fixture {
    /// A ticket on a board that has no repository.
    async fn bare_board_ticket(&self) -> String {
        let clock = self.clock.clone();
        let ids = Arc::new(UuidV7Generator);
        let signal = ChangeSignal::new();
        let runners = Arc::new(
            RunnerService::start(
                self.store.clone(),
                self.store.clone(),
                self.machine.clone(),
                clock.clone(),
                ids.clone(),
                signal.clone(),
            )
            .await
            .unwrap(),
        );
        let projects = ProjectService::new(
            self.store.clone(),
            clock.clone(),
            ids.clone(),
            signal.clone(),
        );
        let boards = BoardService::new(
            self.store.clone(),
            self.store.clone(),
            self.store.clone(),
            runners,
            clock,
            ids,
            signal,
        );
        let project = projects.create("OTH", "Others").await.unwrap();
        let board = boards
            .create(project.id(), "Bare", &self.person)
            .await
            .unwrap()
            .board;
        let input = NewTicket {
            title: String::from("No repository"),
            ..NewTicket::default()
        };
        self.tickets
            .create(project.id(), board.id(), &input, &self.person, FORCE)
            .await
            .unwrap()
            .ticket
            .key()
            .to_string()
    }
}

impl Fixture {
    /// A finished ticket whose first merge the checkout refused.
    async fn refused_ticket(&self) -> String {
        let key = self.finished_ticket().await;
        self.git
            .merges
            .lock()
            .unwrap()
            .push_back(MergeOutcome::Refused {
                reason: String::from("changes are staged"),
            });
        let report = self.dispatcher.pass.run().await;
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!(self.record(&key).await.status, TicketStatus::Blocked);
        key
    }
}

#[tokio::test]
async fn a_retry_unblocks_the_ticket_and_merges_the_branch() {
    use nullradix_app::dispatch::merge_retry::RetryOutcome;
    let f = fixture().await;
    let key = f.refused_ticket().await;
    assert!(
        f.notices.0.lock().unwrap().iter().any(|notice| {
            notice.kind == NoticeKind::Blocked && notice.text.contains("Merge: changes are staged")
        }),
        "the refusal is announced with its problem"
    );
    let retried = f
        .dispatcher
        .retries
        .retry(&key, &f.person, None)
        .await
        .unwrap();
    assert_eq!(retried.outcome, RetryOutcome::Merged, "{}", retried.text);
    assert_eq!(retried.text, format!("{key} merged into main (abc1234)."));
    let after = f.record(&key).await;
    assert_eq!(
        (after.status, after.blocked_reason, after.branch),
        (TicketStatus::Done, None, None)
    );
    let jobs = f.jobs_of(&key).await;
    let phases: Vec<_> = jobs.iter().map(|job| job.job.record().phase).collect();
    assert_eq!(
        phases,
        [
            nullradix_domain::MergePhase::Merged,
            nullradix_domain::MergePhase::Failed
        ]
    );
    let bodies = f.bodies(&key).await;
    assert!(
        bodies
            .iter()
            .any(|body| body.starts_with("[dispatch] merge: retry started by "))
    );
    assert_eq!(f.git.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_retry_forgets_the_conflict_and_merges_again() {
    let f = fixture().await;
    let key = f.finished_ticket().await;
    f.git
        .merges
        .lock()
        .unwrap()
        .push_back(MergeOutcome::Conflict {
            files: vec![String::from("a.rs")],
        });
    f.dispatcher.pass.run().await;
    assert!(f.jobs_of(&key).await[0].job.record().conflict_seen);
    let retried = f
        .dispatcher
        .retries
        .retry(&key, &f.person, None)
        .await
        .unwrap();
    assert_eq!(retried.text, format!("{key} merged into main (abc1234)."));
    // the conflicted job itself was reset, not replaced
    let jobs = f.jobs_of(&key).await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        jobs[0].job.record().phase,
        nullradix_domain::MergePhase::Merged
    );
    assert!(!jobs[0].job.record().conflict_seen);
}

#[tokio::test]
async fn a_retry_of_a_branch_the_target_holds_finishes_without_merging() {
    let f = fixture().await;
    let key = f.refused_ticket().await;
    f.git.gone.store(true, Ordering::SeqCst);
    *f.git.earlier.lock().unwrap() = Some(nullradix_app::dispatch::merge_git::MergedEarlier {
        into: String::from("main"),
        commit: String::from("1234abc"),
    });
    let retried = f
        .dispatcher
        .retries
        .retry(&key, &f.person, None)
        .await
        .unwrap();
    assert_eq!(retried.text, format!("{key} merged into main (1234abc)."));
    assert_eq!(f.git.requests.lock().unwrap().len(), 1);
    let after = f.record(&key).await;
    assert_eq!((after.status, after.branch), (TicketStatus::Done, None));
}

#[tokio::test]
async fn a_retry_that_is_repeated_queues_nothing_twice_but_tries_the_step() {
    use nullradix_app::dispatch::merge_retry::RetryOutcome;
    let f = fixture().await;
    let key = f.refused_ticket().await;
    *f.git.fail_merge.lock().unwrap() = Some(AppError::conflict("git-failed", "locked"));
    let first = f
        .dispatcher
        .retries
        .retry(&key, &f.person, None)
        .await
        .unwrap();
    assert_eq!(first.outcome, RetryOutcome::Pending);
    assert!(first.text.contains("merge not run"), "{}", first.text);
    assert_eq!(f.record(&key).await.status, TicketStatus::Done);
    let comments = f.comment_count(&key).await;
    let versions: Vec<_> = f
        .jobs_of(&key)
        .await
        .iter()
        .map(|job| job.job.record().version)
        .collect();
    // the version the caller saw is stale by now, a repeat still answers with the last error
    *f.git.fail_merge.lock().unwrap() = Some(AppError::conflict("git-failed", "locked"));
    let again = f
        .dispatcher
        .retries
        .retry(&key, &f.person, Some(Version::INITIAL))
        .await
        .unwrap();
    assert_eq!(again.outcome, RetryOutcome::Pending);
    assert!(again.text.contains("locked"), "{}", again.text);
    assert_eq!(f.comment_count(&key).await, comments);
    let after: Vec<_> = f
        .jobs_of(&key)
        .await
        .iter()
        .map(|job| job.job.record().version)
        .collect();
    assert_eq!(after, versions);
    assert_eq!(f.git.requests.lock().unwrap().len(), 3);
    // with nothing in the way the repeat merges it
    let merged = f
        .dispatcher
        .retries
        .retry(&key, &f.person, None)
        .await
        .unwrap();
    assert_eq!(merged.outcome, RetryOutcome::Merged, "{}", merged.text);
    assert_eq!(f.jobs_of(&key).await.len(), 2);
}

#[tokio::test]
async fn a_retry_is_refused_for_a_stale_version_and_for_a_ticket_that_waits_for_no_merge() {
    let f = fixture().await;
    let key = f.refused_ticket().await;
    let stale = f
        .dispatcher
        .retries
        .retry(&key, &f.person, Some(Version::INITIAL))
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    assert_eq!(f.record(&key).await.status, TicketStatus::Blocked);
    assert_eq!(f.jobs_of(&key).await.len(), 1);
    let other = f.ready("Not started", &[]).await;
    let error = f
        .dispatcher
        .retries
        .retry(&other, &f.person, None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "merge-not-possible");
    // blocked for something else is no merge problem
    f.agent_changes(&key, "blocked", Some("no database access"))
        .await;
    let error = f
        .dispatcher
        .retries
        .retry(&key, &f.person, None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "merge-not-possible");
}

#[tokio::test]
async fn a_person_who_sets_the_ticket_done_again_queues_no_merge() {
    let f = fixture().await;
    let key = f.refused_ticket().await;
    f.agent_changes(&key, "done", None).await;
    let after = f.record(&key).await;
    assert_eq!(
        (after.status, after.blocked_reason.clone()),
        (TicketStatus::Done, None)
    );
    assert!(after.branch.is_some());
    f.dispatcher.pass.run().await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.git.requests.lock().unwrap().len(), 1);
    assert_eq!(f.jobs_of(&key).await.len(), 1);
    // the way back into the merge is the retry
    let retried = f
        .dispatcher
        .retries
        .retry(&key, &f.person, None)
        .await
        .unwrap();
    assert_eq!(retried.text, format!("{key} merged into main (abc1234)."));
}

#[tokio::test]
async fn a_retry_waits_for_the_pass_that_is_under_way() {
    let f = fixture().await;
    let key = f.finished_ticket().await;
    f.git
        .merges
        .lock()
        .unwrap()
        .push_back(MergeOutcome::Refused {
            reason: String::from("changes are staged"),
        });
    // the pass stops inside the merge of the first job
    let gate = Arc::new(Notify::new());
    *f.git.hold_merge.lock().unwrap() = Some(gate.clone());
    let pass = {
        let pass = f.dispatcher.pass.clone();
        tokio::spawn(async move { pass.run().await })
    };
    f.git.merge_entered.notified().await;
    let service = f.dispatcher.retries.clone();
    let person = f.person.clone();
    let name = key.clone();
    let retry = tokio::spawn(async move { service.retry(&name, &person, None).await });
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(!retry.is_finished(), "the retry ran inside the pass");
    gate.notify_one();
    pass.await.unwrap();
    // the pass blocked the ticket first, then the retry saw it blocked and merged it
    let retried = retry.await.unwrap().unwrap();
    assert_eq!(retried.text, format!("{key} merged into main (abc1234)."));
    assert_eq!(f.record(&key).await.status, TicketStatus::Done);
    assert_eq!(f.git.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_caller_that_goes_away_does_not_cancel_the_merge() {
    let f = fixture().await;
    let key = f.refused_ticket().await;
    let gate = Arc::new(Notify::new());
    *f.git.hold_merge.lock().unwrap() = Some(gate.clone());
    let service = f.dispatcher.retries.clone();
    let person = f.person.clone();
    let name = key.clone();
    let retry = tokio::spawn(async move { service.retry(&name, &person, None).await });
    f.git.merge_entered.notified().await;
    // the caller gives up while git is merging
    retry.abort();
    let _ = retry.await;
    gate.notify_one();
    f.dispatcher.pass.exclusive(|| async {}).await;
    let after = f.record(&key).await;
    assert_eq!((after.status, after.branch), (TicketStatus::Done, None));
    let phases: Vec<_> = f
        .jobs_of(&key)
        .await
        .iter()
        .map(|job| job.job.record().phase)
        .collect();
    assert_eq!(
        phases,
        [
            nullradix_domain::MergePhase::Merged,
            nullradix_domain::MergePhase::Failed
        ]
    );
}

#[tokio::test]
async fn a_retry_reports_no_error_of_an_unrelated_job() {
    use nullradix_app::dispatch::merge_retry::RetryOutcome;
    let f = fixture().await;
    let key = f.refused_ticket().await;
    let other = f.finished_ticket().await;
    let branch = f.record(&other).await.branch.unwrap();
    *f.git.fail_branch.lock().unwrap() = Some(branch);
    let report = f.dispatcher.pass.run().await;
    assert!(!report.errors.is_empty(), "{report:?}");
    f.git
        .merges
        .lock()
        .unwrap()
        .push_back(MergeOutcome::Conflict {
            files: vec![String::from("a.rs")],
        });
    let retried = f
        .dispatcher
        .retries
        .retry(&key, &f.person, None)
        .await
        .unwrap();
    assert_eq!(retried.outcome, RetryOutcome::Pending);
    assert!(!retried.text.contains("merge not run"), "{}", retried.text);
    assert!(!retried.text.contains("locked"), "{}", retried.text);
}

#[tokio::test]
async fn a_retry_is_refused_while_a_merge_session_runs_for_a_sibling() {
    let f = fixture().await;
    f.configure(|config| {
        config.wip = 3;
        config.join_worktrees = nullradix_domain::JoinWorktrees::SameSpec;
    })
    .await;
    let key = f.refused_ticket().await;
    let sibling = f.ready("Same folder", &["auto"]).await;
    f.worktrees
        .start(
            &sibling,
            f.local,
            ROOT,
            StartMode::AutoJoin,
            false,
            &f.person,
        )
        .await
        .unwrap();
    f.config.sync_hash().await.unwrap();
    let hash = f.config.get().await.unwrap().hash;
    let dispatch = nullradix_domain::DispatchRun {
        kind: nullradix_domain::RunKind::Merge,
        trigger: nullradix_domain::RunTrigger::Dispatcher,
        subject_key: Some(sibling.clone()),
        profile: nullradix_domain::RunProfile {
            id: None,
            name: String::from("medium"),
            model: None,
            effort: None,
            max_turns: 10,
            timeout_minutes: 10,
        },
        account: None,
        config_hash: hash,
        agent: None,
        cwd: String::new(),
        args: Vec::new(),
        prompt: String::new(),
    };
    let spec = nullradix_app::runs::RunSpec {
        program: String::from("claude"),
        args: Vec::new(),
        cwd: std::path::PathBuf::from(ROOT),
        timeout: None,
        env: nullradix_app::runs::RunEnv::default(),
        stdin: None,
        claude: true,
    };
    f.runs
        .start_planned(f.runs.next_id(), "merge session", &spec, Some(dispatch))
        .await
        .unwrap();
    let error = f
        .dispatcher
        .retries
        .retry(&key, &f.person, None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "merge-not-possible");
    assert!(error.message().contains("session"), "{}", error.message());
}

#[tokio::test]
async fn a_failed_step_of_a_retry_reaches_the_dispatcher_log() {
    let f = fixture().await;
    let key = f.refused_ticket().await;
    *f.git.fail_merge.lock().unwrap() = Some(AppError::conflict("git-failed", "index held"));
    f.dispatcher
        .retries
        .retry(&key, &f.person, None)
        .await
        .unwrap();
    let lines = nullradix_app::dispatch::log::DispatcherLogRepository::recent(f.store.as_ref(), 20)
        .await
        .unwrap();
    assert!(
        lines.iter().any(|line| line.message.contains("index held")),
        "{lines:?}"
    );
}

fn held_outcome(kind: nullradix_domain::HoldKind, text: &str) -> MergeOutcome {
    MergeOutcome::Held {
        kind,
        place: String::from("the main checkout"),
        text: text.to_owned(),
    }
}

impl Fixture {
    fn script_holds(&self, holds: Vec<MergeOutcome>) {
        self.git.merges.lock().unwrap().extend(holds);
    }

    fn merge_waiting_notices(&self) -> usize {
        self.notices
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|notice| notice.kind == NoticeKind::MergeWaiting)
            .count()
    }
}

#[tokio::test]
async fn a_held_merge_comments_once_tells_the_person_after_a_quarter_hour_and_goes_on_when_clean() {
    use nullradix_domain::{HoldKind, MergePhase};
    let f = fixture().await;
    let key = f.finished_ticket().await;
    let overlap = "local changes in a.txt that the branch changes too";
    f.script_holds(vec![
        held_outcome(HoldKind::Overlap, overlap),
        held_outcome(HoldKind::Overlap, overlap),
        held_outcome(
            HoldKind::Overlap,
            "local changes in a.txt, b.txt that the branch changes too",
        ),
        held_outcome(HoldKind::Overlap, overlap),
        held_outcome(HoldKind::Overlap, overlap),
        held_outcome(HoldKind::Staged, "staged changes (git add) in c.txt"),
    ]);
    let report = f.dispatcher.pass.run().await;
    assert!(
        report.merged.is_empty() && report.errors.is_empty(),
        "{report:?}"
    );
    let job = f.jobs_of(&key).await.remove(0).job;
    assert_eq!(job.record().phase, MergePhase::Waiting);
    assert_eq!(job.record().held_by.as_deref(), Some("overlap"));
    assert_eq!(
        job.held_text(),
        Some("the main checkout: local changes in a.txt that the branch changes too")
    );
    // nothing is blocked and the branch stays
    let record = f.record(&key).await;
    assert_eq!(record.status, TicketStatus::Done);
    assert!(record.branch.is_some());
    let comment = f.bodies(&key).await.pop().unwrap();
    assert_eq!(
        comment,
        "[dispatch] merge-waiting: waiting for the main checkout: local changes in a.txt that \
         the branch changes too - merged automatically once that is done."
    );
    // the same reason and a different file list say nothing new
    f.dispatcher.pass.run().await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.markers(&key, SystemMarker::MergeWait).await, 1);
    assert_eq!(
        f.jobs_of(&key).await[0].job.held_text(),
        Some("the main checkout: local changes in a.txt, b.txt that the branch changes too")
    );
    // the person is told once, after fifteen minutes
    f.advance(14);
    f.dispatcher.pass.run().await;
    assert_eq!(f.merge_waiting_notices(), 0);
    f.advance(1);
    f.dispatcher.pass.run().await;
    assert_eq!(f.merge_waiting_notices(), 1);
    let notice = f
        .notices
        .0
        .lock()
        .unwrap()
        .iter()
        .find(|notice| notice.kind == NoticeKind::MergeWaiting)
        .cloned()
        .unwrap();
    assert_eq!(notice.ticket.as_deref(), Some(key.as_str()));
    assert_eq!(
        notice.text,
        "merge has waited 15 minutes for the main checkout: local changes in a.txt that the \
         branch changes too"
    );
    f.advance(60);
    f.dispatcher.pass.run().await;
    assert_eq!(f.merge_waiting_notices(), 1);
    // another reason is another comment
    assert_eq!(f.markers(&key, SystemMarker::MergeWait).await, 2);
    assert_eq!(
        f.jobs_of(&key).await[0].job.record().held_by.as_deref(),
        Some("staged")
    );
    // once the folder is clean the merge goes through without a person
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.merged, vec![key.clone()], "{report:?}");
    let job = f.jobs_of(&key).await.remove(0).job;
    assert_eq!(job.record().phase, MergePhase::Merged);
    assert_eq!(job.record().problem, None);
    assert_eq!(f.record(&key).await.branch, None);
    assert_eq!(f.git.requests.lock().unwrap().len(), 7);
}

#[tokio::test]
async fn a_retry_of_a_held_merge_names_the_hold() {
    use nullradix_app::dispatch::merge_retry::RetryOutcome;
    use nullradix_domain::{HoldKind, MergePhase};
    let f = fixture().await;
    let key = f.finished_ticket().await;
    f.script_holds(vec![
        held_outcome(HoldKind::Busy, "a merge is under way"),
        held_outcome(HoldKind::Busy, "a merge is under way"),
    ]);
    f.dispatcher.pass.run().await;
    let job = f.jobs_of(&key).await.remove(0).job;
    assert_eq!(
        job.held_text(),
        Some("the main checkout: a merge is under way")
    );
    // a person retries while the checkout is still busy
    let retried = f
        .dispatcher
        .retries
        .retry(&key, &f.person, None)
        .await
        .unwrap();
    assert_eq!(retried.outcome, RetryOutcome::Waiting, "{}", retried.text);
    assert_eq!(
        retried.text,
        format!(
            "{key}: the merge waits for the main checkout: a merge is under way - it is merged \
             automatically once that is done."
        )
    );
    let jobs = f.jobs_of(&key).await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].job.record().phase, MergePhase::Waiting);
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.merged, vec![key.clone()], "{report:?}");
}

#[tokio::test]
async fn a_hold_is_forgotten_while_a_ticket_of_the_branch_is_open_again() {
    use nullradix_domain::HoldKind;
    let f = fixture().await;
    f.enable().await;
    let first = f.ready("First", &["auto"]).await;
    let second = f.ready("Second", &["auto"]).await;
    f.worktrees
        .start(&first, f.local, ROOT, StartMode::New, false, &f.person)
        .await
        .unwrap();
    f.worktrees
        .start(
            &second,
            f.local,
            ROOT,
            StartMode::JoinTicket(first.clone()),
            false,
            &f.person,
        )
        .await
        .unwrap();
    let merge_blocked = "Merge: an earlier problem";
    f.agent_changes(&second, "blocked", Some(merge_blocked))
        .await;
    f.agent_changes(&first, "done", None).await;
    let record = f.record(&first).await;
    assert!(f.dispatcher.merges.queue(&record).await.unwrap());
    f.script_holds(vec![held_outcome(
        HoldKind::Overlap,
        "local changes in a.txt",
    )]);
    f.dispatcher.pass.run().await;
    assert!(f.jobs_of(&first).await[0].job.held_text().is_some());
    // the second ticket is open again: the job waits for it and the hold text goes away
    f.agent_changes(&second, "planned", None).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.jobs_of(&first).await[0].job.held_text(), None);
    assert_eq!(f.markers(&first, SystemMarker::MergeWait).await, 2);
    // it finishes, the checkout is busy, and the ticket opens once more
    f.agent_changes(&second, "blocked", Some(merge_blocked))
        .await;
    f.script_holds(vec![held_outcome(HoldKind::Busy, "a merge is under way")]);
    f.dispatcher.pass.run().await;
    assert!(f.jobs_of(&first).await[0].job.held_text().is_some());
    f.agent_changes(&second, "planned", None).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.jobs_of(&first).await[0].job.held_text(), None);
    f.agent_changes(&second, "blocked", Some(merge_blocked))
        .await;
    let report = f.dispatcher.pass.run().await;
    assert_eq!(report.merged.len(), 2, "{report:?}");
}
