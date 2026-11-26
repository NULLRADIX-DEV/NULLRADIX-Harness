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
use nullradix_app::dispatch::local_time::{LocalTime, minute_with_offset};
use nullradix_app::dispatch::pause::{PauseHook, PauseKind};
use nullradix_app::dispatch::runs::{DispatchRuns, RunFilesPort};
use nullradix_app::dispatch::setup::{SetupFailure, SetupOutcome, SetupRequest, WorktreeSetupPort};
use nullradix_app::dispatcher_config::DispatcherConfigService;
use nullradix_app::dispatcher_prompts::DispatcherPromptService;
use nullradix_app::projects::ProjectService;
use nullradix_app::questions::{NewQuestion, QuestionService};
use nullradix_app::repositories::{Located, RunnerService, not_a_git_project};
use nullradix_app::runs::{RunFilter, RunProgress, RunService, RunSpec, RunnerPort};
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

struct FakeClock(AtomicI64);

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.0.load(Ordering::SeqCst))
    }
}

#[async_trait]
impl LocalTime for FakeClock {
    async fn minute_of_day(&self, at: Timestamp) -> u32 {
        minute_with_offset(at, 0)
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
        _run: Id<Run>,
        _after: i64,
        _limit: u32,
    ) -> Result<RunProgress, AppError> {
        Ok(RunProgress {
            lines: Vec::new(),
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

struct Fixture {
    hook: Arc<Hooked>,
    clock: Arc<FakeClock>,
    machine: Arc<Machine>,
    setup: Arc<ScriptedSetup>,
    dispatcher: Dispatcher,
    accounts: Arc<AccountService>,
    limits: Arc<FakeLimits>,
    pauses: Arc<InjectingPauses>,
    config: Arc<DispatcherConfigService>,
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
    questions: Arc<QuestionService>,
    worktrees: Arc<WorktreeService>,
    local: Id<Runner>,
    person: Actor,
    board: Board,
    store: Arc<Store>,
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let clock = Arc::new(FakeClock(AtomicI64::new(START)));
    let ids = Arc::new(UuidV7Generator);
    let machine = Arc::new(Machine::default());
    let setup = Arc::new(ScriptedSetup::default());
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
        actors: store.clone(),
        boards: store.clone(),
        runners,
        runs,
        dispatch,
        prompts,
        tickets: tickets.clone(),
        comments: comments.clone(),
        worktrees: worktrees.clone(),
        clock: clock.clone(),
        ids,
        signal,
        local_time: clock.clone(),
        interval: None,
    });
    Fixture {
        hook,
        clock,
        machine,
        setup,
        dispatcher,
        accounts,
        limits,
        pauses,
        config,
        tickets,
        comments,
        questions,
        worktrees,
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
        self.configure(|config| config.enabled = true).await;
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
    // the tickets stay as the agent left them and no comment is added
    assert_eq!(f.record(&keys[0]).await.status, TicketStatus::Done);
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
