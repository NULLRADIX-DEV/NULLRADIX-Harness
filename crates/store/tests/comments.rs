#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;

use nullradix_app::boards::ActorRepository;
use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::comments::{Author, CommentLine, CommentService};
use nullradix_app::projects::ProjectService;
use nullradix_app::tickets::{NewTicket, TicketService, WriteOptions};
use nullradix_app::{ChangeFeed, ChangeOp, ChangeSignal, Entity, SystemClock, UuidV7Generator};
use nullradix_domain::{Actor, ActorKind, ActorName, Board, Id, Timestamp, Version};
use nullradix_store::Store;
use uuid::Uuid;

struct Fixture {
    comments: CommentService,
    tickets: TicketService,
    person: Actor,
    store: Arc<Store>,
    board: Board,
}

const PERSON: Author<'static> = Author {
    by_person: true,
    name: None,
    session: None,
};

/// A CLI call under this name inside an agent session.
fn agent(name: &str) -> Author<'_> {
    Author {
        by_person: false,
        name: Some(name),
        session: Some("s-1"),
    }
}

/// A CLI call under this name outside any agent session.
fn named(name: &str) -> Author<'_> {
    Author {
        name: Some(name),
        ..Author::default()
    }
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let person = local_person(store.as_ref(), "anna", &UuidV7Generator, &SystemClock)
        .await
        .unwrap();
    let clock = Arc::new(SystemClock);
    let ids = Arc::new(UuidV7Generator);
    let projects = ProjectService::new(store.clone(), clock.clone(), ids.clone(), signal.clone());
    let boards = BoardService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        common::runners(&store, &signal).await,
        clock.clone(),
        ids.clone(),
        signal.clone(),
    );
    let project = projects.create("ARC", "Arches").await.unwrap();
    let board = boards
        .create(project.id(), "Main", &person)
        .await
        .unwrap()
        .board;
    let tickets = TicketService::new(
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    );
    let comments = CommentService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        clock,
        ids,
        signal,
    );
    Fixture {
        comments,
        tickets,
        person,
        store,
        board,
    }
}

impl Fixture {
    async fn ticket(&self, title: &str) -> String {
        let input = NewTicket {
            title: title.to_owned(),
            ..NewTicket::default()
        };
        self.tickets
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
            .to_string()
    }

    async fn add(&self, key: &str, body: &str, author: Author<'_>) -> Result<(), String> {
        self.comments
            .add(key, body, author, &self.person)
            .await
            .map(|_| ())
            .map_err(|error| error.code().to_owned())
    }

    async fn bodies(&self, key: &str) -> Vec<String> {
        self.comments
            .list(key)
            .await
            .unwrap()
            .into_iter()
            .map(|line| line.comment.body().to_owned())
            .collect()
    }
}

#[tokio::test]
async fn a_comment_bumps_the_ticket_version_without_a_history_line() {
    let f = fixture().await;
    let key = f.ticket("Commented").await;
    let head = f.store.head().await.unwrap();

    let line = f
        .comments
        .add("arc-1", "  Step 2 done \n", PERSON, &f.person)
        .await
        .unwrap();

    assert_eq!(line.comment.body(), "Step 2 done");
    assert_eq!(line.ticket_key.to_string(), key);
    assert_eq!(line.author_name, "anna");
    assert_eq!(line.author_kind, ActorKind::Human);
    assert_eq!(line.comment.record().author, f.person.id());
    let detail = f.tickets.get(&key).await.unwrap();
    assert_eq!(detail.view.ticket.record().version, Version::from_stored(2));
    assert_eq!(detail.history.len(), 1, "only the created line");
    assert_eq!(detail.comments, [line]);
    let changes: Vec<(Entity, ChangeOp)> = f
        .store
        .changes_since(head, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|change| (change.entity, change.op))
        .collect();
    assert_eq!(
        changes,
        [
            (Entity::Ticket, ChangeOp::Update),
            (Entity::Comment, ChangeOp::Insert)
        ]
    );
}

#[tokio::test]
async fn comments_list_oldest_first_with_their_authors() {
    let f = fixture().await;
    let key = f.ticket("Talk").await;
    f.add(&key, "first", PERSON).await.unwrap();
    f.add(&key, "second", agent("claude")).await.unwrap();
    f.add(&key, "third", agent(" ")).await.unwrap();
    f.add(&key, "fourth", named("bob")).await.unwrap();
    let browser = Author {
        name: Some("claude"),
        ..PERSON
    };
    f.add(&key, "fifth", browser).await.unwrap();

    assert_eq!(
        f.bodies(&key).await,
        ["first", "second", "third", "fourth", "fifth"]
    );
    let authors: Vec<(String, ActorKind)> = f
        .comments
        .list(&key)
        .await
        .unwrap()
        .into_iter()
        .map(|line| (line.author_name, line.author_kind))
        .collect();
    assert_eq!(
        authors,
        [
            (String::from("anna"), ActorKind::Human),
            (String::from("claude"), ActorKind::Session),
            // a session without a name writes as a session under the person's name
            (String::from("anna"), ActorKind::Session),
            (String::from("bob"), ActorKind::Human),
            (String::from("anna"), ActorKind::Human),
        ]
    );
}

#[tokio::test]
async fn refuses_empty_comments_unknown_tickets_and_long_names() {
    let f = fixture().await;
    let key = f.ticket("Strict").await;

    assert_eq!(
        f.add(&key, " \n\t", PERSON).await,
        Err("comment-empty".into())
    );
    assert_eq!(
        f.add("ARC-9", "x", agent("ghost")).await,
        Err("ticket-not-found".into())
    );
    let ghost = Actor::new(
        Id::from_uuid(Uuid::from_u128(7)),
        ActorKind::Session,
        ActorName::parse("ghost").unwrap(),
        Timestamp::from_millis(0),
    );
    assert_eq!(
        f.store.ensure(&ghost).await.unwrap().id(),
        ghost.id(),
        "a wrong key leaves no actor behind"
    );
    assert_eq!(
        f.add("nope", "x", PERSON).await,
        Err("ticket-id-invalid".into())
    );
    let long = "a".repeat(101);
    assert_eq!(
        f.add(&key, "x", agent(&long)).await,
        Err("value-too-long".into())
    );
    assert_eq!(
        f.comments.list("ARC-9").await.unwrap_err().code(),
        "ticket-not-found"
    );
    let version = f
        .tickets
        .get(&key)
        .await
        .unwrap()
        .view
        .ticket
        .record()
        .version;
    assert_eq!(version, Version::INITIAL);
}

#[tokio::test]
async fn only_comments_from_an_agent_session_count_as_delivered() {
    let f = fixture().await;
    let first = f.ticket("One").await;
    let second = f.ticket("Two").await;
    f.add(&first, "please check", PERSON).await.unwrap();
    f.add(&first, "progress", agent("claude")).await.unwrap();
    f.add(&second, "here too", named("bob")).await.unwrap();
    let browser = Author {
        session: Some("s-1"),
        ..PERSON
    };
    f.add(&second, "from the board", browser).await.unwrap();
    let ids = [
        f.tickets.get(&first).await.unwrap().view.ticket.id(),
        f.tickets.get(&second).await.unwrap().view.ticket.id(),
    ];

    let waiting = f.comments.undelivered(&ids).await.unwrap();
    let seen: Vec<(String, &str)> = waiting
        .iter()
        .map(|line| (line.ticket_key.to_string(), line.comment.body()))
        .collect();
    assert_eq!(
        seen,
        [
            (first.clone(), "please check"),
            (second.clone(), "here too"),
            (second.clone(), "from the board")
        ]
    );
    assert_eq!(
        f.comments.undelivered(&[]).await.unwrap(),
        Vec::<CommentLine>::new()
    );

    let delivered: Vec<_> = waiting.iter().map(|line| line.comment.id()).collect();
    let head = f.store.head().await.unwrap();
    f.comments.mark_delivered(&delivered).await.unwrap();
    let marked = f.store.changes_since(head, 10).await.unwrap();
    assert_eq!(
        marked
            .iter()
            .map(|change| (change.entity, change.op, change.entity_id))
            .collect::<Vec<_>>(),
        delivered
            .iter()
            .map(|id| (Entity::Comment, ChangeOp::Update, id.as_uuid()))
            .collect::<Vec<_>>()
    );
    let head = f.store.head().await.unwrap();
    f.comments.mark_delivered(&delivered).await.unwrap();
    f.comments.mark_delivered(&[]).await.unwrap();
    assert_eq!(f.store.head().await.unwrap(), head, "nothing left to mark");

    assert_eq!(
        f.comments.undelivered(&ids).await.unwrap(),
        Vec::<CommentLine>::new()
    );
    let read = f.comments.list(&first).await.unwrap();
    assert!(
        read.iter()
            .all(|line| line.comment.record().delivered_at.is_some())
    );
    let version = f
        .tickets
        .get(&first)
        .await
        .unwrap()
        .view
        .ticket
        .record()
        .version;
    assert_eq!(
        version,
        Version::from_stored(3),
        "delivery keeps the version"
    );
}
