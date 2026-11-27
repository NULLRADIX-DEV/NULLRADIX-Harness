#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;

use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::comments::Author;
use nullradix_app::inbox::{InboxRepository, InboxService, OrderEntry};
use nullradix_app::notes::{NewNote, NoteService};
use nullradix_app::projects::ProjectService;
use nullradix_app::questions::{NewQuestion, QuestionService};
use nullradix_app::tickets::{NewTicket, TicketPatch, TicketService, WriteOptions};
use nullradix_app::{ChangeFeed, ChangeOp, ChangeSignal, Entity, SystemClock, UuidV7Generator};
use nullradix_domain::{Actor, Board, InboxKind, InboxRef, Version};
use nullradix_store::Store;

struct Fixture {
    inbox: InboxService,
    tickets: Arc<TicketService>,
    questions: QuestionService,
    notes: NoteService,
    person: Actor,
    store: Arc<Store>,
    board: Board,
}

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
    let boards = BoardService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        common::runners(&store, &signal).await,
        clock.clone(),
        ids.clone(),
        signal.clone(),
    );
    let projects = ProjectService::new(store.clone(), clock.clone(), ids.clone(), signal.clone());
    let project = projects.create("ARC", "Arches").await.unwrap();
    let board = boards
        .create(project.id(), "Main", &person)
        .await
        .unwrap()
        .board;
    Fixture {
        inbox: InboxService::new(store.clone(), clock.clone(), signal.clone()),
        questions: QuestionService::new(
            store.clone(),
            store.clone(),
            store.clone(),
            store.clone(),
            clock.clone(),
            ids.clone(),
            signal.clone(),
        ),
        notes: NoteService::new(
            store.clone(),
            tickets.clone(),
            store.clone(),
            store.clone(),
            clock,
            ids,
            signal,
        ),
        tickets,
        person,
        store,
        board,
    }
}

impl Fixture {
    async fn ticket(&self, title: &str, status: &str, reason: Option<&str>) -> String {
        let input = NewTicket {
            title: title.to_owned(),
            ..NewTicket::default()
        };
        let key = self
            .tickets
            .create(
                self.board.project(),
                self.board.id(),
                &input,
                &self.person,
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .ticket
            .key()
            .to_string();
        let patch = TicketPatch {
            status: Some(status.to_owned()),
            blocked_reason: reason.map(|reason| Some(reason.to_owned())),
            ..TicketPatch::default()
        };
        self.tickets
            .update(
                &key,
                &patch,
                &self.person,
                Version::INITIAL,
                WriteOptions::default(),
            )
            .await
            .unwrap();
        key
    }

    async fn keys(&self) -> Vec<String> {
        let list = self.inbox.list(None, None).await.unwrap();
        list.items
            .iter()
            .map(|item| item.item.to_string())
            .collect()
    }
}

#[tokio::test]
async fn reads_waiting_tickets_open_questions_and_notes_in_one_snapshot() {
    let f = fixture().await;
    f.ticket("Merge it", "blocked", Some("Merge: conflict in a.rs"))
        .await;
    f.ticket("Plan it", "in-planning", None).await;
    let held = f.ticket("Held back", "blocked", Some("waits")).await;
    f.ticket("Busy", "in-progress", None).await;
    let question = NewQuestion {
        title: String::from("Which store?"),
        tickets: vec![held],
        ..NewQuestion::default()
    };
    f.questions
        .create(f.board.project(), &question, Author::default(), &f.person)
        .await
        .unwrap();
    let note = NewNote {
        title: String::from("Login page is slow"),
        body: Some(String::from("Takes 5 seconds")),
        ..NewNote::default()
    };
    f.notes
        .add(
            f.board.project(),
            f.board.id(),
            &note,
            Author::default(),
            &f.person,
        )
        .await
        .unwrap();

    let snapshot = f.store.snapshot().await.unwrap();
    assert_eq!(snapshot.boards.len(), 1);
    assert_eq!(
        snapshot.tickets.len(),
        3,
        "only waiting or asked-about tickets"
    );
    assert_eq!(snapshot.questions[0].tickets.len(), 1);
    assert_eq!(snapshot.notes[0].seen_count, 1);
    assert_eq!(
        f.keys().await,
        [
            "ticket:ARC-0001",
            "question:ARC-Q0001",
            "ticket:ARC-0002",
            "note:1"
        ]
    );
    let list = f
        .inbox
        .list(Some(f.board.id()), Some("note"))
        .await
        .unwrap();
    assert_eq!(list.items[0].kind, InboxKind::Note);
    assert_eq!(list.items[0].detail.as_deref(), Some("Takes 5 seconds"));
    assert_eq!((list.total, list.counts.planning), (4, 1));
}

#[tokio::test]
async fn saves_the_order_by_id_and_skips_entries_that_are_gone() {
    let f = fixture().await;
    f.ticket("First", "blocked", Some("waits")).await;
    f.ticket("Second", "blocked", Some("waits")).await;
    let entries = [
        OrderEntry {
            item_type: "ticket",
            key: "arc-2",
        },
        OrderEntry {
            item_type: "note",
            key: "99",
        },
        OrderEntry {
            item_type: "ticket",
            key: "ARC-1",
        },
    ];
    let saved = f
        .inbox
        .save_order(&entries, Version::INITIAL)
        .await
        .unwrap();
    assert_eq!(saved.order.len(), 2, "unknown entries are not stored");
    assert_eq!(f.keys().await, ["ticket:ARC-0002", "ticket:ARC-0001"]);
    f.tickets
        .delete("ARC-2", &f.person, None, false)
        .await
        .unwrap();
    assert_eq!(
        f.store.snapshot().await.unwrap().order,
        [InboxRef::parse("ticket", "ARC-1").unwrap()]
    );
    f.inbox.clear_order(saved.version).await.unwrap();
    assert_eq!(
        f.store.snapshot().await.unwrap().order,
        Vec::<InboxRef>::new()
    );
}

#[tokio::test]
async fn every_write_of_the_order_is_a_change_and_a_stale_version_is_refused() {
    let f = fixture().await;
    f.ticket("First", "blocked", Some("waits")).await;
    let entry = [OrderEntry {
        item_type: "ticket",
        key: "ARC-1",
    }];
    let head = f.store.head().await.unwrap();
    let snapshot = f.store.snapshot().await.unwrap();
    assert_eq!(snapshot.order_version, Version::INITIAL);

    let saved = f.inbox.save_order(&entry, Version::INITIAL).await.unwrap();
    assert_eq!(saved.version, Version::INITIAL.next());
    let stale = f.inbox.save_order(&[], Version::INITIAL).await.unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    let stale = f.inbox.clear_order(Version::INITIAL).await.unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    assert_eq!(
        f.keys().await,
        ["ticket:ARC-0001"],
        "a refused write changes nothing"
    );
    let cleared = f.inbox.clear_order(saved.version).await.unwrap();
    assert_eq!(cleared, saved.version.next());
    let list = f.inbox.list(None, None).await.unwrap();
    assert_eq!(list.order_version, cleared);

    let changes = f.store.changes_since(head, 100).await.unwrap();
    let order: Vec<_> = changes
        .iter()
        .filter(|change| change.entity == Entity::InboxOrder)
        .map(|change| change.op)
        .collect();
    assert_eq!(order, [ChangeOp::Update, ChangeOp::Delete]);
}
