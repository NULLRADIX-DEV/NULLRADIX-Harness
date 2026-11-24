#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;

use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::projects::ProjectService;
use nullradix_app::tickets::{
    NewTicket, StepGuard, StepPatch, TicketDetail, TicketFilter, TicketPatch, TicketScope,
    TicketService, WriteOptions,
};
use nullradix_app::{ChangeFeed, ChangeOp, ChangeSignal, Entity, SystemClock, UuidV7Generator};
use nullradix_domain::{Actor, Board, Column, Id, Priority, Project, Step, TicketStatus, Version};
use nullradix_store::Store;
use tokio::sync::watch::Receiver;

struct Fixture {
    tickets: TicketService,
    boards: BoardService,
    projects: ProjectService,
    person: Actor,
    store: Arc<Store>,
    head: Receiver<i64>,
}

const AGENT: WriteOptions = WriteOptions {
    by_person: false,
    force: false,
    allow_duplicate: false,
};
const PERSON: WriteOptions = WriteOptions {
    by_person: true,
    force: false,
    allow_duplicate: false,
};

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let head = signal.subscribe();
    let person = local_person(store.as_ref(), "anna", &UuidV7Generator, &SystemClock)
        .await
        .unwrap();
    let clock = Arc::new(SystemClock);
    let ids = Arc::new(UuidV7Generator);
    Fixture {
        tickets: TicketService::new(
            store.clone(),
            store.clone(),
            clock.clone(),
            ids.clone(),
            signal.clone(),
        ),
        boards: BoardService::new(
            store.clone(),
            store.clone(),
            store.clone(),
            clock.clone(),
            ids.clone(),
            signal.clone(),
        ),
        projects: ProjectService::new(store.clone(), clock, ids, signal),
        person,
        store,
        head,
    }
}

impl Fixture {
    async fn board(&self, key: &str) -> (Project, Board) {
        let project = self.projects.create(key, key).await.unwrap();
        let board = self
            .boards
            .create(project.id(), "Main", &self.person)
            .await
            .unwrap()
            .board;
        (project, board)
    }

    async fn create(
        &self,
        board: &Board,
        input: NewTicket,
        options: WriteOptions,
    ) -> Result<String, String> {
        self.tickets
            .create(board.project(), board.id(), &input, &self.person, options)
            .await
            .map(|view| view.ticket.key().to_string())
            .map_err(|error| error.code().to_owned())
    }

    async fn update(
        &self,
        key: &str,
        patch: TicketPatch,
        version: i64,
        options: WriteOptions,
    ) -> Result<TicketDetail, String> {
        self.tickets
            .update(
                key,
                &patch,
                &self.person,
                Version::parse(version).unwrap(),
                options,
            )
            .await
            .map_err(|error| error.code().to_owned())
    }

    /// The id of step `number` of the ticket, as a caller reads it before a change.
    async fn step(&self, key: &str, number: usize) -> Id<Step> {
        self.tickets.get(key).await.unwrap().steps[number - 1].id
    }

    async fn keys(&self, filter: TicketFilter) -> Vec<String> {
        self.tickets
            .list(&filter)
            .await
            .unwrap()
            .into_iter()
            .map(|view| view.ticket.key().to_string())
            .collect()
    }
}

fn titled(title: &str) -> NewTicket {
    NewTicket {
        title: title.to_owned(),
        ..NewTicket::default()
    }
}

#[tokio::test]
async fn new_tickets_get_the_next_number_and_the_legacy_defaults() {
    let mut fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    let (_, other) = fx.board("ZED").await;
    fx.head.borrow_and_update();

    let view = fx
        .tickets
        .create(
            board.project(),
            board.id(),
            &titled("  First  "),
            &fx.person,
            AGENT,
        )
        .await
        .unwrap();
    assert!(fx.head.has_changed().unwrap());
    let record = view.ticket.record();
    assert_eq!(record.key.to_string(), "ARC-0001");
    assert_eq!(record.title.as_str(), "First");
    assert_eq!(
        (record.status, record.priority, record.needs_planning),
        (TicketStatus::Backlog, Priority::Normal, true)
    );
    assert_eq!((record.sort_order, view.column), (0, Column::Backlog));
    assert_eq!(view.created_by_name, "anna");

    assert_eq!(
        fx.create(&board, titled("Second"), AGENT).await,
        Ok(String::from("ARC-0002"))
    );
    assert_eq!(
        fx.create(&other, titled("First"), AGENT).await,
        Ok(String::from("ZED-0001"))
    );
    let feed = fx.store.changes_since(0, 100).await.unwrap();
    let last = feed.last().unwrap();
    assert_eq!((last.entity, last.op), (Entity::Ticket, ChangeOp::Insert));

    let detail = fx.tickets.get("arc-1").await.unwrap();
    assert_eq!(detail.view, view);
    assert_eq!(detail.history.len(), 1);
    let created = &detail.history[0];
    assert_eq!(
        (
            created.field.as_str(),
            created.from.as_deref(),
            created.to.as_deref()
        ),
        ("status", None, Some("backlog"))
    );
    assert_eq!(
        (created.note.as_deref(), created.actor_name.as_str()),
        (Some("created"), "anna")
    );
    let boards = fx.boards.list(None).await.unwrap();
    assert_eq!(boards[0].open_tickets, 2);
}

/// Inputs refused on create, each with the code that names why.
fn refused_inputs() -> Vec<(NewTicket, &'static str)> {
    vec![
        (titled(" "), "title-empty"),
        (titled(&"x".repeat(401)), "value-too-long"),
        (
            NewTicket {
                labels: vec!["x".repeat(101)],
                ..titled("Labels")
            },
            "value-too-long",
        ),
        (
            NewTicket {
                depends_on: vec![String::from("nope")],
                ..titled("Bad key")
            },
            "ticket-id-invalid",
        ),
        (
            NewTicket {
                depends_on: vec![String::from("ARC-9")],
                ..titled("Unknown")
            },
            "dependency-unknown",
        ),
        (
            NewTicket {
                target_branch: Some(String::from("ticket/ARC-1")),
                ..titled("Branch")
            },
            "target-branch-invalid",
        ),
        (
            NewTicket {
                status: Some(String::from("not-needed")),
                ..titled("Unneeded")
            },
            "not-needed-human-only",
        ),
        (
            NewTicket {
                status: Some(String::from("waiting")),
                ..titled("Waiting")
            },
            "status-invalid",
        ),
        (
            NewTicket {
                priority: Some(String::from("critical")),
                ..titled("Critical")
            },
            "priority-invalid",
        ),
        (
            NewTicket {
                depends_on: vec![String::from("arc-1")],
                ..titled("Itself")
            },
            "dependency-self",
        ),
        (
            NewTicket {
                status: Some(String::from("done")),
                acceptance: Some(String::from("  ")),
                ..titled("Done")
            },
            "acceptance-missing",
        ),
    ]
}

#[tokio::test]
async fn refused_tickets_take_no_number_and_leave_no_trace() {
    let fx = fixture().await;
    let (project, board) = fx.board("ARC").await;
    let cases = refused_inputs();
    for (input, code) in cases {
        assert_eq!(fx.create(&board, input, AGENT).await, Err(code.to_owned()));
    }
    let missing: Id<Project> = Id::from_uuid(uuid::Uuid::from_u128(9));
    let error = fx
        .tickets
        .create(missing, board.id(), &titled("Lost"), &fx.person, AGENT)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "project-not-found");
    let (_, foreign) = fx.board("ZED").await;
    let error = fx
        .tickets
        .create(
            project.id(),
            foreign.id(),
            &titled("Lost"),
            &fx.person,
            AGENT,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), "board-not-found");

    assert_eq!(
        fx.create(&board, titled("Fine"), AGENT).await,
        Ok(String::from("ARC-0001"))
    );
    let tickets = fx.store.changes_since(0, 100).await.unwrap();
    assert_eq!(
        tickets
            .iter()
            .filter(|change| change.entity == Entity::Ticket)
            .count(),
        1
    );
}

#[tokio::test]
async fn open_tickets_with_the_same_title_are_refused_unless_allowed() {
    let fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    let (_, other) = fx.board("ZED").await;
    fx.create(&board, titled("Login page"), AGENT)
        .await
        .unwrap();

    let error = fx
        .tickets
        .create(
            board.project(),
            board.id(),
            &titled(" LOGIN page "),
            &fx.person,
            AGENT,
        )
        .await
        .unwrap_err();
    assert_eq!(
        (error.code(), error.existing()),
        ("ticket-duplicate", Some("ARC-0001"))
    );
    let allowed = WriteOptions {
        allow_duplicate: true,
        ..AGENT
    };
    assert!(
        fx.create(&board, titled("Login page"), allowed)
            .await
            .is_ok()
    );
    assert!(fx.create(&other, titled("Login page"), AGENT).await.is_ok());

    let done = NewTicket {
        status: Some(String::from("done")),
        acceptance: Some(String::from("It works")),
        ..titled("Logout")
    };
    fx.create(&board, done, PERSON).await.unwrap();
    assert!(fx.create(&board, titled("Logout"), AGENT).await.is_ok());
}

#[tokio::test]
async fn dependencies_decide_columns_locks_and_what_unlocks() {
    let fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    fx.create(&board, titled("Base"), AGENT).await.unwrap();
    let waiting = NewTicket {
        depends_on: vec![String::from("arc-1"), String::from("ARC-0001")],
        ..titled("On top")
    };
    fx.create(&board, waiting, AGENT).await.unwrap();

    let top = fx.tickets.get("ARC-2").await.unwrap().view;
    assert_eq!(top.ticket.record().depends_on.len(), 1);
    assert_eq!(top.column, Column::Waiting);
    let base = fx.tickets.get("ARC-1").await.unwrap().view;
    assert_eq!(top.depends_open, [base.ticket.key().clone()]);
    assert_eq!(base.dependents, [top.ticket.key().clone()]);
    assert_eq!(base.unlocks, base.dependents);

    let started = NewTicket {
        depends_on: vec![String::from("ARC-1")],
        status: Some(String::from("in-progress")),
        ..titled("Started")
    };
    assert_eq!(
        fx.create(&board, started.clone(), PERSON).await,
        Err(String::from("dependency-open"))
    );
    let forced = WriteOptions {
        force: true,
        ..AGENT
    };
    assert_eq!(
        fx.create(&board, started, forced).await,
        Ok(String::from("ARC-0003"))
    );
    let not_needed = NewTicket {
        status: Some(String::from("not-needed")),
        ..titled("Unneeded")
    };
    assert!(fx.create(&board, not_needed, PERSON).await.is_ok());
}

#[tokio::test]
async fn lists_filter_and_keep_the_display_order() {
    let fx = fixture().await;
    let (project, board) = fx.board("ARC").await;
    let (_, other) = fx.board("ZED").await;
    for (title, priority, labels) in [
        ("Low one", "low", vec![String::from("ui")]),
        ("Urgent one", "urgent", Vec::new()),
        ("Needle in text", "normal", vec![String::from("ui, api")]),
    ] {
        let input = NewTicket {
            priority: Some(priority.to_owned()),
            labels,
            ..titled(title)
        };
        fx.create(&board, input, AGENT).await.unwrap();
    }
    let planned = NewTicket {
        status: Some(String::from("planned")),
        needs_planning: Some(false),
        ..titled("Planned")
    };
    fx.create(&board, planned, AGENT).await.unwrap();
    fx.create(&other, titled("Elsewhere"), AGENT).await.unwrap();

    let all = fx.keys(TicketFilter::default()).await;
    assert_eq!(
        all,
        ["ARC-0002", "ZED-0001", "ARC-0003", "ARC-0001", "ARC-0004"]
    );
    let on_board = TicketScope {
        board: Some(board.id()),
        ..TicketScope::default()
    };
    let scoped = |filter: TicketFilter| TicketFilter {
        scope: on_board,
        ..filter
    };
    assert_eq!(fx.keys(scoped(TicketFilter::default())).await.len(), 4);
    let label = TicketFilter {
        label: Some(String::from("ui")),
        ..TicketFilter::default()
    };
    assert_eq!(fx.keys(scoped(label)).await, ["ARC-0003", "ARC-0001"]);
    let query = TicketFilter {
        query: Some(String::from("NEEDLE")),
        ..TicketFilter::default()
    };
    assert_eq!(fx.keys(query).await, ["ARC-0003"]);
    let by_key = TicketFilter {
        query: Some(String::from("zed-0001")),
        ..TicketFilter::default()
    };
    assert_eq!(fx.keys(by_key).await, ["ZED-0001"]);
    let columns = TicketFilter {
        columns: Some(vec![Column::Planned]),
        needs_planning: Some(false),
        ..TicketFilter::default()
    };
    assert_eq!(fx.keys(columns).await, ["ARC-0004"]);
    let in_project = TicketFilter {
        scope: TicketScope {
            project: Some(project.id()),
            board: None,
        },
        assignee: Some(String::from("nobody")),
        ..TicketFilter::default()
    };
    assert_eq!(fx.keys(in_project).await, Vec::<String>::new());

    let missing = TicketFilter {
        scope: TicketScope {
            board: Some(Id::from_uuid(uuid::Uuid::from_u128(9))),
            ..TicketScope::default()
        },
        ..TicketFilter::default()
    };
    let error = fx.tickets.list(&missing).await.unwrap_err();
    assert_eq!(error.code(), "board-not-found");
}

#[tokio::test]
async fn tickets_are_found_by_loose_keys_only() {
    let fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    fx.create(&board, titled("One"), AGENT).await.unwrap();
    assert!(fx.tickets.get(" arc-0001 ").await.is_ok());
    assert_eq!(
        fx.tickets.get("ARC-2").await.unwrap_err().code(),
        "ticket-not-found"
    );
    assert_eq!(
        fx.tickets.get("ARC").await.unwrap_err().code(),
        "ticket-id-invalid"
    );
}

#[tokio::test]
async fn boards_go_with_their_tickets_and_open_ones_only_when_asked() {
    let fx = fixture().await;
    let (project, board) = fx.board("ARC").await;
    let other = fx
        .boards
        .create(project.id(), "Other", &fx.person)
        .await
        .unwrap()
        .board;
    fx.create(&board, titled("Open"), AGENT).await.unwrap();
    let done = NewTicket {
        status: Some(String::from("done")),
        acceptance: Some(String::from("works")),
        ..titled("Done")
    };
    fx.create(&board, done, PERSON).await.unwrap();
    let waiting = NewTicket {
        depends_on: vec![String::from("ARC-1"), String::from("ARC-2")],
        ..titled("Elsewhere")
    };
    fx.create(&other, waiting, AGENT).await.unwrap();

    let error = fx
        .boards
        .delete(project.id(), board.id(), board.version(), false, &fx.person)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "board-has-open-tickets");
    assert!(error.message().contains("ARC-0001"));
    assert!(!error.message().contains("ARC-0002"));

    let deleted = fx
        .boards
        .delete(project.id(), board.id(), board.version(), true, &fx.person)
        .await
        .unwrap();
    let deleted: Vec<String> = deleted.iter().map(ToString::to_string).collect();
    assert_eq!(deleted, ["ARC-0001", "ARC-0002"]);
    assert_eq!(
        fx.tickets.get("ARC-1").await.unwrap_err().code(),
        "ticket-not-found"
    );
    let survivor = fx.tickets.get("ARC-3").await.unwrap();
    let record = survivor.view.ticket.record();
    assert_eq!((record.depends_on.len(), record.version.value()), (0, 3));
    let notes: Vec<Option<&str>> = survivor
        .history
        .iter()
        .map(|line| line.note.as_deref())
        .collect();
    assert_eq!(
        notes,
        [
            Some("created"),
            Some("dependency deleted"),
            Some("dependency deleted")
        ]
    );
    let boards = fx.boards.list(None).await.unwrap();
    assert_eq!(boards.len(), 1);

    let empty = fx
        .boards
        .create(project.id(), "Done only", &fx.person)
        .await
        .unwrap()
        .board;
    let finished = NewTicket {
        status: Some(String::from("not-needed")),
        ..titled("Dropped")
    };
    fx.create(&empty, finished, PERSON).await.unwrap();
    let gone = fx
        .boards
        .delete(project.id(), empty.id(), empty.version(), false, &fx.person)
        .await
        .unwrap();
    assert_eq!(gone.len(), 1);
}

fn patch() -> TicketPatch {
    TicketPatch::default()
}

#[tokio::test]
async fn updates_check_locks_and_versions_like_legacy() {
    let fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    fx.create(&board, titled("Base"), AGENT).await.unwrap();
    fx.create(&board, titled("Top"), AGENT).await.unwrap();

    let nothing = fx.update("ARC-1", patch(), 1, AGENT).await;
    assert_eq!(nothing.unwrap_err(), "nothing-changed");
    let stale = TicketPatch {
        title: Some(String::from("Again")),
        ..patch()
    };
    assert_eq!(
        fx.update("ARC-1", stale, 2, AGENT).await.unwrap_err(),
        "version-conflict"
    );
    let first = TicketPatch {
        depends_on: Some(vec![String::from("ARC-2")]),
        ..patch()
    };
    fx.update("ARC-1", first, 1, AGENT).await.unwrap();
    let back = TicketPatch {
        depends_on_add: Some(vec![String::from("ARC-1")]),
        ..patch()
    };
    assert_eq!(
        fx.update("ARC-2", back, 1, AGENT).await.unwrap_err(),
        "dependency-cycle"
    );
    let start = TicketPatch {
        status: Some(String::from("in-progress")),
        ..patch()
    };
    let locked = fx.update("ARC-1", start.clone(), 2, PERSON).await;
    assert_eq!(locked.unwrap_err(), "dependency-open");
    let forced = WriteOptions {
        force: true,
        ..AGENT
    };
    let started = fx.update("ARC-1", start, 2, forced).await.unwrap();
    let record = started.view.ticket.record();
    assert_eq!(
        (record.status, record.version.value()),
        (TicketStatus::InProgress, 3)
    );

    // legacy bumps the version even when nothing really changes
    let same = TicketPatch {
        title: Some(String::from("Base")),
        ..patch()
    };
    let bumped = fx.update("ARC-1", same, 3, AGENT).await.unwrap();
    assert_eq!(bumped.view.ticket.record().version.value(), 4);
    assert_eq!(bumped.history.len(), 3);
}

#[tokio::test]
async fn updates_write_fields_and_move_between_boards_of_one_project() {
    let fx = fixture().await;
    let (project, board) = fx.board("ARC").await;
    fx.create(&board, titled("Base"), AGENT).await.unwrap();
    let fields = TicketPatch {
        assignee: Some(Some(String::from(" claude "))),
        blocked_reason: Some(Some(String::from("waits"))),
        labels: Some(vec![String::from("ui, UI, api")]),
        description: Some(String::from("four")),
        target_branch: Some(Some(String::from("release/2"))),
        ..patch()
    };
    let detail = fx.update("ARC-1", fields, 1, AGENT).await.unwrap();
    let record = detail.view.ticket.record();
    assert_eq!(record.assignee.as_deref(), Some("claude"));
    assert_eq!(record.labels, ["ui", "api"]);
    let fields: Vec<&str> = detail.history[1..]
        .iter()
        .map(|line| line.field.as_str())
        .collect();
    assert_eq!(
        fields,
        [
            "assignee",
            "blockedReason",
            "targetBranch",
            "labels",
            "description"
        ]
    );
    let cleared = TicketPatch {
        assignee: Some(None),
        target_branch: Some(Some(String::from("  "))),
        ..patch()
    };
    let detail = fx.update("ARC-1", cleared, 2, AGENT).await.unwrap();
    let record = detail.view.ticket.record();
    assert_eq!(
        (record.assignee.as_ref(), record.target_branch.as_ref()),
        (None, None)
    );
    let long = TicketPatch {
        assignee: Some(Some("x".repeat(101))),
        ..patch()
    };
    assert_eq!(
        fx.update("ARC-1", long, 3, AGENT).await.unwrap_err(),
        "value-too-long"
    );

    let (_, foreign) = fx.board("ZED").await;
    let elsewhere = TicketPatch {
        board: Some(foreign.id()),
        ..patch()
    };
    let refused = fx.update("ARC-1", elsewhere, 3, AGENT).await;
    assert_eq!(refused.unwrap_err(), "board-other-project");
    let second = fx
        .boards
        .create(project.id(), "Second", &fx.person)
        .await
        .unwrap()
        .board;
    let moved = TicketPatch {
        board: Some(second.id()),
        ..patch()
    };
    let detail = fx.update("ARC-1", moved, 3, AGENT).await.unwrap();
    let record = detail.view.ticket.record();
    assert_eq!((record.board, record.sort_order), (second.id(), 0));
}

#[tokio::test]
async fn moves_clear_the_block_reason_and_skip_no_ops() {
    let fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    fx.create(&board, titled("One"), AGENT).await.unwrap();
    let blocked = TicketPatch {
        status: Some(String::from("blocked")),
        blocked_reason: Some(Some(String::from("waits"))),
        ..patch()
    };
    fx.tickets
        .update("ARC-1", &blocked, &fx.person, Version::INITIAL, AGENT)
        .await
        .unwrap();
    let move_to = |status: Option<&'static str>, acceptance: Option<&'static str>| {
        let fx = &fx;
        async move {
            fx.tickets
                .move_to("ARC-1", status, acceptance, &fx.person, None, PERSON)
                .await
                .map_err(|error| error.code().to_owned())
        }
    };
    assert_eq!(move_to(None, None).await.unwrap_err(), "status-missing");
    let same = move_to(Some("blocked"), None).await.unwrap();
    assert_eq!(same.ticket.record().version.value(), 2);
    assert_eq!(
        move_to(Some("done"), None).await.unwrap_err(),
        "acceptance-missing"
    );
    let done = move_to(Some("done"), Some("It works")).await.unwrap();
    let record = done.ticket.record();
    assert_eq!(
        (record.status, record.blocked_reason.as_ref()),
        (TicketStatus::Done, None)
    );
    assert_eq!(record.version.value(), 3);
    let history = fx.tickets.get("ARC-1").await.unwrap().history;
    let last = history.last().unwrap();
    assert_eq!(last.note.as_deref(), Some("moved"));
    let stale = fx
        .tickets
        .move_to(
            "ARC-1",
            Some("planned"),
            None,
            &fx.person,
            Some(Version::INITIAL),
            AGENT,
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
}

#[tokio::test]
async fn deleted_tickets_free_their_dependents_and_leave_the_numbers_unused() {
    let fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    fx.create(&board, titled("Base"), AGENT).await.unwrap();
    let top = NewTicket {
        depends_on: vec![String::from("ARC-1")],
        ..titled("Top")
    };
    fx.create(&board, top, AGENT).await.unwrap();

    let stale = fx
        .tickets
        .delete("ARC-1", &fx.person, Some(Version::INITIAL.next()))
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    fx.tickets.delete("arc-1", &fx.person, None).await.unwrap();
    assert_eq!(
        fx.tickets.get("ARC-1").await.unwrap_err().code(),
        "ticket-not-found"
    );
    let top = fx.tickets.get("ARC-2").await.unwrap();
    assert_eq!(top.view.column, Column::Backlog);
    let last = top.history.last().unwrap();
    assert_eq!(
        (
            last.field.as_str(),
            last.from.as_deref(),
            last.to.as_deref()
        ),
        ("dependsOn", Some(r#"["ARC-0001"]"#), Some("[]"))
    );
    assert_eq!(
        fx.create(&board, titled("Next"), AGENT).await,
        Ok(String::from("ARC-0003"))
    );
    let feed = fx.store.changes_since(0, 100).await.unwrap();
    let ops: Vec<ChangeOp> = feed
        .iter()
        .filter(|change| change.entity == Entity::Ticket)
        .map(|change| change.op)
        .collect();
    assert_eq!(
        ops,
        [
            ChangeOp::Insert,
            ChangeOp::Insert,
            ChangeOp::Update,
            ChangeOp::Delete,
            ChangeOp::Insert
        ]
    );
    assert_eq!(
        fx.tickets
            .delete("ARC-1", &fx.person, None)
            .await
            .unwrap_err()
            .code(),
        "ticket-not-found"
    );
}

#[tokio::test]
async fn checks_run_in_the_legacy_order_and_skip_blank_dependencies() {
    let fx = fixture().await;
    let (project, board) = fx.board("ARC").await;
    let first = NewTicket {
        depends_on: vec![String::new(), String::from(" ")],
        ..titled("Login page")
    };
    assert_eq!(
        fx.create(&board, first, AGENT).await,
        Ok(String::from("ARC-0001"))
    );
    let blank_entries = NewTicket {
        depends_on: vec![String::from("ARC-1"), String::new()],
        ..titled("Logout")
    };
    assert_eq!(
        fx.create(&board, blank_entries, AGENT).await,
        Ok(String::from("ARC-0002"))
    );

    let duplicate_first = NewTicket {
        labels: vec!["x".repeat(101)],
        ..titled("login PAGE")
    };
    assert_eq!(
        fx.create(&board, duplicate_first, AGENT).await,
        Err(String::from("ticket-duplicate"))
    );
    let dependency_before_branch = NewTicket {
        depends_on: vec![String::from("ARC-99")],
        target_branch: Some(String::from("ticket")),
        ..titled("Lost")
    };
    assert_eq!(
        fx.create(&board, dependency_before_branch, AGENT).await,
        Err(String::from("dependency-unknown"))
    );
    let missing: Id<Board> = Id::from_uuid(uuid::Uuid::from_u128(9));
    let error = fx
        .tickets
        .create(project.id(), missing, &titled(" "), &fx.person, AGENT)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "board-not-found");
    let other_column = NewTicket {
        status: Some(String::from("planned")),
        ..titled("Planned")
    };
    fx.create(&board, other_column, AGENT).await.unwrap();
    let mut orders = Vec::new();
    for key in ["ARC-2", "ARC-3"] {
        let view = fx.tickets.get(key).await.unwrap().view;
        orders.push(view.ticket.record().sort_order);
    }
    assert_eq!(orders, [1, 0]);
}

#[tokio::test]
async fn updates_clear_the_block_reason_and_lock_not_needed_like_legacy() {
    let fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    fx.create(&board, titled("One"), AGENT).await.unwrap();
    fx.create(&board, titled("Two"), AGENT).await.unwrap();
    let blocked = TicketPatch {
        status: Some(String::from("blocked")),
        blocked_reason: Some(Some(String::from("waits"))),
        ..patch()
    };
    fx.update("ARC-1", blocked, 1, AGENT).await.unwrap();
    let kept = TicketPatch {
        status: Some(String::from("planned")),
        blocked_reason: Some(Some(String::from("still noted"))),
        ..patch()
    };
    let detail = fx.update("ARC-1", kept, 2, AGENT).await.unwrap();
    assert_eq!(
        detail.view.ticket.record().blocked_reason.as_deref(),
        Some("still noted")
    );
    let again = TicketPatch {
        status: Some(String::from("blocked")),
        ..patch()
    };
    fx.update("ARC-1", again, 3, AGENT).await.unwrap();
    let left = TicketPatch {
        status: Some(String::from("backlog")),
        ..patch()
    };
    let detail = fx.update("ARC-1", left, 4, AGENT).await.unwrap();
    let record = detail.view.ticket.record();
    assert_eq!(record.blocked_reason, None);
    // ARC-2 holds place 1 in the backlog, so ARC-1 comes after it
    assert_eq!(record.sort_order, 2);

    let unneeded = TicketPatch {
        status: Some(String::from("not-needed")),
        ..patch()
    };
    let refused = fx.update("ARC-2", unneeded.clone(), 1, AGENT).await;
    assert_eq!(refused.unwrap_err(), "not-needed-human-only");
    fx.update("ARC-2", unneeded, 1, PERSON).await.unwrap();
    let moved = fx
        .tickets
        .move_to("ARC-1", Some("not-needed"), None, &fx.person, None, AGENT)
        .await
        .unwrap_err();
    assert_eq!(moved.code(), "not-needed-human-only");
}

#[tokio::test]
async fn dependency_lists_replace_add_and_remove_in_the_legacy_order() {
    let fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    for title in ["A", "B", "C", "Top"] {
        fx.create(&board, titled(title), AGENT).await.unwrap();
    }
    let keys = |detail: &TicketDetail| -> Vec<String> {
        detail
            .view
            .ticket
            .record()
            .depends_on
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    let joined = TicketPatch {
        depends_on: Some(vec![String::from("ARC-1, ARC-2")]),
        ..patch()
    };
    let refused = fx.update("ARC-4", joined, 1, AGENT).await;
    assert_eq!(refused.unwrap_err(), "ticket-id-invalid");
    let replaced = TicketPatch {
        depends_on: Some(vec![
            String::from("ARC-1"),
            String::from("ARC-2"),
            String::from("ARC-3"),
        ]),
        depends_on_remove: Some(vec![String::from("arc-2")]),
        ..patch()
    };
    let detail = fx.update("ARC-4", replaced, 1, AGENT).await.unwrap();
    assert_eq!(keys(&detail), ["ARC-0001", "ARC-0003"]);
    let added = TicketPatch {
        depends_on_add: Some(vec![String::from("ARC-2")]),
        ..patch()
    };
    let detail = fx.update("ARC-4", added, 2, AGENT).await.unwrap();
    assert_eq!(keys(&detail), ["ARC-0001", "ARC-0003", "ARC-0002"]);

    fx.tickets.delete("ARC-1", &fx.person, None).await.unwrap();
    let top = fx.tickets.get("ARC-4").await.unwrap();
    let last = top.history.last().unwrap();
    assert_eq!(
        (last.from.as_deref(), last.to.as_deref()),
        (
            Some(r#"["ARC-0003","ARC-0002","ARC-0001"]"#),
            Some(r#"["ARC-0003","ARC-0002"]"#)
        )
    );
}

fn guard(version: i64) -> StepGuard {
    StepGuard {
        version: Some(Version::parse(version).unwrap()),
        expect_text: None,
    }
}

#[tokio::test]
async fn steps_start_with_the_ticket_and_count_on_the_board() {
    let fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    let blank = NewTicket {
        steps: vec![String::from("Write"), String::from(" ")],
        ..titled("Blank step")
    };
    assert_eq!(
        fx.create(&board, blank, AGENT).await,
        Err(String::from("step-empty"))
    );
    let input = NewTicket {
        steps: vec![String::from(" Write "), String::from("Test")],
        ..titled("Stepped")
    };
    fx.create(&board, input, AGENT).await.unwrap();
    let detail = fx.tickets.get("ARC-1").await.unwrap();
    let texts: Vec<&str> = detail.steps.iter().map(|step| step.text.as_str()).collect();
    assert_eq!(texts, ["Write", "Test"]);
    assert_eq!((detail.view.steps.done, detail.view.steps.total), (0, 2));
    assert_eq!(detail.history.len(), 1);

    let checked = fx
        .tickets
        .change_step(
            "ARC-1",
            fx.step("ARC-1", 2).await,
            &StepPatch {
                done: Some(true),
                ..StepPatch::default()
            },
            &guard(1),
            &fx.person,
        )
        .await
        .unwrap();
    assert_eq!(checked.view.ticket.record().version.value(), 2);
    let line = checked.history.last().unwrap();
    assert_eq!(
        (
            line.field.as_str(),
            line.from.as_deref(),
            line.to.as_deref(),
            line.note.as_deref()
        ),
        ("step", Some("2: open"), Some("2: done"), Some("Test"))
    );
    let listed = fx.tickets.list(&TicketFilter::default()).await.unwrap();
    assert_eq!((listed[0].steps.done, listed[0].steps.total), (1, 2));
}

#[tokio::test]
async fn step_changes_follow_the_legacy_guards() {
    let fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    let input = NewTicket {
        steps: vec![String::from("a"), String::from("b")],
        ..titled("Stepped")
    };
    fx.create(&board, input, AGENT).await.unwrap();
    let none = StepGuard::default();
    let code = |result: Result<TicketDetail, nullradix_app::AppError>| {
        result.map(|_| ()).map_err(|error| error.code().to_owned())
    };

    let added = fx
        .tickets
        .add_step("ARC-1", "z", Some("how"), Some(1), &none, &fx.person)
        .await
        .unwrap();
    let texts: Vec<&str> = added.steps.iter().map(|step| step.text.as_str()).collect();
    assert_eq!(texts, ["z", "a", "b"]);
    assert_eq!(added.steps[0].detail.as_deref(), Some("how"));
    let far = fx
        .tickets
        .add_step("ARC-1", "x", None, Some(5), &none, &fx.person)
        .await;
    assert_eq!(code(far), Err(String::from("step-not-found")));
    let stale = fx
        .tickets
        .add_step("ARC-1", "x", None, None, &guard(1), &fx.person)
        .await;
    assert_eq!(code(stale), Err(String::from("version-conflict")));
    let moved_away = StepGuard {
        version: None,
        expect_text: Some(String::from("a")),
    };
    let changed = fx
        .tickets
        .remove_step("ARC-1", fx.step("ARC-1", 1).await, &moved_away, &fx.person)
        .await;
    assert_eq!(code(changed), Err(String::from("conflict")));

    let nothing = fx
        .tickets
        .change_step(
            "ARC-1",
            fx.step("ARC-1", 1).await,
            &StepPatch::default(),
            &none,
            &fx.person,
        )
        .await;
    assert_eq!(code(nothing), Err(String::from("nothing-changed")));
    let both = StepPatch {
        text: Some(String::from("zz")),
        detail: Some(None),
        done: Some(true),
    };
    let edited = fx
        .tickets
        .change_step("ARC-1", fx.step("ARC-1", 1).await, &both, &none, &fx.person)
        .await
        .unwrap();
    assert_eq!(edited.view.ticket.record().version.value(), 3);
    let notes: Vec<Option<&str>> = edited.history[2..]
        .iter()
        .map(|line| line.note.as_deref())
        .collect();
    assert_eq!(notes, [Some("step edited"), Some("zz")]);
    assert_eq!(
        (edited.steps[0].detail.as_ref(), edited.steps[0].done),
        (None, true)
    );

    let again = StepPatch {
        done: Some(true),
        ..StepPatch::default()
    };
    let same = fx
        .tickets
        .change_step(
            "ARC-1",
            fx.step("ARC-1", 1).await,
            &again,
            &none,
            &fx.person,
        )
        .await
        .unwrap();
    assert_eq!(same.view.ticket.record().version.value(), 3);
}

#[tokio::test]
async fn steps_go_by_id_whatever_their_number() {
    let fx = fixture().await;
    let (_, board) = fx.board("ARC").await;
    let input = NewTicket {
        steps: vec![String::from("a"), String::from("b"), String::from("c")],
        ..titled("Stepped")
    };
    fx.create(&board, input, AGENT).await.unwrap();
    let none = StepGuard::default();
    let first = fx.step("ARC-1", 1).await;
    let moved = fx
        .tickets
        .move_step("ARC-1", first, 3, &none, &fx.person)
        .await
        .unwrap();
    let texts: Vec<&str> = moved.steps.iter().map(|step| step.text.as_str()).collect();
    assert_eq!(texts, ["b", "c", "a"]);
    assert_eq!(moved.steps[2].id, first);
    let checked = fx
        .tickets
        .change_step(
            "ARC-1",
            first,
            &StepPatch {
                done: Some(true),
                ..StepPatch::default()
            },
            &none,
            &fx.person,
        )
        .await
        .unwrap();
    let line = checked.history.last().unwrap();
    assert_eq!(line.from.as_deref(), Some("3: open"));
    let removed = fx
        .tickets
        .remove_step("ARC-1", first, &none, &fx.person)
        .await
        .unwrap();
    assert_eq!(removed.steps.len(), 2);
    assert_eq!(
        removed.history.last().unwrap().from.as_deref(),
        Some("3: a")
    );

    let gone = fx
        .tickets
        .change_step("ARC-1", first, &StepPatch::default(), &none, &fx.person)
        .await
        .unwrap_err();
    assert_eq!(gone.code(), "step-not-found");
    let missing = fx
        .tickets
        .change_step("ARC-9", first, &StepPatch::default(), &none, &fx.person)
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "ticket-not-found");
    let stale = StepGuard {
        version: Some(Version::INITIAL),
        expect_text: None,
    };
    let second = fx.step("ARC-1", 1).await;
    let outdated = fx
        .tickets
        .change_step("ARC-1", second, &StepPatch::default(), &stale, &fx.person)
        .await
        .unwrap_err();
    assert_eq!(outdated.code(), "version-conflict");
    let changed = StepGuard {
        version: None,
        expect_text: Some(String::from("z")),
    };
    let conflict = fx
        .tickets
        .change_step("ARC-1", second, &StepPatch::default(), &changed, &fx.person)
        .await
        .unwrap_err();
    assert_eq!(conflict.code(), "conflict");
}
