#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::hooks::{HookRepository, HookService};
use nullradix_app::projects::ProjectService;
use nullradix_app::sessions::{SessionQuery, SessionService};
use nullradix_app::tickets::{NewTicket, TicketService, WriteOptions};
use nullradix_app::{ChangeFeed, ChangeOp, ChangeSignal, Clock, Entity, UuidV7Generator};
use nullradix_domain::{
    DisplayState, HookDetails, HookEvent, HookKind, Id, SessionRef, SessionState, SubagentState,
    Timestamp, Version,
};
use nullradix_store::Store;

const START: i64 = 1_791_626_400_000;
const DAY: i64 = 24 * 60 * 60 * 1000;

#[derive(Default)]
struct ManualClock(AtomicI64);

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.0.load(Ordering::SeqCst))
    }
}

struct Fixture {
    store: Arc<Store>,
    clock: Arc<ManualClock>,
    hooks: HookService,
    sessions: Arc<SessionService>,
    runners: Arc<nullradix_app::repositories::RunnerService>,
    signal: ChangeSignal,
    serial: AtomicU64,
}

async fn fixture() -> Fixture {
    fixture_on(Arc::new(Store::in_memory().await.unwrap())).await
}

async fn fixture_on(store: Arc<Store>) -> Fixture {
    let signal = ChangeSignal::new();
    let runners = common::runners(&store, &signal).await;
    let clock = Arc::new(ManualClock(AtomicI64::new(START)));
    let sessions = Arc::new(SessionService::new(
        store.clone(),
        runners.clone(),
        clock.clone(),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let hooks = HookService::new(store.clone(), clock.clone()).with_sessions(sessions.clone());
    Fixture {
        store,
        clock,
        hooks,
        sessions,
        runners,
        signal,
        serial: AtomicU64::new(0),
    }
}

impl Fixture {
    fn event(&self, kind: HookKind, session: &str, cwd: &str, details: HookDetails) -> HookEvent {
        let serial = u128::from(self.serial.fetch_add(1, Ordering::SeqCst) + 1);
        HookEvent::new(
            Id::from_uuid(uuid::Uuid::from_u128(serial)),
            kind,
            SessionRef::parse(session).unwrap(),
            cwd,
            self.clock.now(),
            details,
        )
        .unwrap()
    }

    async fn hook(&self, kind: HookKind, session: &str, cwd: &str, details: HookDetails) {
        let event = self.event(kind, session, cwd, details);
        self.hooks.receive(&event).await.unwrap();
    }

    fn advance(&self, millis: i64) {
        self.clock.0.fetch_add(millis, Ordering::SeqCst);
    }

    async fn ids(&self, query: SessionQuery<'_>) -> Vec<String> {
        self.sessions
            .list(query)
            .await
            .unwrap()
            .into_iter()
            .map(|line| line.session.id().as_str().to_owned())
            .collect()
    }
}

fn notification(kind: &str) -> HookDetails {
    HookDetails {
        notification_type: Some(kind.to_owned()),
        ..HookDetails::default()
    }
}

fn agent(id: &str) -> HookDetails {
    HookDetails {
        agent_id: Some(id.to_owned()),
        agent_type: Some(String::from("Explore")),
        ..HookDetails::default()
    }
}

#[tokio::test]
async fn a_hook_sequence_builds_one_session_and_each_event_is_kept_once() {
    let f = fixture().await;
    let cwd = "/elsewhere";
    let prompt = HookDetails {
        prompt: Some(String::from("Fix the login please")),
        model: Some(String::from("opus")),
        ..HookDetails::default()
    };
    f.hook(HookKind::SessionStart, "s1", cwd, HookDetails::default())
        .await;
    f.advance(1000);
    f.hook(HookKind::UserPrompt, "s1", cwd, prompt).await;
    let waiting = f.event(
        HookKind::Notification,
        "s1",
        cwd,
        notification("permission_prompt"),
    );
    f.hooks.receive(&waiting).await.unwrap();
    f.hooks.receive(&waiting).await.unwrap();
    assert_eq!(
        f.sessions.get("s1").await.unwrap().session.record().state,
        SessionState::Waiting
    );
    f.hook(HookKind::SubagentStart, "s1", cwd, agent("a1"))
        .await;
    f.advance(1000);
    f.hook(HookKind::SubagentStop, "s1", cwd, agent("a1")).await;
    f.hook(HookKind::Stop, "s1", cwd, HookDetails::default())
        .await;
    f.advance(1000);
    f.hook(HookKind::SessionEnd, "s1", cwd, HookDetails::default())
        .await;
    f.sessions.settled().await;

    let line = f.sessions.get("s1").await.unwrap();
    let record = line.session.record();
    assert_eq!(record.state, SessionState::Ended);
    assert_eq!(record.title.as_deref(), Some("Fix the login please"));
    assert_eq!(record.model.as_deref(), Some("opus"));
    assert_eq!(record.repository, None);
    assert_eq!(record.number, None);
    assert_eq!(record.runner, Some(f.runners.local().id()));
    assert_eq!(record.ended_at, Some(Timestamp::from_millis(START + 3000)));
    assert_eq!(record.subagents.len(), 1);
    assert_eq!(record.subagents[0].state, SubagentState::Ended);
    assert_eq!(line.view.display_state, DisplayState::Ended);
    assert_eq!(line.view.runtime.as_millis(), 3000);
    assert_eq!(f.store.recent(50).await.unwrap().len(), 7);
    assert_eq!(f.ids(SessionQuery::default()).await, ["s1"]);
    assert_eq!(
        f.sessions.get("nope").await.unwrap_err().code(),
        "session-not-found"
    );
    assert_eq!(
        f.sessions.get(" ").await.unwrap_err().code(),
        "session-id-invalid"
    );
}

#[tokio::test]
async fn sessions_list_latest_activity_first_and_filter_by_state() {
    let f = fixture().await;
    let none = HookDetails::default;
    f.hook(HookKind::SessionStart, "old", "/x", none()).await;
    f.hook(HookKind::SessionStart, "new", "/x", none()).await;
    assert_eq!(f.ids(SessionQuery::default()).await, ["new", "old"]);
    f.hook(HookKind::UserPrompt, "old", "/x", none()).await;
    assert_eq!(f.ids(SessionQuery::default()).await, ["old", "new"]);
    f.advance(1000);
    f.hook(HookKind::Stop, "new", "/x", none()).await;
    assert_eq!(f.ids(SessionQuery::default()).await, ["new", "old"]);
    let running = SessionQuery {
        state: Some("running"),
        ..SessionQuery::default()
    };
    assert_eq!(f.ids(running).await, ["old"]);
    f.advance(31 * 60 * 1000);
    let stale = SessionQuery {
        state: Some("stale"),
        ..SessionQuery::default()
    };
    assert_eq!(f.ids(stale).await, ["new", "old"]);
    let wrong = SessionQuery {
        state: Some("busy"),
        ..SessionQuery::default()
    };
    assert_eq!(
        f.sessions.list(wrong).await.unwrap_err().code(),
        "state-invalid"
    );
}

#[tokio::test]
async fn older_events_are_kept_but_change_no_session() {
    let f = fixture().await;
    f.advance(5000);
    f.hook(HookKind::UserPrompt, "s1", "/x", HookDetails::default())
        .await;
    f.advance(-1000);
    f.hook(HookKind::SessionEnd, "s1", "/x", HookDetails::default())
        .await;
    let record = f.sessions.get("s1").await.unwrap().session;
    assert_eq!(record.record().state, SessionState::Running);
    assert_eq!(record.record().version, Version::from_stored(2));
    assert_eq!(f.store.recent(10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn sessions_in_a_known_checkout_get_its_repository_and_numbers() {
    let f = fixture().await;
    let ids = Arc::new(UuidV7Generator);
    let person = local_person(f.store.as_ref(), "anna", ids.as_ref(), f.clock.as_ref())
        .await
        .unwrap();
    let projects = ProjectService::new(
        f.store.clone(),
        f.clock.clone(),
        ids.clone(),
        f.signal.clone(),
    );
    let boards = BoardService::new(
        f.store.clone(),
        f.store.clone(),
        f.store.clone(),
        f.runners.clone(),
        f.clock.clone(),
        ids.clone(),
        f.signal.clone(),
    );
    let project = projects.create("ARC", "Arches").await.unwrap();
    let board = boards.create(project.id(), "Main", &person).await.unwrap();
    let bound = boards
        .set_repository(
            project.id(),
            board.board.id(),
            f.runners.local().id(),
            "/work/arches",
            Version::INITIAL,
        )
        .await
        .unwrap();
    let repository = bound.board.repository().unwrap();

    let none = HookDetails::default;
    f.hook(HookKind::SessionStart, "a", "/work/arches/web", none())
        .await;
    f.hook(HookKind::SessionStart, "b", "/work/arches/web", none())
        .await;
    f.hook(HookKind::SessionStart, "c", "/outside", none())
        .await;
    f.sessions.settled().await;
    f.hook(HookKind::SessionStart, "d", "/work/arches/web", none())
        .await;
    f.sessions.settled().await;

    let number = |id: &'static str| {
        let sessions = f.sessions.clone();
        async move {
            let record = sessions.get(id).await.unwrap().session.record().clone();
            (record.repository, record.number)
        }
    };
    assert_eq!(number("a").await, (Some(repository), Some(1)));
    assert_eq!(number("b").await, (Some(repository), Some(2)));
    assert_eq!(number("c").await, (None, None));
    assert_eq!(number("d").await, (Some(repository), Some(3)));
    let in_repository = SessionQuery {
        repository: Some(repository),
        ..SessionQuery::default()
    };
    assert_eq!(f.ids(in_repository).await, ["d", "b", "a"]);
    let label = f.sessions.get("d").await.unwrap().session.label();
    assert_eq!(label, "SE-003");
}

#[tokio::test]
async fn the_ticket_a_hook_names_is_linked_once() {
    let f = fixture().await;
    let ids = Arc::new(UuidV7Generator);
    let person = local_person(f.store.as_ref(), "anna", ids.as_ref(), f.clock.as_ref())
        .await
        .unwrap();
    let projects = ProjectService::new(
        f.store.clone(),
        f.clock.clone(),
        ids.clone(),
        f.signal.clone(),
    );
    let boards = BoardService::new(
        f.store.clone(),
        f.store.clone(),
        f.store.clone(),
        f.runners.clone(),
        f.clock.clone(),
        ids.clone(),
        f.signal.clone(),
    );
    let tickets = TicketService::new(
        f.store.clone(),
        f.store.clone(),
        f.clock.clone(),
        ids.clone(),
        f.signal.clone(),
    );
    let project = projects.create("ARC", "Arches").await.unwrap();
    let board = boards.create(project.id(), "Main", &person).await.unwrap();
    let input = NewTicket {
        title: String::from("Login"),
        ..NewTicket::default()
    };
    let options = WriteOptions {
        by_person: true,
        force: false,
        allow_duplicate: true,
    };
    tickets
        .create(project.id(), board.board.id(), &input, &person, options)
        .await
        .unwrap();

    let named = || HookDetails {
        ticket: Some(String::from("arc-1, ARC-99")),
        ..HookDetails::default()
    };
    f.hook(HookKind::SessionStart, "s1", "/x", named()).await;
    f.hook(HookKind::UserPrompt, "s1", "/x", named()).await;
    f.hook(HookKind::SessionStart, "s2", "/x", HookDetails::default())
        .await;
    assert_eq!(f.sessions.get("s2").await.unwrap().tickets, Vec::new());
    let line = f.sessions.get("s1").await.unwrap();
    assert_eq!(line.tickets.len(), 1);
    assert_eq!(line.tickets[0].key.to_string(), "ARC-0001");
    assert_eq!(line.tickets[0].title, "Login");
    let linked: Vec<String> = f
        .sessions
        .for_ticket("ARC-1")
        .await
        .unwrap()
        .into_iter()
        .map(|line| line.session.id().as_str().to_owned())
        .collect();
    assert_eq!(linked, ["s1"]);
    let filtered = SessionQuery {
        ticket: Some("ARC-0001"),
        ..SessionQuery::default()
    };
    assert_eq!(f.ids(filtered).await, ["s1"]);
    let error = f.sessions.for_ticket("ARC-9").await.unwrap_err();
    assert_eq!(error.code(), "ticket-not-found");
}

#[tokio::test]
async fn sessions_are_forgotten_a_week_after_their_end_or_last_hook() {
    let f = fixture().await;
    let none = HookDetails::default;
    f.hook(HookKind::SessionStart, "ended", "/x", none()).await;
    f.hook(HookKind::SessionEnd, "ended", "/x", none()).await;
    f.hook(HookKind::SessionStart, "silent", "/x", none()).await;
    f.advance(7 * DAY - 1000);
    f.hook(HookKind::SessionStart, "fresh", "/x", none()).await;
    assert_eq!(f.sessions.prune().await.unwrap(), 0);
    f.advance(2000);
    assert_eq!(f.ids(SessionQuery::default()).await, ["fresh"]);
    let head = f.store.head().await.unwrap();
    assert_eq!(f.sessions.prune().await.unwrap(), 2);
    let changes = f.store.changes_since(head, 10).await.unwrap();
    assert_eq!(changes.len(), 2);
    assert!(
        changes
            .iter()
            .all(|change| change.entity == Entity::Session && change.op == ChangeOp::Delete)
    );
    assert_eq!(
        f.sessions.get("ended").await.unwrap_err().code(),
        "session-not-found"
    );
    assert_eq!(
        f.sessions.get("fresh").await.unwrap().session.id().as_str(),
        "fresh"
    );
}

/// Ticket keys of the session, in key order.
async fn keys(f: &Fixture, id: &str) -> Vec<String> {
    f.sessions
        .get(id)
        .await
        .unwrap()
        .tickets
        .into_iter()
        .map(|ticket| ticket.key.to_string())
        .collect()
}

/// Rows written beside the store, for worktree states no service in this test can reach.
struct Raw(sqlx::SqlitePool);

impl Raw {
    async fn ticket(&self, number: i64) -> uuid::Uuid {
        sqlx::query_scalar("SELECT id FROM tickets WHERE number = ?")
            .bind(number)
            .fetch_one(&self.0)
            .await
            .unwrap()
    }

    async fn status(&self, number: i64, status: &str) {
        sqlx::query("UPDATE tickets SET status = ? WHERE number = ?")
            .bind(status)
            .bind(number)
            .execute(&self.0)
            .await
            .unwrap();
    }

    async fn worktree(
        &self,
        f: &Fixture,
        repository: uuid::Uuid,
        path: &str,
        desired: &str,
        ticket: i64,
        released: bool,
    ) {
        let id = uuid::Uuid::now_v7();
        sqlx::query(
            "INSERT INTO worktrees (id, repository_id, runner_id, branch, path, desired_state,
                                    actual_state, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, 'present', 1, 1)",
        )
        .bind(id)
        .bind(repository)
        .bind(f.runners.local().id().as_uuid())
        .bind(format!("branch-{id}"))
        .bind(path)
        .bind(desired)
        .execute(&self.0)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO worktree_tickets (worktree_id, ticket_id, position, joined_at, released_at)
             VALUES (?, ?, 0, 1, ?)",
        )
        .bind(id)
        .bind(self.ticket(ticket).await)
        .bind(released.then_some(2_i64))
        .execute(&self.0)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn sessions_in_the_worktree_of_an_in_progress_ticket_are_linked_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nullradix.db");
    let f = fixture_on(Arc::new(Store::open(&path).await.unwrap())).await;
    let url = format!("sqlite:{}", path.display().to_string().replace('\\', "/"));
    let raw = Raw(sqlx::SqlitePool::connect(&url).await.unwrap());
    let ids = Arc::new(UuidV7Generator);
    let person = local_person(f.store.as_ref(), "anna", ids.as_ref(), f.clock.as_ref())
        .await
        .unwrap();
    let projects = ProjectService::new(
        f.store.clone(),
        f.clock.clone(),
        ids.clone(),
        f.signal.clone(),
    );
    let boards = BoardService::new(
        f.store.clone(),
        f.store.clone(),
        f.store.clone(),
        f.runners.clone(),
        f.clock.clone(),
        ids.clone(),
        f.signal.clone(),
    );
    let tickets = TicketService::new(
        f.store.clone(),
        f.store.clone(),
        f.clock.clone(),
        ids.clone(),
        f.signal.clone(),
    );
    let project = projects.create("ARC", "Arches").await.unwrap();
    let board = boards.create(project.id(), "Main", &person).await.unwrap();
    let bound = boards
        .set_repository(
            project.id(),
            board.board.id(),
            f.runners.local().id(),
            "/work/arches",
            Version::INITIAL,
        )
        .await
        .unwrap();
    let repository = bound.board.repository().unwrap().as_uuid();
    let options = WriteOptions {
        by_person: true,
        force: false,
        allow_duplicate: true,
    };
    for title in ["Live", "Backlog", "Released", "Removed", "Second"] {
        let input = NewTicket {
            title: title.to_owned(),
            ..NewTicket::default()
        };
        tickets
            .create(project.id(), board.board.id(), &input, &person, options)
            .await
            .unwrap();
    }
    for number in [1, 3, 4, 5] {
        raw.status(number, "in-progress").await;
    }
    raw.worktree(&f, repository, "/wt/T-1", "present", 1, false)
        .await;
    raw.worktree(&f, repository, "/wt/T-2", "present", 2, false)
        .await;
    raw.worktree(&f, repository, "/wt/T-3", "present", 3, true)
        .await;
    raw.worktree(&f, repository, "/wt/T-4", "absent", 4, false)
        .await;

    let none = HookDetails::default;
    for (session, cwd) in [
        ("inside", "/wt/T-1/src"),
        ("sibling", "/wt/T-10"),
        ("backlog", "/wt/T-2"),
        ("released", "/wt/T-3"),
        ("removed", "/wt/T-4"),
        ("cased", "/WT/t-1"),
    ] {
        f.hook(HookKind::SessionStart, session, cwd, none()).await;
    }
    assert_eq!(keys(&f, "inside").await, ["ARC-0001"]);
    for session in ["sibling", "backlog", "released", "removed"] {
        assert_eq!(keys(&f, session).await, Vec::<String>::new(), "{session}");
    }
    let folded: Vec<String> = if cfg!(windows) {
        vec![String::from("ARC-0001")]
    } else {
        Vec::new()
    };
    assert_eq!(keys(&f, "cased").await, folded);

    raw.status(2, "in-progress").await;
    f.hook(HookKind::UserPrompt, "backlog", "/wt/T-2", none())
        .await;
    assert_eq!(keys(&f, "backlog").await, ["ARC-0002"]);
    raw.worktree(&f, repository, "/wt/T-2/inner", "present", 5, false)
        .await;
    f.hook(HookKind::Stop, "backlog", "/wt/T-2/inner", none())
        .await;
    assert_eq!(keys(&f, "backlog").await, ["ARC-0002"]);
    f.hook(HookKind::SessionStart, "fresh", "/wt/T-2/inner", none())
        .await;
    assert_eq!(keys(&f, "fresh").await, ["ARC-0002", "ARC-0005"]);
}
