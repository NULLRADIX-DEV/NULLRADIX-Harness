#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests fail loudly"
)]

use std::num::NonZeroUsize;
use std::sync::Arc;

use nullradix_app::projects::ProjectService;
use nullradix_app::{
    Change, ChangeFeed, ChangeOp, Entity, ErrorKind, EventBus, SystemClock, UuidV7Generator,
};
use nullradix_domain::{Id, Version};
use nullradix_store::Store;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;

struct Fixture {
    projects: ProjectService,
    store: Arc<Store>,
    events: Receiver<Change>,
}

impl Fixture {
    async fn ops(&self) -> Vec<ChangeOp> {
        self.store
            .changes_since(0, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|change| change.op)
            .collect()
    }

    fn next_event(&mut self) -> Option<Change> {
        match self.events.try_recv() {
            Ok(change) => Some(change),
            Err(TryRecvError::Empty) => None,
            Err(error) => panic!("{error}"),
        }
    }
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let bus = EventBus::new(NonZeroUsize::new(16).unwrap());
    let events = bus.subscribe();
    let projects = ProjectService::new(
        store.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        bus,
    );
    Fixture {
        projects,
        store,
        events,
    }
}

fn unknown() -> Id<nullradix_domain::Project> {
    Id::from_uuid(uuid::Uuid::from_u128(9))
}

#[tokio::test]
async fn creates_lists_and_reads_projects() {
    let mut fx = fixture().await;
    let zed = fx.projects.create("ZED", "Zed").await.unwrap();
    let arc = fx.projects.create("ARC", "  Arches  ").await.unwrap();

    let keys: Vec<String> = fx
        .projects
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|project| project.key().to_string())
        .collect();
    assert_eq!(keys, vec!["ARC", "ZED"]);
    assert_eq!(fx.projects.get(arc.id()).await.unwrap(), arc);
    assert_eq!(arc.name().as_str(), "Arches");

    let first = fx.next_event().unwrap();
    assert_eq!(
        (first.entity, first.entity_id, first.op),
        (Entity::Project, zed.id().as_uuid(), ChangeOp::Insert)
    );
    let feed = fx.store.changes_since(0, 10).await.unwrap();
    assert_eq!(feed.len(), 2);
    assert_eq!(feed[0], first);
}

#[tokio::test]
async fn rejects_invalid_input_and_taken_keys() {
    let fx = fixture().await;
    let error = fx.projects.create("arc", "Arches").await.unwrap_err();
    assert_eq!(error.code(), "invalid-project-key");
    let error = fx.projects.create("ARC", " ").await.unwrap_err();
    assert_eq!(error.code(), "invalid-project-name");

    fx.projects.create("ARC", "Arches").await.unwrap();
    let error = fx.projects.create("ARC", "Other").await.unwrap_err();
    assert_eq!(
        (error.kind(), error.code()),
        (ErrorKind::Conflict, "project-key-taken")
    );
    assert_eq!(fx.ops().await, vec![ChangeOp::Insert]);
}

#[tokio::test]
async fn renames_with_optimistic_concurrency() {
    let mut fx = fixture().await;
    let project = fx.projects.create("ARC", "Arches").await.unwrap();
    fx.next_event();
    let renamed = fx
        .projects
        .rename(project.id(), "Arches 2", project.version())
        .await
        .unwrap();
    assert_eq!(renamed.name().as_str(), "Arches 2");
    assert_eq!(renamed.version(), Version::from_stored(2));
    assert_eq!(fx.projects.get(project.id()).await.unwrap(), renamed);
    assert_eq!(fx.next_event().unwrap().op, ChangeOp::Update);

    let stale = fx
        .projects
        .rename(project.id(), "Arches 3", project.version())
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");

    let missing = fx
        .projects
        .rename(unknown(), "Nope", Version::INITIAL)
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "project-not-found");
    assert_eq!(fx.ops().await, vec![ChangeOp::Insert, ChangeOp::Update]);
    assert_eq!(fx.next_event(), None);
}

#[tokio::test]
async fn renaming_to_the_same_name_changes_nothing() {
    let mut fx = fixture().await;
    let project = fx.projects.create("ARC", "Arches").await.unwrap();
    fx.next_event();
    let same = fx
        .projects
        .rename(project.id(), " Arches ", project.version())
        .await
        .unwrap();
    assert_eq!(same, project);
    assert_eq!(fx.ops().await, vec![ChangeOp::Insert]);
    assert_eq!(fx.next_event(), None);
}

#[tokio::test]
async fn deletes_with_optimistic_concurrency() {
    let mut fx = fixture().await;
    let project = fx.projects.create("ARC", "Arches").await.unwrap();
    fx.next_event();
    let stale = fx
        .projects
        .delete(project.id(), Version::from_stored(5))
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    let missing = fx
        .projects
        .delete(unknown(), Version::INITIAL)
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "project-not-found");
    assert_eq!(fx.ops().await, vec![ChangeOp::Insert]);

    fx.projects
        .delete(project.id(), project.version())
        .await
        .unwrap();
    assert_eq!(fx.next_event().unwrap().op, ChangeOp::Delete);
    assert_eq!(
        fx.projects.get(project.id()).await.unwrap_err().code(),
        "project-not-found"
    );
    let after_delete = fx
        .projects
        .rename(project.id(), "Ghost", project.version())
        .await
        .unwrap_err();
    assert_eq!(after_delete.code(), "project-not-found");
    assert_eq!(fx.ops().await, vec![ChangeOp::Insert, ChangeOp::Delete]);
}
