#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;

use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::projects::ProjectService;
use nullradix_app::{ChangeSignal, SystemClock, UuidV7Generator};
use nullradix_domain::{Actor, Id, Project, Version};
use nullradix_store::Store;

struct Fixture {
    boards: BoardService,
    projects: ProjectService,
    person: Actor,
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let person = local_person(store.as_ref(), "anna", &UuidV7Generator, &SystemClock)
        .await
        .unwrap();
    Fixture {
        boards: BoardService::new(
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ),
        projects: ProjectService::new(
            store,
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal,
        ),
        person,
    }
}

async fn project(fx: &Fixture, key: &str) -> Project {
    fx.projects.create(key, key).await.unwrap()
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
    assert_eq!(error.code(), "invalid-actor-name");
}

#[tokio::test]
async fn boards_list_by_project_key_then_name() {
    let fx = fixture().await;
    let zed = project(&fx, "ZED").await;
    let arc = project(&fx, "ARC").await;
    fx.boards
        .create(zed.id(), "alpha", &fx.person)
        .await
        .unwrap();
    fx.boards
        .create(arc.id(), "beta", &fx.person)
        .await
        .unwrap();
    fx.boards
        .create(arc.id(), "Alpha", &fx.person)
        .await
        .unwrap();

    let names = |summaries: Vec<nullradix_app::boards::BoardSummary>| -> Vec<String> {
        summaries
            .into_iter()
            .map(|summary| summary.board.name().as_str().to_owned())
            .collect()
    };
    assert_eq!(
        names(fx.boards.list(None).await.unwrap()),
        ["Alpha", "beta", "alpha"]
    );
    let only_arc = fx.boards.list(Some(arc.id())).await.unwrap();
    assert_eq!(only_arc[0].owner_name, "anna");
    assert_eq!(only_arc[0].open_tickets, 0);
    assert_eq!(names(only_arc), ["Alpha", "beta"]);
}

#[tokio::test]
async fn boards_need_their_project_and_a_valid_name() {
    let fx = fixture().await;
    let missing: Id<Project> = Id::from_uuid(uuid::Uuid::now_v7());
    let error = fx
        .boards
        .create(missing, "Main", &fx.person)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "project-not-found");
    let arc = project(&fx, "ARC").await;
    let error = fx
        .boards
        .create(arc.id(), "  ", &fx.person)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "invalid-board-name");
}

#[tokio::test]
async fn boards_rename_with_their_version_and_belong_to_one_project() {
    let fx = fixture().await;
    let arc = project(&fx, "ARC").await;
    let other = project(&fx, "OTH").await;
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
    let elsewhere = fx.boards.get(other.id(), board.id()).await.unwrap_err();
    assert_eq!(elsewhere.code(), "board-not-found");
    let same = fx
        .boards
        .rename(arc.id(), board.id(), "Next", renamed.board.version())
        .await
        .unwrap();
    assert_eq!(same.board.version(), renamed.board.version());
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
        .delete(arc.id(), board.id(), Version::INITIAL.next())
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    fx.boards
        .delete(arc.id(), board.id(), board.version())
        .await
        .unwrap();
    assert_eq!(fx.boards.list(None).await.unwrap(), Vec::new());
    fx.projects.delete(arc.id(), arc.version()).await.unwrap();
}
