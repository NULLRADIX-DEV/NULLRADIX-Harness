#![allow(
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::cloned_ref_to_slice_refs,
    reason = "integration tests fail loudly"
)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nullradix_app::accounts::{AccountFolders, AccountService};
use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::comments::{Author, CommentService};
use nullradix_app::dispatch::assemble::{Dispatcher, DispatcherDeps, assemble};
use nullradix_app::dispatch::local_time::{LocalTime, minute_with_offset};
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
use nullradix_app::worktrees::{
    CreatedWorktree, EnsureWorktree, EnsuredWorktree, NewWorktree, RemoveWorktree, StartMode,
    WorktreeRemoval, WorktreeService,
};
use nullradix_app::{AppError, ChangeSignal, Clock, UuidV7Generator};
use nullradix_domain::{
    Actor, ActorKind, Board, ConfigDir, DispatchOutcome, Id, Run, RunEnd, RunOutcome, Runner,
    SetupState, SystemMarker, TicketStatus, Timestamp, Version, WorktreeSetupStep,
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

/// Setup double that holds every setup until the test lets it go.
#[derive(Default)]
struct ScriptedSetup {
    runs: Mutex<Vec<SetupRequest>>,
    failure: Mutex<Option<SetupFailure>>,
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
        accounts,
        store.clone(),
        store.clone(),
        machine.clone(),
    ));
    let prompts = Arc::new(DispatcherPromptService::new(
        config.clone(),
        tickets.clone(),
        questions.clone(),
    ));
    let dispatcher = assemble(DispatcherDeps {
        config: config.clone(),
        states: store.clone(),
        log: store.clone(),
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
    // the run ended without telling: a cancelled run is no longer active
    let runs = f.store.clone();
    let active = nullradix_app::runs::RunRepository::active(runs.as_ref())
        .await
        .unwrap();
    for run in active {
        let ended = run.lost(f.clock.now()).unwrap();
        nullradix_app::runs::RunRepository::update(runs.as_ref(), &ended, run.version())
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
async fn a_start_that_always_fails_is_not_tried_again_until_the_ticket_changes() {
    let f = fixture().await;
    f.enable().await;
    f.machine.fail_create.store(true, Ordering::SeqCst);
    let key = f.ready("Build", &["auto"]).await;
    let first = f.dispatcher.pass.run().await;
    assert_eq!(first.errors.len(), 1, "{first:?}");
    let after_first = f.record(&key).await;
    assert_eq!(after_first.assignee, None, "the claim was given back");
    let second = f.dispatcher.pass.run().await;
    assert!(second.errors.is_empty(), "{second:?}");
    assert_eq!(
        f.record(&key).await,
        after_first,
        "no claim and release again"
    );
    // an edit of the ticket is a reason to try again
    let patch = TicketPatch {
        plan: Some(String::from("Do it differently.")),
        ..TicketPatch::default()
    };
    f.tickets
        .update(&key, &patch, &f.person, after_first.version, FORCE)
        .await
        .unwrap();
    f.machine.fail_create.store(false, Ordering::SeqCst);
    assert_eq!(f.dispatcher.pass.run().await.started, [key]);
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
        self.configure(|config| config.wip = 3).await;
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
            SystemMarker::Question,
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
async fn a_run_without_a_marker_outcome_stays_open_for_the_abort_slice() {
    let f = fixture().await;
    f.enable().await;
    let key = f.start_tickets(1).await.remove(0);
    let run = f.finish_run(&key).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.outcome(run).await, None);
    assert_eq!(f.record(&key).await.status, TicketStatus::InProgress);
    assert_eq!(f.markers(&key, SystemMarker::Abort).await, 0);
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
    let comments = f.comment_count(&key).await;
    f.dispatcher.pass.run().await;
    assert_eq!(f.comment_count(&key).await, comments);
}
