#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;

use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::comments::{Author, CommentLine, CommentService};
use nullradix_app::projects::ProjectService;
use nullradix_app::tickets::{NewTicket, TicketService, WriteOptions};
use nullradix_app::{ChangeFeed, ChangeOp, ChangeSignal, Entity, SystemClock, UuidV7Generator};
use nullradix_domain::{Actor, ActorKind, Board, Version};
use nullradix_store::Store;

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
};

fn agent(name: &str) -> Author<'_> {
    Author {
        by_person: false,
        name: Some(name),
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

    assert_eq!(f.bodies(&key).await, ["first", "second", "third"]);
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
        f.add("ARC-9", "x", PERSON).await,
        Err("ticket-not-found".into())
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
async fn only_person_comments_wait_for_delivery_to_agents() {
    let f = fixture().await;
    let first = f.ticket("One").await;
    let second = f.ticket("Two").await;
    f.add(&first, "please check", PERSON).await.unwrap();
    f.add(&first, "progress", agent("claude")).await.unwrap();
    f.add(&second, "here too", PERSON).await.unwrap();
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
            (second.clone(), "here too")
        ]
    );
    assert_eq!(
        f.comments.undelivered(&[]).await.unwrap(),
        Vec::<CommentLine>::new()
    );

    let delivered: Vec<_> = waiting.iter().map(|line| line.comment.id()).collect();
    f.comments.mark_delivered(&delivered).await.unwrap();
    f.comments.mark_delivered(&[]).await.unwrap();

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

#[tokio::test]
async fn deleting_a_ticket_deletes_its_comments() {
    let f = fixture().await;
    let key = f.ticket("Gone").await;
    f.add(&key, "soon gone", PERSON).await.unwrap();
    let id = f.tickets.get(&key).await.unwrap().view.ticket.id();

    f.tickets.delete(&key, &f.person, None).await.unwrap();

    assert_eq!(
        f.comments.undelivered(&[id]).await.unwrap(),
        Vec::<CommentLine>::new()
    );
    assert_eq!(
        f.comments.list(&key).await.unwrap_err().code(),
        "ticket-not-found"
    );
}
