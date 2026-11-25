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
use nullradix_app::comments::CommentService;
use nullradix_app::dispatch::assemble::{Dispatcher, DispatcherDeps, assemble};
use nullradix_app::dispatch::local_time::{LocalTime, minute_with_offset};
use nullradix_app::dispatch::runs::{DispatchRuns, RunFilesPort};
use nullradix_app::dispatch::setup::{SetupFailure, SetupOutcome, SetupRequest, WorktreeSetupPort};
use nullradix_app::dispatcher_config::DispatcherConfigService;
use nullradix_app::dispatcher_prompts::DispatcherPromptService;
use nullradix_app::projects::ProjectService;
use nullradix_app::questions::QuestionService;
use nullradix_app::repositories::{Located, RunnerService, not_a_git_project};
use nullradix_app::runs::{RunProgress, RunService, RunSpec, RunnerPort};
use nullradix_app::secrets::MemorySecretStore;
use nullradix_app::session_detail::{
    ChangeRequest, DiffRequest, FileDiff, SessionChanges, TranscriptChunk, TranscriptRead,
    TranscriptSource, TranscriptSummary, change_not_found,
};
use nullradix_app::tickets::{NewTicket, TicketPatch, TicketService, WriteOptions};
use nullradix_app::worktrees::{
    CreatedWorktree, EnsureWorktree, EnsuredWorktree, NewWorktree, RemoveWorktree, WorktreeRemoval,
    WorktreeService,
};
use nullradix_app::{AppError, ChangeSignal, Clock, UuidV7Generator};
use nullradix_domain::{
    Actor, ActorKind, Board, ConfigDir, Id, Run, Timestamp, Version, WorktreeSetupStep,
};
use nullradix_store::Store;
use tokio::sync::Notify;

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
}

#[async_trait]
impl RunnerPort for Machine {
    async fn check(&self, _spec: &RunSpec) -> Result<(), AppError> {
        Ok(())
    }

    async fn start(&self, _run: Id<Run>, spec: &RunSpec) -> Result<u32, AppError> {
        self.specs.lock().unwrap().push(spec.clone());
        Ok(4242)
    }

    async fn cancel(&self, _run: Id<Run>) -> Result<(), AppError> {
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
    async fn run(&self, request: &SetupRequest) -> Result<SetupOutcome, AppError> {
        self.runs.lock().unwrap().push(request.clone());
        self.entered.notify_one();
        let gate = self.release.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        Ok(match self.failure.lock().unwrap().clone() {
            Some(failure) => SetupOutcome::Failed(failure),
            None => SetupOutcome::Done,
        })
    }
}

struct Fixture {
    clock: Arc<FakeClock>,
    machine: Arc<Machine>,
    setup: Arc<ScriptedSetup>,
    dispatcher: Dispatcher,
    config: Arc<DispatcherConfigService>,
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
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
    let tickets = Arc::new(TicketService::new(
        store.clone(),
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
        questions,
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
        worktrees,
        clock: clock.clone(),
        ids,
        signal,
        local_time: clock.clone(),
        interval: None,
    });
    Fixture {
        clock,
        machine,
        setup,
        dispatcher,
        config,
        tickets,
        comments,
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
