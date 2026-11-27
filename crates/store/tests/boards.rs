#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;

use nullradix_app::boards::{BoardService, BoardSummary, local_person};
use nullradix_app::projects::ProjectService;
use nullradix_app::{
    ChangeFeed, ChangeOp, ChangeSignal, Clock, Entity, ErrorKind, SystemClock, UuidV7Generator,
};
use nullradix_domain::{Actor, ActorKind, ActorName, Id, Project, Version};
use nullradix_store::Store;
use tokio::sync::watch::Receiver;

struct Fixture {
    boards: BoardService,
    projects: ProjectService,
    person: Actor,
    store: Arc<Store>,
    head: Receiver<i64>,
}

impl Fixture {
    /// Board changes in the feed, oldest first.
    async fn board_ops(&self) -> Vec<ChangeOp> {
        self.store
            .changes_since(0, 100)
            .await
            .unwrap()
            .into_iter()
            .filter(|change| change.entity == Entity::Board)
            .map(|change| change.op)
            .collect()
    }

    /// The head seq the signal announced since the last call, if it moved.
    fn signalled(&mut self) -> Option<i64> {
        self.head
            .has_changed()
            .unwrap()
            .then(|| *self.head.borrow_and_update())
    }
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let head = signal.subscribe();
    let person = local_person(store.as_ref(), "anna", &UuidV7Generator, &SystemClock)
        .await
        .unwrap();
    Fixture {
        boards: BoardService::new(
            store.clone(),
            store.clone(),
            store.clone(),
            common::runners(&store, &signal).await,
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ),
        projects: ProjectService::new(
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal,
        ),
        person,
        store,
        head,
    }
}

async fn project(fx: &Fixture, key: &str) -> Project {
    fx.projects.create(key, key).await.unwrap()
}

fn unknown() -> Id<Project> {
    Id::from_uuid(uuid::Uuid::from_u128(9))
}

fn names(summaries: Vec<BoardSummary>) -> Vec<String> {
    summaries
        .into_iter()
        .map(|summary| summary.board.name().as_str().to_owned())
        .collect()
}

#[tokio::test]
async fn the_local_person_is_created_once() {
    let store = Store::in_memory().await.unwrap();
    let first = local_person(&store, " anna ", &UuidV7Generator, &SystemClock)
        .await
        .unwrap();
    let again = local_person(&store, "anna", &UuidV7Generator, &SystemClock)
        .await
        .unwrap();
    assert_eq!(first, again);
    assert_eq!(first.name().as_str(), "anna");
    let error = local_person(&store, " ", &UuidV7Generator, &SystemClock)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "actor-name-empty");
}

#[tokio::test]
async fn boards_list_by_project_key_then_name() {
    let fx = fixture().await;
    let zed = project(&fx, "ZED").await;
    let arc = project(&fx, "ARC").await;
    let empty = project(&fx, "EMP").await;
    for (project, name) in [(&zed, "alpha"), (&arc, "beta"), (&arc, "Alpha")] {
        fx.boards
            .create(project.id(), name, &fx.person)
            .await
            .unwrap();
    }

    assert_eq!(
        names(fx.boards.list(None).await.unwrap()),
        ["Alpha", "beta", "alpha"]
    );
    let only_arc = fx.boards.list(Some(arc.id())).await.unwrap();
    assert_eq!(only_arc[0].owner_name, "anna");
    assert_eq!(only_arc[0].open_tickets, 0);
    assert_eq!(names(only_arc), ["Alpha", "beta"]);
    assert_eq!(fx.boards.list(Some(empty.id())).await.unwrap(), []);
    let error = fx.boards.list(Some(unknown())).await.unwrap_err();
    assert_eq!(error.code(), "project-not-found");
}

#[tokio::test]
async fn boards_need_their_project_an_owner_and_a_valid_name() {
    let fx = fixture().await;
    let error = fx
        .boards
        .create(unknown(), "Main", &fx.person)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "project-not-found");
    let arc = project(&fx, "ARC").await;
    let error = fx
        .boards
        .create(arc.id(), "  ", &fx.person)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "board-name-empty");

    let stranger = Actor::new(
        Id::from_uuid(uuid::Uuid::from_u128(7)),
        ActorKind::Human,
        ActorName::parse("ghost").unwrap(),
        SystemClock.now(),
    );
    let error = fx
        .boards
        .create(arc.id(), "Main", &stranger)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Internal);
    assert_eq!(fx.board_ops().await, []);
}

#[tokio::test]
async fn board_writes_land_in_the_change_feed() {
    let mut fx = fixture().await;
    let arc = project(&fx, "ARC").await;
    fx.signalled();
    let board = fx
        .boards
        .create(arc.id(), "Main", &fx.person)
        .await
        .unwrap()
        .board;
    assert!(fx.signalled().is_some());

    let renamed = fx
        .boards
        .rename(arc.id(), board.id(), "Next", board.version())
        .await
        .unwrap()
        .board;
    assert!(fx.signalled().is_some());
    fx.boards
        .rename(arc.id(), board.id(), " Next ", renamed.version())
        .await
        .unwrap();
    fx.projects
        .delete(arc.id(), arc.version())
        .await
        .unwrap_err();
    assert_eq!(fx.signalled(), None);

    fx.boards
        .delete(
            arc.id(),
            board.id(),
            renamed.version(),
            false,
            &fx.person,
            false,
        )
        .await
        .unwrap();
    assert!(fx.signalled().is_some());
    let feed = fx.store.changes_since(0, 100).await.unwrap();
    let last = feed.last().unwrap();
    assert_eq!(
        (last.entity, last.entity_id),
        (Entity::Board, board.id().as_uuid())
    );
    assert_eq!(
        fx.board_ops().await,
        [ChangeOp::Insert, ChangeOp::Update, ChangeOp::Delete]
    );
}

#[tokio::test]
async fn boards_rename_with_their_version() {
    let fx = fixture().await;
    let arc = project(&fx, "ARC").await;
    let board = fx
        .boards
        .create(arc.id(), "Main", &fx.person)
        .await
        .unwrap()
        .board;

    let renamed = fx
        .boards
        .rename(arc.id(), board.id(), "Next", Version::INITIAL)
        .await
        .unwrap();
    assert_eq!(renamed.board.name().as_str(), "Next");
    let stale = fx
        .boards
        .rename(arc.id(), board.id(), "Again", Version::INITIAL)
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    let same = fx
        .boards
        .rename(arc.id(), board.id(), "Next", renamed.board.version())
        .await
        .unwrap();
    assert_eq!(same.board.version(), renamed.board.version());
}

#[tokio::test]
async fn boards_answer_only_under_their_own_project() {
    let fx = fixture().await;
    let arc = project(&fx, "ARC").await;
    let other = project(&fx, "OTH").await;
    let board = fx
        .boards
        .create(arc.id(), "Main", &fx.person)
        .await
        .unwrap()
        .board;

    let codes = [
        fx.boards.get(other.id(), board.id()).await.unwrap_err(),
        fx.boards
            .rename(other.id(), board.id(), "Taken", board.version())
            .await
            .unwrap_err(),
        fx.boards
            .delete(
                other.id(),
                board.id(),
                board.version(),
                false,
                &fx.person,
                false,
            )
            .await
            .unwrap_err(),
    ]
    .map(|error| error.code());
    assert_eq!(codes, ["board-not-found"; 3]);
    let error = fx.boards.get(unknown(), board.id()).await.unwrap_err();
    assert_eq!(error.code(), "project-not-found");
    assert_eq!(fx.board_ops().await, [ChangeOp::Insert]);
    let kept = fx.boards.get(arc.id(), board.id()).await.unwrap();
    assert_eq!(kept.board, board);
}

#[tokio::test]
async fn projects_with_boards_cannot_be_deleted_until_the_boards_are_gone() {
    let fx = fixture().await;
    let arc = project(&fx, "ARC").await;
    let board = fx
        .boards
        .create(arc.id(), "Main", &fx.person)
        .await
        .unwrap()
        .board;

    let refused = fx
        .projects
        .delete(arc.id(), arc.version())
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "project-has-boards");

    let stale = fx
        .boards
        .delete(
            arc.id(),
            board.id(),
            Version::INITIAL.next(),
            false,
            &fx.person,
            false,
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    fx.boards
        .delete(
            arc.id(),
            board.id(),
            board.version(),
            false,
            &fx.person,
            false,
        )
        .await
        .unwrap();
    assert_eq!(fx.boards.list(None).await.unwrap(), Vec::new());
    fx.projects.delete(arc.id(), arc.version()).await.unwrap();
}
