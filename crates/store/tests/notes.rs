#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;

use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::comments::CommentService;
use nullradix_app::notes::{Author, NewNote, NoteRow, NoteService, Promotion};
use nullradix_app::projects::ProjectService;
use nullradix_app::tickets::{NewTicket, TicketService, WriteOptions};
use nullradix_app::{
    AppError, ChangeFeed, ChangeOp, ChangeSignal, Entity, SystemClock, UuidV7Generator,
};
use nullradix_domain::{Actor, ActorKind, Board, Id, NoteState, Priority, TicketStatus, Version};
use nullradix_store::Store;

struct Fixture {
    notes: NoteService,
    tickets: Arc<TicketService>,
    boards: BoardService,
    projects: ProjectService,
    person: Actor,
    store: Arc<Store>,
}

const AGENT: Author<'static> = Author {
    by_person: false,
    name: None,
    session: None,
};

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let person = local_person(store.as_ref(), "anna", &UuidV7Generator, &SystemClock)
        .await
        .unwrap();
    let clock = Arc::new(SystemClock);
    let ids = Arc::new(UuidV7Generator);
    let tickets = Arc::new(TicketService::new(
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    ));
    Fixture {
        notes: NoteService::new(
            store.clone(),
            tickets.clone(),
            store.clone(),
            store.clone(),
            clock.clone(),
            ids.clone(),
            signal.clone(),
        ),
        tickets,
        boards: BoardService::new(
            store.clone(),
            store.clone(),
            store.clone(),
            common::runners(&store, &signal).await,
            clock.clone(),
            ids.clone(),
            signal.clone(),
        ),
        projects: ProjectService::new(store.clone(), clock, ids, signal),
        person,
        store,
    }
}

fn noted(title: &str) -> NewNote {
    NewNote {
        title: title.to_owned(),
        ..NewNote::default()
    }
}

fn code<T>(result: Result<T, AppError>) -> String {
    result
        .err()
        .map_or_else(String::new, |error| error.code().to_owned())
}

fn numbers(rows: &[NoteRow]) -> Vec<i64> {
    rows.iter().map(|row| row.note.number).collect()
}

impl Fixture {
    async fn board(&self, key: &str) -> Board {
        let project = self.projects.create(key, key).await.unwrap();
        self.boards
            .create(project.id(), "Main", &self.person)
            .await
            .unwrap()
            .board
    }

    async fn ticket(&self, board: &Board, title: &str) -> String {
        let input = NewTicket {
            title: title.to_owned(),
            ..NewTicket::default()
        };
        self.tickets
            .create(
                board.project(),
                board.id(),
                &input,
                &self.person,
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .ticket
            .key()
            .to_string()
    }

    async fn add(&self, board: &Board, input: NewNote) -> Result<NoteRow, AppError> {
        self.notes
            .add(board.project(), board.id(), &input, AGENT, &self.person)
            .await
    }

    async fn list(&self, board: &Board, state: Option<&str>) -> Vec<i64> {
        numbers(
            &self
                .notes
                .list(board.project(), board.id(), state, None)
                .await
                .unwrap(),
        )
    }
}

#[tokio::test]
async fn adds_open_notes_with_numbers_labels_and_authors() {
    let f = fixture().await;
    let board = f.board("ARC").await;
    let key = f.ticket(&board, "Login").await;
    let input = NewNote {
        title: "  Cache of the board view never expires ".to_owned(),
        body: Some("  BoardView keeps the old column. ".to_owned()),
        source: Some(key.to_lowercase()),
        labels: vec![String::from("cache, ui"), String::from("UI")],
        force: false,
    };
    let row = f
        .notes
        .add(
            board.project(),
            board.id(),
            &input,
            Author {
                by_person: false,
                name: Some("claude"),
                session: None,
            },
            &f.person,
        )
        .await
        .unwrap();
    let note = &row.note;
    assert_eq!(note.number, 1);
    assert_eq!(note.title, "Cache of the board view never expires");
    assert_eq!(note.body, "BoardView keeps the old column.");
    assert_eq!(note.labels, ["cache", "ui"]);
    assert_eq!(note.state, NoteState::Open);
    assert_eq!(row.author_name, "claude");
    assert_eq!(
        note.source.as_ref().map(|source| source.key.to_string()),
        Some(key)
    );
    assert_eq!(f.notes.get("1").await.unwrap(), row);
    let second = f.add(&board, noted("Another finding")).await.unwrap();
    assert_eq!(second.note.number, 2);
    assert_eq!(second.author_name, "anna");
    let changes = f.store.changes_since(0, 100).await.unwrap();
    let notes: Vec<(Entity, ChangeOp)> = changes
        .iter()
        .filter(|change| change.entity == Entity::Note)
        .map(|change| (change.entity, change.op))
        .collect();
    assert_eq!(notes, [(Entity::Note, ChangeOp::Insert); 2]);
}

#[tokio::test]
async fn refuses_bad_titles_labels_boards_and_sources() {
    let f = fixture().await;
    let board = f.board("ARC").await;
    let other = f.board("ZED").await;
    let foreign = f.ticket(&other, "Elsewhere").await;
    assert_eq!(code(f.add(&board, noted("  ")).await), "note-empty");
    assert_eq!(
        code(f.add(&board, noted("one\ntwo")).await),
        "note-title-multiline"
    );
    assert_eq!(
        code(f.add(&board, noted(&"a".repeat(201))).await),
        "value-too-long"
    );
    let labels = NewNote {
        labels: (0..11)
            .map(|index| format!("{index:02}{}", "a".repeat(98)))
            .collect(),
        ..noted("Labels")
    };
    assert_eq!(code(f.add(&board, labels).await), "value-too-long");
    let source = |text: &str| NewNote {
        source: Some(text.to_owned()),
        ..noted("Sourced")
    };
    assert_eq!(
        code(f.add(&board, source("nope")).await),
        "ticket-id-invalid"
    );
    assert_eq!(
        code(f.add(&board, source("ARC-9")).await),
        "ticket-not-found"
    );
    let error = f.add(&board, source(&foreign)).await.unwrap_err();
    assert_eq!(error.code(), "note-source-other-project");
    assert!(error.message().contains(&format!("--board {}", other.id())));
    let wrong = f
        .notes
        .add(other.project(), board.id(), &noted("x"), AGENT, &f.person)
        .await;
    assert_eq!(code(wrong), "board-not-found");
    let missing = f
        .notes
        .add(
            Id::from_uuid(uuid_of(9)),
            board.id(),
            &noted("x"),
            AGENT,
            &f.person,
        )
        .await;
    assert_eq!(code(missing), "project-not-found");
    assert_eq!(f.list(&board, Some("all")).await, Vec::<i64>::new());
}

fn uuid_of(value: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(value)
}

#[tokio::test]
async fn similar_open_or_promoted_notes_refuse_a_new_one_unless_forced() {
    let f = fixture().await;
    let board = f.board("ARC").await;
    let key = f.ticket(&board, "Origin").await;
    f.add(
        &board,
        NewNote {
            body: Some("WorktreeManager creates the branch twice".to_owned()),
            ..noted("Dispatcher starts the worktree twice")
        },
    )
    .await
    .unwrap();
    let again = NewNote {
        source: Some(key.clone()),
        ..noted("WorktreeManager: branch twice, dispatcher")
    };
    let error = f.add(&board, again.clone()).await.unwrap_err();
    assert_eq!(error.code(), "note-similar");
    assert_eq!(error.similar().len(), 1);
    assert_eq!(error.similar()[0].number, 1);
    assert_eq!(error.similar()[0].state, NoteState::Open);
    assert!(
        error
            .message()
            .contains(&format!("nullradix note seen 1 --from {key}")),
        "{}",
        error.message()
    );
    let forced = f
        .add(
            &board,
            NewNote {
                force: true,
                ..again.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(forced.note.number, 2);
    // a dropped note no longer counts, a promoted one does
    f.notes.drop_note("1", "duplicate").await.unwrap();
    let promoted = f
        .notes
        .promote("2", &Promotion::default(), AGENT, &f.person)
        .await
        .unwrap();
    let error = f.add(&board, again).await.unwrap_err();
    assert_eq!(error.code(), "note-similar");
    assert_eq!(numbers_of(&error), [2]);
    assert_eq!(
        error.similar()[0].ticket.as_deref(),
        Some(promoted.ticket.ticket.key().to_string().as_str())
    );
    // another board has its own notes
    let other = f.board("ZED").await;
    let elsewhere = f
        .add(&other, noted("WorktreeManager: branch twice, dispatcher"))
        .await;
    assert!(elsewhere.is_ok());
}

fn numbers_of(error: &AppError) -> Vec<i64> {
    error.similar().iter().map(|note| note.number).collect()
}

#[tokio::test]
async fn seen_counts_sightings_and_remembers_other_tickets() {
    let f = fixture().await;
    let board = f.board("ARC").await;
    let first = f.ticket(&board, "First").await;
    let second = f.ticket(&board, "Second").await;
    f.add(
        &board,
        NewNote {
            source: Some(first.clone()),
            ..noted("Cache never expires")
        },
    )
    .await
    .unwrap();
    f.notes.seen("1", Some(&first)).await.unwrap();
    f.notes
        .seen("1", Some(&second.to_lowercase()))
        .await
        .unwrap();
    f.notes.seen("1", None).await.unwrap();
    let row = f.notes.seen("1", Some(&second)).await.unwrap();
    assert_eq!(row.note.seen_count(), 5);
    let seen: Vec<String> = row.note.seen_in().iter().map(ToString::to_string).collect();
    assert_eq!(seen, std::slice::from_ref(&second));
    assert_eq!(row.note.version, Version::from_stored(5));
    assert_eq!(f.notes.get("1").await.unwrap(), row);
    assert_eq!(code(f.notes.seen("7", None).await), "note-not-found");
    assert_eq!(code(f.notes.seen("x", None).await), "note-id-invalid");
    assert_eq!(
        code(f.notes.seen("1", Some("ARC-77")).await),
        "ticket-not-found"
    );
    let other = f.board("ZED").await;
    let foreign = f.ticket(&other, "Elsewhere").await;
    assert_eq!(
        code(f.notes.seen("1", Some(&foreign)).await),
        "note-source-other-project"
    );
    f.notes.drop_note("1", "not worth it").await.unwrap();
    let error = f.notes.seen("1", None).await.unwrap_err();
    assert_eq!(error.code(), "note-not-open");
    assert_eq!(error.message(), "note 1 is dropped (not worth it)");
    f.add(&board, noted("Login is slow")).await.unwrap();
    let promoted = f
        .notes
        .promote("2", &Promotion::default(), AGENT, &f.person)
        .await
        .unwrap();
    let error = f.notes.seen("2", None).await.unwrap_err();
    assert_eq!(error.code(), "note-promoted");
    assert_eq!(
        error.message(),
        format!(
            "note 2 is already promoted to {}; comment there",
            promoted.ticket.ticket.key()
        )
    );
}

#[tokio::test]
async fn lists_by_state_and_source_in_number_order() {
    let f = fixture().await;
    let board = f.board("ARC").await;
    let key = f.ticket(&board, "Origin").await;
    f.add(&board, noted("Alpha finding")).await.unwrap();
    f.add(
        &board,
        NewNote {
            source: Some(key.clone()),
            ..noted("Bravo finding")
        },
    )
    .await
    .unwrap();
    f.add(&board, noted("Charlie finding")).await.unwrap();
    f.add(&board, noted("Delta finding")).await.unwrap();
    f.notes.drop_note("1", "gone").await.unwrap();
    f.notes
        .promote("3", &Promotion::default(), AGENT, &f.person)
        .await
        .unwrap();
    assert_eq!(f.list(&board, None).await, [2, 4]);
    assert_eq!(f.list(&board, Some("open")).await, [2, 4]);
    assert_eq!(f.list(&board, Some("promoted")).await, [3]);
    assert_eq!(f.list(&board, Some("dropped")).await, [1]);
    assert_eq!(f.list(&board, Some("all")).await, [1, 2, 3, 4]);
    let sourced = f
        .notes
        .list(
            board.project(),
            board.id(),
            Some("all"),
            Some(&key.to_lowercase()),
        )
        .await
        .unwrap();
    assert_eq!(numbers(&sourced), [2]);
    let invalid = f
        .notes
        .list(board.project(), board.id(), Some("taken"), None)
        .await;
    assert_eq!(code(invalid), "state-invalid");
    let invalid = f
        .notes
        .list(board.project(), board.id(), None, Some("x"))
        .await;
    assert_eq!(code(invalid), "ticket-id-invalid");
    let other = f.board("ZED").await;
    assert_eq!(f.list(&other, Some("all")).await, Vec::<i64>::new());
    let wrong = f.notes.list(other.project(), board.id(), None, None).await;
    assert_eq!(code(wrong), "board-not-found");
}

#[tokio::test]
async fn promotion_makes_a_ticket_in_the_same_transaction() {
    let f = fixture().await;
    let board = f.board("ARC").await;
    let origin = f.ticket(&board, "Origin").await;
    let input = NewNote {
        body: Some("Expires never.".to_owned()),
        source: Some(origin.clone()),
        labels: vec![String::from("cache")],
        ..noted("Cache never expires")
    };
    let created = f.add(&board, input).await.unwrap();
    f.notes.seen("1", None).await.unwrap();
    let promoted = f
        .notes
        .promote(
            "1",
            &Promotion {
                priority: Some(String::from("high")),
                ..Promotion::default()
            },
            Author {
                by_person: false,
                name: Some("claude"),
                session: None,
            },
            &f.person,
        )
        .await
        .unwrap();
    let ticket = promoted.ticket.ticket.record();
    assert_eq!(ticket.title.as_str(), "Cache never expires");
    assert_eq!(
        ticket.description,
        format!(
            "Expires never.\n\n(note of {}, seen at {origin}; seen 2 times in total)",
            created.note.created_at
        )
    );
    assert_eq!(ticket.status, TicketStatus::Backlog);
    assert_eq!(ticket.priority, Priority::High);
    assert_eq!(ticket.labels, ["cache"]);
    assert!(ticket.needs_planning);
    assert_eq!(promoted.ticket.created_by_name, "claude");
    let note = &promoted.note.note;
    assert_eq!(note.state, NoteState::Promoted);
    assert_eq!(
        note.promoted_to.as_ref().map(|ticket| &ticket.key),
        Some(promoted.ticket.ticket.key())
    );
    assert_eq!(f.notes.get("1").await.unwrap(), promoted.note);
    let error = f
        .notes
        .promote("1", &Promotion::default(), AGENT, &f.person)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "note-not-open");
    assert_eq!(
        error.message(),
        format!(
            "note 1 is already promoted to {}",
            promoted.ticket.ticket.key()
        )
    );
    let changes = f.store.changes_since(0, 100).await.unwrap();
    let last: Vec<Entity> = changes
        .iter()
        .rev()
        .take(2)
        .map(|change| change.entity)
        .collect();
    assert_eq!(last, [Entity::Note, Entity::Ticket]);
}

#[tokio::test]
async fn promotion_takes_other_values_and_rolls_back_on_locks() {
    let f = fixture().await;
    let board = f.board("ARC").await;
    f.add(
        &board,
        NewNote {
            labels: vec![String::from("old")],
            ..noted("Search is slow")
        },
    )
    .await
    .unwrap();
    let locked = Promotion {
        status: Some(String::from("not-needed")),
        ..Promotion::default()
    };
    let error = f.notes.promote("1", &locked, AGENT, &f.person).await;
    assert_eq!(code(error), "not-needed-human-only");
    let bad = Promotion {
        status: Some(String::from("later")),
        ..Promotion::default()
    };
    assert_eq!(
        code(f.notes.promote("1", &bad, AGENT, &f.person).await),
        "status-invalid"
    );
    let blank = Promotion {
        title: Some(String::from("  ")),
        ..Promotion::default()
    };
    assert_eq!(
        code(f.notes.promote("1", &blank, AGENT, &f.person).await),
        "title-empty"
    );
    assert_eq!(f.list(&board, None).await, [1]);
    let custom = Promotion {
        title: Some(String::from("Speed up search")),
        status: Some(String::from("planned")),
        labels: Some(vec![String::from("search, perf")]),
        priority: None,
    };
    let promoted = f
        .notes
        .promote("1", &custom, AGENT, &f.person)
        .await
        .unwrap();
    let ticket = promoted.ticket.ticket.record();
    assert_eq!(ticket.title.as_str(), "Speed up search");
    assert_eq!(ticket.status, TicketStatus::Planned);
    assert_eq!(ticket.labels, ["search", "perf"]);
    assert_eq!(ticket.key.to_string(), "ARC-0001");
    assert_eq!(promoted.ticket.created_by_name, "anna");
}

#[tokio::test]
async fn drop_needs_a_reason_and_leaves_promoted_notes_alone() {
    let f = fixture().await;
    let board = f.board("ARC").await;
    f.add(&board, noted("Flaky test")).await.unwrap();
    assert_eq!(code(f.notes.drop_note("1", " ").await), "reason-missing");
    assert_eq!(
        code(f.notes.drop_note("1", &"a".repeat(1001)).await),
        "value-too-long"
    );
    assert_eq!(code(f.notes.drop_note("0", "x").await), "note-id-invalid");
    assert_eq!(code(f.notes.drop_note("5", "x").await), "note-not-found");
    let dropped = f.notes.drop_note("1", " fixed with ARC-1 ").await.unwrap();
    assert_eq!(dropped.note.state, NoteState::Dropped);
    assert_eq!(dropped.note.reason.as_deref(), Some("fixed with ARC-1"));
    let again = f.notes.drop_note("1", "really gone").await.unwrap();
    assert_eq!(again.note.reason.as_deref(), Some("really gone"));
    assert_eq!(again.note.version, Version::from_stored(3));
    let error = f
        .notes
        .promote("1", &Promotion::default(), AGENT, &f.person)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "note-not-open");
    assert_eq!(error.message(), "note 1 is dropped");
    f.add(&board, noted("Another one")).await.unwrap();
    let promoted = f
        .notes
        .promote("2", &Promotion::default(), AGENT, &f.person)
        .await
        .unwrap();
    let error = f.notes.drop_note("2", "no").await.unwrap_err();
    assert_eq!(error.code(), "note-promoted");
    assert_eq!(
        error.message(),
        format!(
            "note 2 is already promoted to {}",
            promoted.ticket.ticket.key()
        )
    );
}

#[tokio::test]
async fn notes_keep_ticket_keys_after_the_tickets_are_deleted() {
    let f = fixture().await;
    let board = f.board("ARC").await;
    let origin = f.ticket(&board, "Origin").await;
    f.add(
        &board,
        NewNote {
            source: Some(origin.clone()),
            ..noted("Cache never expires")
        },
    )
    .await
    .unwrap();
    let promoted = f
        .notes
        .promote("1", &Promotion::default(), AGENT, &f.person)
        .await
        .unwrap();
    let made = promoted.ticket.ticket.key().to_string();
    f.tickets
        .delete(&origin, &f.person, None, false)
        .await
        .unwrap();
    f.tickets
        .delete(&made, &f.person, None, false)
        .await
        .unwrap();
    let note = f.notes.get("1").await.unwrap().note;
    let source = note.source.unwrap();
    assert_eq!(source.key.to_string(), origin);
    assert_eq!(source.id, None);
    let to = note.promoted_to.unwrap();
    assert_eq!(to.key.to_string(), made);
    assert_eq!(to.id, None);
    // the board takes its notes along
    f.boards
        .delete(
            board.project(),
            board.id(),
            board.version(),
            true,
            &f.person,
            false,
        )
        .await
        .unwrap();
    assert_eq!(code(f.notes.get("1").await), "note-not-found");
}

#[tokio::test]
async fn comments_and_notes_share_one_named_author() {
    let f = fixture().await;
    let board = f.board("ARC").await;
    let key = f.ticket(&board, "Origin").await;
    let comments = CommentService::new(
        f.store.clone(),
        f.store.clone(),
        f.store.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    );
    let claude = Author {
        by_person: false,
        name: Some("claude"),
        session: None,
    };
    let comment = comments
        .add(&key, "Looked at it", claude, &f.person)
        .await
        .unwrap();
    let note = f
        .notes
        .add(
            board.project(),
            board.id(),
            &noted("Cache"),
            claude,
            &f.person,
        )
        .await
        .unwrap();
    assert_eq!(note.note.author, comment.comment.record().author);
    assert_eq!(comment.author_kind, ActorKind::Human);
    assert_eq!(note.author_name, "claude");
    // inside a session the same name is a session actor; a browser writes as the person
    let in_session = Author {
        session: Some("s-1"),
        ..claude
    };
    let sessioned = f
        .notes
        .add(
            board.project(),
            board.id(),
            &noted("Search"),
            in_session,
            &f.person,
        )
        .await
        .unwrap();
    assert_ne!(sessioned.note.author, note.note.author);
    let browser = Author {
        by_person: true,
        ..in_session
    };
    let typed = f
        .notes
        .add(
            board.project(),
            board.id(),
            &noted("Login"),
            browser,
            &f.person,
        )
        .await
        .unwrap();
    assert_eq!(typed.note.author, f.person.id());
    assert_eq!(typed.author_name, "anna");
}
