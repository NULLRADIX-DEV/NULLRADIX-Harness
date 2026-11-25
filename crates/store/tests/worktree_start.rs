#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::projects::ProjectService;
use nullradix_app::repositories::{Located, RunnerService, not_a_git_project};
use nullradix_app::runs::{RunProgress, RunSpec, RunnerPort};
use nullradix_app::session_detail::{
    ChangeRequest, DiffRequest, FileDiff, SessionChanges, TranscriptChunk, TranscriptRead,
    TranscriptSource, TranscriptSummary, change_not_found,
};
use nullradix_app::tickets::{NewTicket, TicketPatch, TicketService, WriteOptions};
use nullradix_app::worktrees::{
    Cleaned, CreatedWorktree, DoneRequest, EnsureWorktree, EnsuredWorktree, Finished, NewWorktree,
    RemoveWorktree, StartMode, Started, TargetCreated, WorktreeRemoval, WorktreeRepository,
    WorktreeService,
};
use nullradix_app::{AppError, ChangeSignal, Clock, SystemClock, UuidV7Generator};
use nullradix_domain::{
    Actor, ActualState, Board, DesiredState, Id, Run, Runner, TicketStatus, Version, Worktree,
    WorktreeBranch, WorktreeDraft, WorktreePath, WorktreePurpose,
};
use nullradix_store::Store;
use tokio::sync::Notify;

/// Lets a test stop the runner ensure half way.
#[derive(Default)]
struct Gate {
    entered: Notify,
    release: Notify,
}

/// Git double: folders below `/work/<name>` are checkouts, created worktrees exist.
#[derive(Default)]
struct FakeGit {
    requests: Mutex<Vec<NewWorktree>>,
    folders: Mutex<HashSet<String>>,
    /// Code of the error the next create or ensure fails with.
    failure: Mutex<Option<&'static str>>,
    long_paths: Mutex<bool>,
    removals: Mutex<Vec<RemoveWorktree>>,
    /// Reason the runner keeps worktrees for; `None` removes them.
    keep: Mutex<Option<&'static str>>,
    ensures: Mutex<Vec<EnsureWorktree>>,
    branch_missing: AtomicBool,
    /// Where a leftover folder goes at the next ensure.
    rest: Mutex<Option<String>>,
    gate: Mutex<Option<Arc<Gate>>>,
}

#[async_trait]
impl RunnerPort for FakeGit {
    async fn check(&self, _spec: &RunSpec) -> Result<(), AppError> {
        Err(AppError::unavailable("runner-failed", "runs nothing"))
    }

    async fn start(&self, _run: Id<Run>, _spec: &RunSpec) -> Result<u32, AppError> {
        Err(AppError::unavailable("runner-failed", "runs nothing"))
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
        Err(AppError::unavailable("runner-failed", "runs nothing"))
    }

    async fn locate(&self, path: &str) -> Result<Located, AppError> {
        let mut parts = path.trim_start_matches('/').split('/');
        match (parts.next(), parts.next()) {
            (Some("work"), Some(name)) if !name.is_empty() => Ok(Located {
                root: format!("/work/{name}"),
                key: format!("/work/{name}"),
                name: name.to_owned(),
            }),
            _ => Err(not_a_git_project(path)),
        }
    }

    async fn create_worktree(&self, request: &NewWorktree) -> Result<CreatedWorktree, AppError> {
        if let Some(code) = self.failure.lock().unwrap().take() {
            return Err(AppError::invalid(code, "refused"));
        }
        self.requests.lock().unwrap().push(request.clone());
        self.folders.lock().unwrap().insert(request.path.clone());
        Ok(CreatedWorktree {
            target_created_from: request.target.as_ref().map(|_| String::from("main")),
            long_paths_enabled: *self.long_paths.lock().unwrap(),
        })
    }

    async fn folders_exist(&self, paths: &[String]) -> Result<Vec<bool>, AppError> {
        let folders = self.folders.lock().unwrap();
        Ok(paths.iter().map(|path| folders.contains(path)).collect())
    }

    async fn ensure_worktree(&self, request: &EnsureWorktree) -> Result<EnsuredWorktree, AppError> {
        if let Some(code) = self.failure.lock().unwrap().take() {
            return Err(AppError::invalid(code, "refused"));
        }
        self.ensures.lock().unwrap().push(request.clone());
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        if self.branch_missing.load(Ordering::SeqCst) {
            return Ok(EnsuredWorktree::BranchMissing);
        }
        if !self.folders.lock().unwrap().insert(request.path.clone()) {
            return Ok(EnsuredWorktree::Present);
        }
        let moved_aside = self.rest.lock().unwrap().take();
        Ok(EnsuredWorktree::Restored { moved_aside })
    }

    async fn remove_worktree(&self, request: &RemoveWorktree) -> Result<WorktreeRemoval, AppError> {
        self.removals.lock().unwrap().push(request.clone());
        if !self.folders.lock().unwrap().contains(&request.path) {
            return Ok(WorktreeRemoval::Missing);
        }
        if let Some(reason) = *self.keep.lock().unwrap() {
            return Ok(WorktreeRemoval::Kept(reason.to_owned()));
        }
        self.folders.lock().unwrap().remove(&request.path);
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

struct Fixture {
    git: Arc<FakeGit>,
    tickets: Arc<TicketService>,
    worktrees: WorktreeService,
    store: Arc<Store>,
    person: Actor,
    bound: Board,
    loose: Board,
    runner: Id<Runner>,
}

const FORCE: WriteOptions = WriteOptions {
    by_person: false,
    force: true,
    allow_duplicate: true,
};

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let clock = Arc::new(SystemClock);
    let ids = Arc::new(UuidV7Generator);
    let git = Arc::new(FakeGit::default());
    let runners = Arc::new(
        RunnerService::start(
            store.clone(),
            store.clone(),
            git.clone(),
            clock.clone(),
            ids.clone(),
            signal.clone(),
        )
        .await
        .unwrap(),
    );
    let runner = runners.local().id();
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
    let bound = boards
        .set_repository(
            project.id(),
            created.board.id(),
            runner,
            "/work/arches",
            Version::INITIAL,
        )
        .await
        .unwrap()
        .board;
    let loose = boards
        .create(project.id(), "Loose", &person)
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
    let worktrees = WorktreeService::new(
        tickets.clone(),
        store.clone(),
        store.clone(),
        runners,
        clock,
        ids,
        signal,
    );
    Fixture {
        git,
        tickets,
        worktrees,
        store,
        person,
        bound,
        loose,
        runner,
    }
}

impl Fixture {
    async fn ticket_on(&self, board: &Board, input: NewTicket) -> String {
        self.tickets
            .create(board.project(), board.id(), &input, &self.person, FORCE)
            .await
            .unwrap()
            .ticket
            .key()
            .to_string()
    }

    async fn ticket(&self, title: &str) -> String {
        let input = NewTicket {
            title: title.to_owned(),
            ..NewTicket::default()
        };
        self.ticket_on(&self.bound, input).await
    }

    async fn start(&self, key: &str, cwd: &str, mode: StartMode) -> Result<Started, AppError> {
        self.worktrees
            .start(key, self.runner, cwd, mode, false, &self.person)
            .await
    }

    fn created(&self) -> usize {
        self.git.requests.lock().unwrap().len()
    }

    async fn done(&self, key: &str, cwd: &str) -> Result<Finished, AppError> {
        let request = DoneRequest {
            acceptance: Some("It works"),
            expected: None,
            options: WriteOptions::default(),
        };
        self.worktrees
            .done(key, self.runner, cwd, request, &self.person)
            .await
    }

    async fn worktree_of(&self, key: &str) -> Worktree {
        let id = self.tickets.get(key).await.unwrap().view.ticket.id();
        let entries = self
            .store
            .list(self.bound.repository().unwrap(), self.runner)
            .await
            .unwrap();
        entries
            .into_iter()
            .find(|entry| entry.tickets.iter().any(|link| link.ticket == id))
            .unwrap()
            .worktree
    }

    async fn accepted(&self, title: &str) -> String {
        let input = NewTicket {
            title: title.to_owned(),
            acceptance: Some(String::from("A person looked")),
            ..NewTicket::default()
        };
        self.ticket_on(&self.bound, input).await
    }

    async fn block(&self, key: &str, reason: &str) {
        let version = self
            .tickets
            .get(key)
            .await
            .unwrap()
            .view
            .ticket
            .record()
            .version;
        let patch = TicketPatch {
            status: Some(String::from("blocked")),
            blocked_reason: Some(Some(reason.to_owned())),
            ..TicketPatch::default()
        };
        self.tickets
            .update(key, &patch, &self.person, version, FORCE)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_new_worktree_links_the_ticket_and_moves_it_to_in_progress() {
    let fixture = fixture().await;
    let key = fixture.ticket("Fix the login").await;
    let started = fixture
        .start(&key, "/work/arches/src", StartMode::New)
        .await
        .unwrap();
    assert_eq!(started.branch, "ticket/ARC-0001-fix-the-login");
    assert_eq!(started.worktree, "/work/arches-worktrees/ARC-0001");
    assert!(!started.joined);
    assert_eq!(started.warnings, Vec::<String>::new());
    assert_eq!(started.target_created, None);
    let record = started.ticket.ticket.record();
    assert_eq!(record.status, TicketStatus::InProgress);
    assert_eq!(record.branch.as_deref(), Some(started.branch.as_str()));
    assert_eq!(record.worktree.as_deref(), Some(started.worktree.as_str()));
    let request = fixture.git.requests.lock().unwrap()[0].clone();
    assert_eq!(request.cwd, "/work/arches/src");
    assert_eq!(request.root, "/work/arches");
    assert_eq!(request.target, None);
    let entry = fixture
        .store
        .of_ticket(started.ticket.ticket.id())
        .await
        .unwrap()
        .unwrap();
    let worktree = entry.worktree.record();
    assert_eq!(
        (worktree.desired, worktree.actual),
        (DesiredState::Present, ActualState::Present)
    );
    assert!(worktree.observed_at.is_some());
    assert_eq!(entry.tickets.len(), 1);
    let history = fixture.tickets.get(&key).await.unwrap().history;
    let fields: Vec<(&str, Option<&str>)> = history
        .iter()
        .map(|line| (line.field.as_str(), line.note.as_deref()))
        .collect();
    for field in ["status", "branch", "worktree"] {
        assert!(fields.contains(&(field, Some("started"))), "{fields:?}");
    }
    let again = fixture.start(&key, "/work/arches", StartMode::New).await;
    assert_eq!(again.unwrap_err().code(), "already-started");
    assert_eq!(fixture.created(), 1);
}

#[tokio::test]
async fn the_board_must_point_to_the_folder_repository() {
    let fixture = fixture().await;
    let key = fixture.ticket("Here").await;
    let foreign = fixture.start(&key, "/work/other", StartMode::New).await;
    assert_eq!(foreign.unwrap_err().code(), "board-repo-mismatch");
    let input = NewTicket {
        title: String::from("Nowhere"),
        ..NewTicket::default()
    };
    let loose = fixture.ticket_on(&fixture.loose, input).await;
    let unbound = fixture.start(&loose, "/work/arches", StartMode::New).await;
    assert_eq!(unbound.unwrap_err().code(), "board-repo-mismatch");
    let outside = fixture.start(&key, "/elsewhere", StartMode::New).await;
    assert_eq!(outside.unwrap_err().code(), "not-a-git-project");
    assert_eq!(fixture.created(), 0);
}

#[tokio::test]
async fn open_dependencies_lock_the_start_unless_forced_or_already_in_progress() {
    let fixture = fixture().await;
    let first = fixture.ticket("First").await;
    let input = NewTicket {
        title: String::from("Second"),
        depends_on: vec![first.clone()],
        ..NewTicket::default()
    };
    let second = fixture.ticket_on(&fixture.bound, input).await;
    let locked = fixture.start(&second, "/work/arches", StartMode::New).await;
    assert_eq!(locked.unwrap_err().code(), "dependency-open");
    let forced = fixture
        .worktrees
        .start(
            &second,
            fixture.runner,
            "/work/arches",
            StartMode::New,
            true,
            &fixture.person,
        )
        .await
        .unwrap();
    assert_eq!(forced.ticket.ticket.status(), TicketStatus::InProgress);
    let input = NewTicket {
        title: String::from("Running"),
        status: Some(String::from("in-progress")),
        depends_on: vec![first],
        ..NewTicket::default()
    };
    let running = fixture.ticket_on(&fixture.bound, input).await;
    fixture
        .start(&running, "/work/arches", StartMode::New)
        .await
        .unwrap();
}

#[tokio::test]
async fn joining_a_ticket_shares_its_worktree_or_falls_back_with_a_warning() {
    let fixture = fixture().await;
    let host = fixture.ticket("Host").await;
    let guest = fixture.ticket("Guest").await;
    let hosted = fixture
        .start(&host, "/work/arches", StartMode::New)
        .await
        .unwrap();
    let joined = fixture
        .start(&guest, "/work/arches", StartMode::JoinTicket(host.clone()))
        .await
        .unwrap();
    assert!(joined.joined);
    assert_eq!(
        (&joined.branch, &joined.worktree),
        (&hosted.branch, &hosted.worktree)
    );
    assert_eq!(joined.ticket.ticket.status(), TicketStatus::InProgress);
    assert_eq!(fixture.created(), 1);
    let entry = fixture
        .store
        .of_ticket(joined.ticket.ticket.id())
        .await
        .unwrap()
        .unwrap();
    let keys: Vec<String> = entry.tickets.iter().map(|t| t.key.to_string()).collect();
    assert_eq!(keys, [host.clone(), guest]);
    let input = NewTicket {
        title: String::from("Release fix"),
        target_branch: Some(String::from("release")),
        ..NewTicket::default()
    };
    let other = fixture.ticket_on(&fixture.bound, input).await;
    let fallback = fixture
        .start(&other, "/work/arches", StartMode::JoinTicket(host.clone()))
        .await
        .unwrap();
    assert!(!fallback.joined);
    assert_eq!(
        fallback.warnings,
        ["ARC-0003 has another target branch than ARC-0001; it gets its own worktree"]
    );
    assert_eq!(
        fallback.target_created,
        Some(TargetCreated {
            branch: String::from("release"),
            from: String::from("main"),
        })
    );
    let idle = fixture.ticket("Idle").await;
    let lonely = fixture.ticket("Lonely").await;
    let refused = fixture
        .start(&lonely, "/work/arches", StartMode::JoinTicket(idle))
        .await;
    assert_eq!(refused.unwrap_err().code(), "host-not-started");
}

#[tokio::test]
async fn auto_join_takes_the_first_open_worktree_with_the_same_target() {
    let fixture = fixture().await;
    let first = fixture.ticket("First").await;
    let alone = fixture
        .start(&first, "/work/arches", StartMode::AutoJoin)
        .await
        .unwrap();
    assert!(!alone.joined);
    let other = fixture.ticket("Other").await;
    let later = fixture
        .start(&other, "/work/arches", StartMode::New)
        .await
        .unwrap();
    assert_ne!(later.worktree, alone.worktree);
    let second = fixture.ticket("Second").await;
    let joined = fixture
        .start(&second, "/work/arches", StartMode::AutoJoin)
        .await
        .unwrap();
    assert!(joined.joined);
    assert_eq!(joined.worktree, alone.worktree);
    *fixture.git.long_paths.lock().unwrap() = true;
    let input = NewTicket {
        title: String::from("Release"),
        target_branch: Some(String::from("release")),
        ..NewTicket::default()
    };
    let release = fixture.ticket_on(&fixture.bound, input).await;
    let own = fixture
        .start(&release, "/work/arches", StartMode::AutoJoin)
        .await
        .unwrap();
    assert!(!own.joined);
    assert_eq!(own.warnings.len(), 1);
    assert!(own.warnings[0].starts_with("core.longpaths"));
    fixture.git.folders.lock().unwrap().remove(&alone.worktree);
    let third = fixture.ticket("Third").await;
    let next = fixture
        .start(&third, "/work/arches", StartMode::AutoJoin)
        .await
        .unwrap();
    assert!(next.joined);
    assert_eq!(next.worktree, later.worktree);
    fixture.git.folders.lock().unwrap().remove(&later.worktree);
    let fourth = fixture.ticket("Fourth").await;
    let fresh = fixture
        .start(&fourth, "/work/arches", StartMode::AutoJoin)
        .await
        .unwrap();
    assert!(!fresh.joined);
    assert_eq!(fresh.worktree, "/work/arches-worktrees/ARC-0006");
}

#[tokio::test]
async fn a_failed_git_step_leaves_the_ticket_untouched() {
    let fixture = fixture().await;
    let key = fixture.ticket("Empty repo").await;
    *fixture.git.failure.lock().unwrap() = Some("no-commit");
    let error = fixture
        .start(&key, "/work/arches", StartMode::New)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "no-commit");
    let detail = fixture.tickets.get(&key).await.unwrap();
    assert_eq!(detail.view.ticket.status(), TicketStatus::Backlog);
    assert_eq!(detail.view.ticket.record().branch, None);
    let entries = fixture
        .store
        .list(fixture.bound.repository().unwrap(), fixture.runner)
        .await
        .unwrap();
    assert_eq!(entries.len(), 0);
}

#[tokio::test]
async fn starting_a_blocked_ticket_clears_its_reason() {
    let fixture = fixture().await;
    let key = fixture.ticket("Stuck").await;
    let patch = TicketPatch {
        status: Some(String::from("blocked")),
        blocked_reason: Some(Some(String::from("waiting for the API"))),
        ..TicketPatch::default()
    };
    fixture
        .tickets
        .update(&key, &patch, &fixture.person, Version::INITIAL, FORCE)
        .await
        .unwrap();
    let started = fixture
        .start(&key, "/work/arches", StartMode::New)
        .await
        .unwrap();
    let record = started.ticket.ticket.record();
    assert_eq!(record.status, TicketStatus::InProgress);
    assert_eq!(record.blocked_reason, None);
    let history = fixture.tickets.get(&key).await.unwrap().history;
    let cleared = history
        .iter()
        .find(|line| line.field == "blockedReason" && line.note.as_deref() == Some("started"))
        .unwrap();
    assert_eq!(cleared.from.as_deref(), Some("waiting for the API"));
    assert_eq!(cleared.to, None);
}

#[tokio::test]
async fn a_stale_live_row_with_the_branch_is_replaced() {
    let fixture = fixture().await;
    let key = fixture.ticket("Fix").await;
    let holder = fixture.ticket("Holder").await;
    let holder_id = fixture.tickets.get(&holder).await.unwrap().view.ticket.id();
    let repository = fixture.bound.repository().unwrap();
    let draft = WorktreeDraft {
        repository,
        runner: fixture.runner,
        branch: WorktreeBranch::parse("ticket/ARC-0001-fix").unwrap(),
        target_branch: None,
        path: WorktreePath::parse("/work/arches-worktrees/OLD").unwrap(),
        purpose: WorktreePurpose::Work,
    };
    let stale = Worktree::new(
        Id::from_uuid(uuid::Uuid::now_v7()),
        draft,
        nullradix_domain::Timestamp::from_millis(1),
    );
    let mut write = nullradix_app::tickets::TicketRepository::begin(fixture.store.as_ref())
        .await
        .unwrap();
    write.insert_worktree(&stale, holder_id).await.unwrap();
    write.commit().await.unwrap();
    let started = fixture
        .start(&key, "/work/arches", StartMode::New)
        .await
        .unwrap();
    assert_eq!(started.branch, "ticket/ARC-0001-fix");
    let old = fixture.store.get(stale.id()).await.unwrap().unwrap();
    let record = old.worktree.record();
    assert_eq!(
        (record.desired, record.actual),
        (DesiredState::Absent, ActualState::Removed)
    );
    assert_eq!(old.tickets.len(), 1);
    assert_eq!(old.tickets[0].ticket, holder_id);
    assert!(!old.tickets[0].live());
    assert_eq!(fixture.store.of_ticket(holder_id).await.unwrap(), None);
    let new = fixture
        .store
        .of_ticket(started.ticket.ticket.id())
        .await
        .unwrap()
        .unwrap();
    assert_ne!(new.worktree.id(), stale.id());
    assert_eq!(new.owner().unwrap().ticket, started.ticket.ticket.id());
    assert_eq!(
        fixture
            .store
            .list(repository, fixture.runner)
            .await
            .unwrap()
            .len(),
        2
    );
}

fn removed(path: &str) -> Cleaned {
    Cleaned {
        path: path.to_owned(),
        branch: String::new(),
        removal: WorktreeRemoval::Removed,
    }
}

fn outcomes(cleanup: &[Cleaned]) -> Vec<(String, WorktreeRemoval)> {
    cleanup
        .iter()
        .map(|cleaned| (cleaned.path.clone(), cleaned.removal.clone()))
        .collect()
}

#[tokio::test]
async fn done_marks_the_worktree_ready_and_removes_it_keeping_the_branch() {
    let fixture = fixture().await;
    let key = fixture.ticket("Fix the login").await;
    let started = fixture
        .start(&key, "/work/arches", StartMode::New)
        .await
        .unwrap();
    assert_eq!(started.cleanup, Vec::new());
    let finished = fixture.done(&key, "/work/arches/src").await.unwrap();
    assert_eq!(finished.ticket.ticket.status(), TicketStatus::Done);
    assert_eq!(finished.ticket.ticket.record().acceptance, "It works");
    assert!(finished.ready_to_merge);
    assert_eq!(finished.branch.as_deref(), Some(started.branch.as_str()));
    assert_eq!(
        outcomes(&finished.cleanup),
        outcomes(&[removed(&started.worktree)])
    );
    assert_eq!(finished.cleanup[0].branch, started.branch);
    let request = fixture.git.removals.lock().unwrap()[0].clone();
    assert_eq!(
        request,
        RemoveWorktree {
            root: String::from("/work/arches"),
            path: started.worktree.clone(),
            cwd: String::from("/work/arches/src"),
        }
    );
    let record = fixture.worktree_of(&key).await.record().clone();
    assert!(record.ready_to_merge);
    assert_eq!(
        (record.desired, record.actual, record.last_error),
        (DesiredState::Absent, ActualState::Removed, None)
    );
    let ticket = fixture.tickets.get(&key).await.unwrap().view.ticket;
    assert_eq!(
        ticket.record().branch.as_deref(),
        Some(started.branch.as_str())
    );
    let again = fixture.done(&key, "/work/arches").await.unwrap();
    assert!(again.ready_to_merge);
    assert_eq!(again.cleanup, Vec::new());
    assert_eq!(fixture.git.removals.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn done_keeps_the_legacy_checks() {
    let fixture = fixture().await;
    let key = fixture.ticket("Fix").await;
    fixture
        .start(&key, "/work/arches", StartMode::New)
        .await
        .unwrap();
    let foreign = fixture.done(&key, "/work/other").await;
    assert_eq!(foreign.unwrap_err().code(), "board-repo-mismatch");
    let request = DoneRequest {
        acceptance: None,
        expected: None,
        options: WriteOptions::default(),
    };
    let missing = fixture
        .worktrees
        .done(
            &key,
            fixture.runner,
            "/work/arches",
            request,
            &fixture.person,
        )
        .await;
    assert_eq!(missing.unwrap_err().code(), "acceptance-missing");
    let stale = DoneRequest {
        acceptance: Some("ok"),
        expected: Some(Version::INITIAL),
        options: WriteOptions::default(),
    };
    let conflict = fixture
        .worktrees
        .done(&key, fixture.runner, "/work/arches", stale, &fixture.person)
        .await
        .unwrap_err();
    assert_eq!(conflict.kind(), nullradix_app::ErrorKind::Conflict);
    let loose = NewTicket {
        title: String::from("Loose"),
        acceptance: Some(String::from("done when done")),
        ..NewTicket::default()
    };
    let plain = fixture.ticket_on(&fixture.bound, loose).await;
    let finished = fixture.done(&plain, "/work/arches").await.unwrap();
    assert!(!finished.ready_to_merge);
    assert_eq!(finished.branch, None);
    assert!(fixture.git.removals.lock().unwrap().is_empty());
}

#[tokio::test]
async fn readiness_waits_for_every_live_ticket_and_is_written_only_on_change() {
    let fixture = fixture().await;
    let host = fixture.ticket("Host").await;
    let guest = fixture.ticket("Guest").await;
    fixture
        .start(&host, "/work/arches", StartMode::New)
        .await
        .unwrap();
    fixture
        .start(&guest, "/work/arches", StartMode::JoinTicket(host.clone()))
        .await
        .unwrap();
    let before = fixture.worktree_of(&host).await.record().version;
    let first = fixture.done(&host, "/work/arches").await.unwrap();
    assert!(!first.ready_to_merge);
    assert_eq!(first.cleanup, Vec::new());
    assert_eq!(fixture.worktree_of(&host).await.record().version, before);
    *fixture.git.keep.lock().unwrap() = Some("the current folder is inside it");
    fixture.block(&guest, "Merge: conflict in a.rs").await;
    let ready = fixture.done(&host, "/work/arches").await.unwrap();
    assert!(ready.ready_to_merge);
    let kept = WorktreeRemoval::Kept(String::from("the current folder is inside it"));
    assert_eq!(ready.cleanup[0].removal, kept);
    let record = fixture.worktree_of(&host).await.record().clone();
    assert!(record.ready_to_merge);
    // ready, the claim and its release with the reason
    assert_eq!(record.version, before.next().next().next());
    assert_eq!(record.desired, DesiredState::Present);
    assert_eq!(
        record.last_error.as_deref(),
        Some("the current folder is inside it")
    );
    let unchanged = fixture.done(&host, "/work/arches").await.unwrap();
    assert!(unchanged.ready_to_merge);
    let again = fixture.worktree_of(&host).await.record().clone();
    assert!(again.ready_to_merge);
    assert_eq!(again.desired, DesiredState::Present);
    assert_eq!(again.last_error, record.last_error);
}

#[tokio::test]
async fn a_released_ticket_no_longer_holds_the_worktree_open() {
    let fixture = fixture().await;
    let host = fixture.ticket("Host").await;
    let guest = fixture.ticket("Guest").await;
    fixture
        .start(&host, "/work/arches", StartMode::New)
        .await
        .unwrap();
    fixture
        .start(&guest, "/work/arches", StartMode::JoinTicket(host.clone()))
        .await
        .unwrap();
    let version = fixture
        .tickets
        .get(&guest)
        .await
        .unwrap()
        .view
        .ticket
        .record()
        .version;
    let clear = TicketPatch {
        clear_link: true,
        ..TicketPatch::default()
    };
    fixture
        .tickets
        .update(&guest, &clear, &fixture.person, version, FORCE)
        .await
        .unwrap();
    let finished = fixture.done(&host, "/work/arches").await.unwrap();
    assert!(finished.ready_to_merge);
    assert_eq!(finished.cleanup.len(), 1);
}

#[tokio::test]
async fn accept_marks_the_worktree_ready_and_cleans_up_from_the_main_checkout() {
    let fixture = fixture().await;
    let key = fixture.accepted("Review me").await;
    let started = fixture
        .start(&key, "/work/arches", StartMode::New)
        .await
        .unwrap();
    fixture.block(&key, "Human Interaction: review").await;
    let view = fixture
        .worktrees
        .accept(&key, &fixture.person, None, true)
        .await
        .unwrap();
    assert_eq!(view.ticket.status(), TicketStatus::Done);
    let request = fixture.git.removals.lock().unwrap()[0].clone();
    assert_eq!(
        (request.root.as_str(), request.cwd.as_str(), &request.path),
        ("/work/arches", "/work/arches", &started.worktree)
    );
    let record = fixture.worktree_of(&key).await.record().clone();
    assert!(record.ready_to_merge);
    assert_eq!(record.actual, ActualState::Removed);
    let plain = fixture.accepted("Without worktree").await;
    fixture.block(&plain, "Human Interaction: look").await;
    let accepted = fixture
        .worktrees
        .accept(&plain, &fixture.person, None, true)
        .await
        .unwrap();
    assert_eq!(accepted.ticket.status(), TicketStatus::Done);
    assert_eq!(fixture.git.removals.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn start_removes_ready_worktrees_before_it_begins() {
    let fixture = fixture().await;
    let first = fixture.ticket("First").await;
    let started = fixture
        .start(&first, "/work/arches", StartMode::New)
        .await
        .unwrap();
    *fixture.git.keep.lock().unwrap() = Some("busy");
    let finished = fixture.done(&first, "/work/arches").await.unwrap();
    assert!(finished.ready_to_merge);
    assert_eq!(
        fixture.worktree_of(&first).await.record().desired,
        DesiredState::Present
    );
    *fixture.git.keep.lock().unwrap() = None;
    let second = fixture.ticket("Second").await;
    let next = fixture
        .start(&second, "/work/arches", StartMode::AutoJoin)
        .await
        .unwrap();
    assert_eq!(
        outcomes(&next.cleanup),
        outcomes(&[removed(&started.worktree)])
    );
    assert!(!next.joined);
    let record = fixture.worktree_of(&first).await.record().clone();
    assert_eq!(
        (record.desired, record.actual, record.last_error),
        (DesiredState::Absent, ActualState::Removed, None)
    );
}

#[tokio::test]
async fn a_worktree_claimed_by_another_cleanup_is_left_to_it() {
    let fixture = fixture().await;
    let key = fixture.ticket("Fix").await;
    let started = fixture
        .start(&key, "/work/arches", StartMode::New)
        .await
        .unwrap();
    *fixture.git.keep.lock().unwrap() = Some("busy");
    fixture.done(&key, "/work/arches").await.unwrap();
    *fixture.git.keep.lock().unwrap() = None;
    let id = fixture.worktree_of(&key).await.id();
    let now = nullradix_domain::Timestamp::from_millis(i64::MAX / 2);
    let mut write = nullradix_app::tickets::TicketRepository::begin(fixture.store.as_ref())
        .await
        .unwrap();
    assert!(write.claim_removal(id, now).await.unwrap().is_some());
    assert!(write.claim_removal(id, now).await.unwrap().is_none());
    write.commit().await.unwrap();
    let calls = fixture.git.removals.lock().unwrap().len();
    let second = fixture.ticket("Second").await;
    let next = fixture
        .start(&second, "/work/arches", StartMode::AutoJoin)
        .await
        .unwrap();
    assert_eq!(next.cleanup, Vec::new());
    assert!(!next.joined);
    assert_eq!(fixture.git.removals.lock().unwrap().len(), calls);
    let mut write = nullradix_app::tickets::TicketRepository::begin(fixture.store.as_ref())
        .await
        .unwrap();
    assert!(write.mark_removed(id, now).await.unwrap().is_some());
    let late = write.mark_kept(id, Some("vanished"), now).await.unwrap();
    assert!(late.is_none());
    assert!(write.claim_removal(id, now).await.unwrap().is_none());
    write.commit().await.unwrap();
    let record = fixture.worktree_of(&key).await.record().clone();
    assert_eq!(
        (record.desired, record.actual, record.last_error),
        (DesiredState::Absent, ActualState::Removed, None)
    );
    assert_eq!(record.path.as_str(), started.worktree);
}

#[tokio::test]
async fn a_hand_deleted_ready_worktree_is_skipped_silently() {
    let fixture = fixture().await;
    let key = fixture.ticket("Fix").await;
    let started = fixture
        .start(&key, "/work/arches", StartMode::New)
        .await
        .unwrap();
    fixture
        .git
        .folders
        .lock()
        .unwrap()
        .remove(&started.worktree);
    let finished = fixture.done(&key, "/work/arches").await.unwrap();
    assert!(finished.ready_to_merge);
    assert_eq!(finished.cleanup, Vec::new());
    let second = fixture.ticket("Second").await;
    let next = fixture
        .start(&second, "/work/arches", StartMode::New)
        .await
        .unwrap();
    assert_eq!(next.cleanup, Vec::new());
    assert_eq!(fixture.git.removals.lock().unwrap().len(), 2);
    let record = fixture.worktree_of(&key).await.record().clone();
    assert!(record.ready_to_merge);
    assert_eq!(
        (record.desired, record.actual, record.last_error),
        (DesiredState::Present, ActualState::Present, None)
    );
}

use nullradix_app::worktrees::Resumed;

impl Fixture {
    async fn started(&self, title: &str) -> (String, Started) {
        let key = self.ticket(title).await;
        let started = self
            .start(&key, "/work/arches", StartMode::New)
            .await
            .unwrap();
        (key, started)
    }

    async fn row_of(&self, key: &str) -> nullradix_domain::WorktreeRecord {
        self.worktree_of(key).await.record().clone()
    }

    /// The folder vanished behind the daemon's back.
    fn lose_folder(&self, path: &str) {
        self.git.folders.lock().unwrap().remove(path);
    }
}

#[tokio::test]
async fn only_a_started_ticket_can_be_resumed() {
    let fixture = fixture().await;
    let key = fixture.ticket("Idle").await;
    let ensure = fixture.worktrees.ensure(&key).await.unwrap_err();
    assert_eq!(ensure.code(), "ticket-not-started");
    let resume = fixture.worktrees.resume(&key, &fixture.person).await;
    assert_eq!(resume.unwrap_err().code(), "ticket-not-started");
    assert!(fixture.git.ensures.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_worktree_that_is_there_stays_as_it_is() {
    let fixture = fixture().await;
    let (key, started) = fixture.started("Fix the login").await;
    let before = fixture.row_of(&key).await;
    let Resumed::Ready(ready) = fixture
        .worktrees
        .resume(&key, &fixture.person)
        .await
        .unwrap()
    else {
        panic!("not ready");
    };
    assert_eq!(ready.path, started.worktree);
    assert_eq!(ready.branch, started.branch);
    assert!(!ready.restored);
    let request = fixture.git.ensures.lock().unwrap()[0].clone();
    assert_eq!(request.root, "/work/arches");
    assert_eq!(request.branch, started.branch);
    assert_eq!(request.path, started.worktree);
    let after = fixture.row_of(&key).await;
    assert_eq!(
        (after.desired, after.actual, after.branch, after.path),
        (before.desired, before.actual, before.branch, before.path)
    );
}

#[tokio::test]
async fn a_missing_folder_is_added_again_on_the_same_branch() {
    let fixture = fixture().await;
    let (key, started) = fixture.started("Fix the login").await;
    fixture.lose_folder(&started.worktree);
    let resumed = fixture
        .worktrees
        .resume(&key, &fixture.person)
        .await
        .unwrap();
    let Resumed::Ready(ready) = resumed else {
        panic!("not ready");
    };
    assert!(ready.restored);
    assert_eq!(ready.moved_aside, None);
    assert!(
        fixture
            .git
            .folders
            .lock()
            .unwrap()
            .contains(&started.worktree)
    );
    let ticket = fixture.tickets.get(&key).await.unwrap().view.ticket;
    let record = ticket.record();
    assert_eq!(record.branch.as_deref(), Some(started.branch.as_str()));
    assert_eq!(record.worktree.as_deref(), Some(started.worktree.as_str()));
    assert_eq!(fixture.created(), 1);
}

#[tokio::test]
async fn a_leftover_folder_moved_aside_is_reported() {
    let fixture = fixture().await;
    let (key, started) = fixture.started("Fix the login").await;
    fixture.lose_folder(&started.worktree);
    let aside = format!("{}.rest", started.worktree);
    *fixture.git.rest.lock().unwrap() = Some(aside.clone());
    let ensured = fixture.worktrees.ensure(&key).await.unwrap();
    assert!(ensured.restored);
    assert_eq!(ensured.moved_aside, Some(aside));
}

#[tokio::test]
async fn ensuring_a_cleaned_up_worktree_is_a_conflict_and_changes_nothing() {
    let fixture = fixture().await;
    let (key, _) = fixture.started("Fix the login").await;
    fixture.done(&key, "/work/arches").await.unwrap();
    let removed = fixture.row_of(&key).await;
    let error = fixture.worktrees.ensure(&key).await.unwrap_err();
    assert_eq!(error.code(), "worktree-removed");
    assert!(fixture.git.ensures.lock().unwrap().is_empty());
    assert_eq!(fixture.row_of(&key).await, removed);
}

#[tokio::test]
async fn a_cleaned_up_worktree_is_not_restored_by_a_resume_but_reset() {
    let fixture = fixture().await;
    let (key, started) = fixture.started("Fix the login").await;
    fixture.done(&key, "/work/arches").await.unwrap();
    let Resumed::Reset(text) = fixture
        .worktrees
        .resume(&key, &fixture.person)
        .await
        .unwrap()
    else {
        panic!("not reset");
    };
    assert!(text.contains("was cleaned up"), "{text}");
    assert!(text.contains(&started.branch), "{text}");
    assert!(fixture.git.ensures.lock().unwrap().is_empty());
    let ticket = fixture.tickets.get(&key).await.unwrap().view.ticket;
    assert_eq!(ticket.record().branch, None);
    assert_eq!(ticket.record().worktree, None);
    let row = fixture.row_of(&key).await;
    assert_eq!(row.branch.as_str(), started.branch);
    assert_eq!(row.actual, ActualState::Removed);
    assert_eq!(fixture.store.of_ticket(ticket.id()).await.unwrap(), None);
    let history = fixture.tickets.get(&key).await.unwrap().history;
    assert!(history.iter().any(|line| line.field == "branch"));
}

#[tokio::test]
async fn a_branch_missing_on_the_runner_skips_the_resume_and_fails_the_ensure() {
    let fixture = fixture().await;
    let (key, started) = fixture.started("Fix the login").await;
    fixture.lose_folder(&started.worktree);
    fixture.git.branch_missing.store(true, Ordering::SeqCst);
    let resumed = fixture
        .worktrees
        .resume(&key, &fixture.person)
        .await
        .unwrap();
    let Resumed::Skipped(text) = resumed else {
        panic!("not skipped");
    };
    assert!(text.contains(&started.branch), "{text}");
    let ensure = fixture.worktrees.ensure(&key).await.unwrap_err();
    assert_eq!(ensure.code(), "git-failed");
    assert!(
        !fixture
            .git
            .folders
            .lock()
            .unwrap()
            .contains(&started.worktree)
    );
}

#[tokio::test]
async fn a_refusal_of_the_runner_reaches_the_caller_and_changes_nothing() {
    let fixture = fixture().await;
    let (key, started) = fixture.started("Fix the login").await;
    fixture.lose_folder(&started.worktree);
    let before = fixture.row_of(&key).await;
    *fixture.git.failure.lock().unwrap() = Some("worktree-mismatch");
    let error = fixture
        .worktrees
        .resume(&key, &fixture.person)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "worktree-mismatch");
    assert_eq!(fixture.row_of(&key).await, before);
    let ticket = fixture.tickets.get(&key).await.unwrap().view.ticket;
    assert_eq!(
        ticket.record().worktree.as_deref(),
        Some(started.worktree.as_str())
    );
}

/// Holds the runner ensure of the next resume until the test lets it go.
fn hold_ensure(fixture: &Fixture) -> Arc<Gate> {
    let gate = Arc::new(Gate::default());
    *fixture.git.gate.lock().unwrap() = Some(gate.clone());
    gate
}

#[tokio::test]
async fn a_cleanup_waits_for_a_running_resume_and_the_folder_stays_removed() {
    let fixture = fixture().await;
    let (key, started) = fixture.started("Fix the login").await;
    fixture.lose_folder(&started.worktree);
    let gate = hold_ensure(&fixture);
    let resume = fixture.worktrees.resume(&key, &fixture.person);
    let done = async {
        gate.entered.notified().await;
        let finishing = fixture.done(&key, "/work/arches");
        let release = async {
            // the cleanup cannot touch the worktree while the resume holds it
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            assert!(fixture.git.removals.lock().unwrap().is_empty());
            gate.release.notify_one();
        };
        tokio::join!(finishing, release).0
    };
    let (resumed, finished) = tokio::join!(resume, done);
    assert!(matches!(resumed.unwrap(), Resumed::Ready(_)));
    finished.unwrap();
    let row = fixture.row_of(&key).await;
    assert_eq!(
        (row.desired, row.actual),
        (DesiredState::Absent, ActualState::Removed)
    );
    assert!(
        !fixture
            .git
            .folders
            .lock()
            .unwrap()
            .contains(&started.worktree)
    );
}

#[tokio::test]
async fn a_claim_made_during_the_runner_ensure_is_never_undone() {
    let fixture = fixture().await;
    let (key, started) = fixture.started("Fix the login").await;
    fixture.lose_folder(&started.worktree);
    let gate = hold_ensure(&fixture);
    let id = fixture.worktree_of(&key).await.id();
    let resume = fixture.worktrees.resume(&key, &fixture.person);
    let claim = async {
        gate.entered.notified().await;
        let mut write = nullradix_app::tickets::TicketRepository::begin(fixture.store.as_ref())
            .await
            .unwrap();
        let claimed = write.claim_removal(id, SystemClock.now()).await.unwrap();
        write.commit().await.unwrap();
        assert!(claimed.is_some());
        gate.release.notify_one();
    };
    let (resumed, ()) = tokio::join!(resume, claim);
    assert!(matches!(resumed.unwrap(), Resumed::Reset(_)));
    let row = fixture.row_of(&key).await;
    assert_eq!(row.desired, DesiredState::Absent);
    let ticket = fixture.tickets.get(&key).await.unwrap().view.ticket;
    assert_eq!(ticket.record().branch, None);
}
