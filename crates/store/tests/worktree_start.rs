#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::projects::ProjectService;
use nullradix_app::repositories::{Located, RunnerService, not_a_git_project};
use nullradix_app::runs::{RunProgress, RunSpec, RunnerPort};
use nullradix_app::tickets::{NewTicket, TicketPatch, TicketService, WriteOptions};
use nullradix_app::worktrees::{
    CreatedWorktree, NewWorktree, StartMode, Started, TargetCreated, WorktreeRepository,
    WorktreeService,
};
use nullradix_app::{AppError, ChangeSignal, SystemClock, UuidV7Generator};
use nullradix_domain::{
    Actor, ActualState, Board, DesiredState, Id, Run, Runner, TicketStatus, Version, Worktree,
    WorktreeBranch, WorktreeDraft, WorktreePath, WorktreePurpose,
};
use nullradix_store::Store;

/// Git double: folders below `/work/<name>` are checkouts, created worktrees exist.
#[derive(Default)]
struct FakeGit {
    requests: Mutex<Vec<NewWorktree>>,
    folders: Mutex<HashSet<String>>,
    /// Code of the error the next create fails with.
    failure: Mutex<Option<&'static str>>,
    long_paths: Mutex<bool>,
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
