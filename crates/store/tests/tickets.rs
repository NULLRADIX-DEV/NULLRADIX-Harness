#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;

use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::projects::ProjectService;
use nullradix_app::tickets::{NewTicket, TicketFilter, TicketScope, TicketService, WriteOptions};
use nullradix_app::{ChangeFeed, ChangeOp, ChangeSignal, Entity, SystemClock, UuidV7Generator};
use nullradix_domain::{Actor, Board, Column, Id, Priority, Project, TicketStatus};
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

#[tokio::test]
async fn refused_tickets_take_no_number_and_leave_no_trace() {
    let fx = fixture().await;
    let (project, board) = fx.board("ARC").await;
    let cases = [
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
                status: Some(TicketStatus::NotNeeded),
                ..titled("Unneeded")
            },
            "not-needed-human-only",
        ),
        (
            NewTicket {
                status: Some(TicketStatus::Done),
                acceptance: Some(String::from("  ")),
                ..titled("Done")
            },
            "acceptance-missing",
        ),
    ];
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
        status: Some(TicketStatus::Done),
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
        status: Some(TicketStatus::InProgress),
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
        status: Some(TicketStatus::NotNeeded),
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
        ("Low one", Priority::Low, vec![String::from("ui")]),
        ("Urgent one", Priority::Urgent, Vec::new()),
        (
            "Needle in text",
            Priority::Normal,
            vec![String::from("ui, api")],
        ),
    ] {
        let input = NewTicket {
            priority: Some(priority),
            labels,
            ..titled(title)
        };
        fx.create(&board, input, AGENT).await.unwrap();
    }
    let planned = NewTicket {
        status: Some(TicketStatus::Planned),
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
async fn boards_with_tickets_stay_until_the_tickets_are_gone() {
    let fx = fixture().await;
    let (project, board) = fx.board("ARC").await;
    fx.create(&board, titled("One"), AGENT).await.unwrap();
    let error = fx
        .boards
        .delete(project.id(), board.id(), board.version())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "board-has-tickets");
}
