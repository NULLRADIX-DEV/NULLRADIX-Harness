#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;

use nullradix_app::snippets::{SnippetRepository, SnippetService};
use nullradix_app::{
    ChangeFeed, ChangeOp, ChangeSignal, Entity, ErrorKind, SystemClock, UuidV7Generator,
};
use nullradix_domain::{
    SNIPPET_CSS_MAX, Snippet, SnippetCss, SnippetId, SnippetName, SnippetRecord, Timestamp, Version,
};
use nullradix_store::Store;
use uuid::Uuid;

fn service(store: &Store) -> SnippetService {
    SnippetService::new(
        Arc::new(store.clone()),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    )
}

fn snippet(id: &str, css: &str, order: i64) -> Snippet {
    Snippet::create(
        SnippetId::parse(id).unwrap(),
        &SnippetName::parse("Name").unwrap(),
        &SnippetCss::parse(css).unwrap(),
        true,
        order,
        Timestamp::from_millis(7),
    )
}

async fn changes(store: &Store) -> Vec<(Entity, ChangeOp)> {
    store
        .changes_since(0, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|change| (change.entity, change.op))
        .collect()
}

#[tokio::test]
async fn a_snippet_round_trips_in_delivery_order() {
    let store = Store::in_memory().await.unwrap();
    let late = snippet("late", "b{}", 2);
    let early = snippet("early", "a{}\n", 1);
    let mut write = SnippetRepository::begin(&store).await.unwrap();
    let change = write.insert(&late, Uuid::from_u128(1)).await.unwrap();
    assert_eq!(
        (change.entity, change.op),
        (Entity::Snippet, ChangeOp::Insert)
    );
    write.insert(&early, Uuid::from_u128(2)).await.unwrap();
    write.commit().await.unwrap();
    assert_eq!(
        SnippetRepository::list(&store).await.unwrap(),
        vec![early, late]
    );
}

#[tokio::test]
async fn positions_count_up_and_an_id_is_taken_once() {
    let store = Store::in_memory().await.unwrap();
    let service = service(&store);
    let first = service.create("one", "One", "a{}", None).await.unwrap();
    let second = service.create("two", "Two", "b{}", None).await.unwrap();
    assert_eq!((first.record().order, second.record().order), (1, 2));
    service.delete("one", Version::INITIAL).await.unwrap();
    let third = service.create("three", "Three", "c{}", None).await.unwrap();
    assert_eq!(third.record().order, 3);
    let error = service
        .create("two", "Again", "d{}", None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "snippet-exists");
    assert_eq!(
        service.active_css().await.unwrap(),
        "/* snippet: two */\nb{}\n/* snippet: three */\nc{}\n"
    );
}

#[tokio::test]
async fn a_duplicate_insert_is_a_conflict_not_a_database_error() {
    let store = Store::in_memory().await.unwrap();
    let mut write = SnippetRepository::begin(&store).await.unwrap();
    write
        .insert(&snippet("one", "a{}", 1), Uuid::from_u128(1))
        .await
        .unwrap();
    let error = write
        .insert(&snippet("one", "b{}", 2), Uuid::from_u128(2))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "snippet-exists");
}

#[tokio::test]
async fn changes_are_guarded_by_the_version_and_reach_the_feed() {
    let store = Store::in_memory().await.unwrap();
    let service = service(&store);
    service.create("one", "One", "a{}", None).await.unwrap();
    let off = service
        .update("one", None, None, Some(false), Version::INITIAL)
        .await
        .unwrap();
    assert_eq!(off.record().version, Version::from_stored(2));
    let stale = service
        .update("one", None, Some("b{}"), None, Version::INITIAL)
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    let stale = service.delete("one", Version::INITIAL).await.unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    let gone = service
        .update("none", None, None, None, Version::INITIAL)
        .await
        .unwrap_err();
    assert_eq!(gone.code(), "snippet-missing");
    assert_eq!(service.active_css().await.unwrap(), "");
    service
        .delete("one", Version::from_stored(2))
        .await
        .unwrap();
    assert_eq!(
        changes(&store).await,
        vec![
            (Entity::Snippet, ChangeOp::Insert),
            (Entity::Snippet, ChangeOp::Update),
            (Entity::Snippet, ChangeOp::Delete),
        ]
    );
}

#[tokio::test]
async fn two_writers_with_the_same_version_cannot_both_win() {
    let store = Store::in_memory().await.unwrap();
    let service = Arc::new(service(&store));
    service.create("one", "One", "a{}", None).await.unwrap();
    let (first, second) = tokio::join!(
        service.update("one", None, Some("b{}"), None, Version::INITIAL),
        service.update("one", None, Some("c{}"), None, Version::INITIAL),
    );
    let outcomes = [first, second];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    let error = outcomes.into_iter().find_map(Result::err).unwrap();
    assert_eq!(error.code(), "version-conflict");
    let stored = service.list().await.unwrap();
    assert_eq!(stored[0].record().version, Version::from_stored(2));
}

#[tokio::test]
async fn the_database_holds_the_size_limit_too() {
    let store = Store::in_memory().await.unwrap();
    let mut record = snippet("big", "a{}", 1).record().clone();
    record.css = "a".repeat(SNIPPET_CSS_MAX + 1);
    let oversized = Snippet::restore(SnippetRecord { ..record });
    let mut write = SnippetRepository::begin(&store).await.unwrap();
    let error = write
        .insert(&oversized, Uuid::from_u128(1))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Internal);
}

#[tokio::test]
async fn the_store_refuses_a_stale_write_by_itself() {
    let store = Store::in_memory().await.unwrap();
    let first = snippet("one", "a{}", 1);
    let mut write = SnippetRepository::begin(&store).await.unwrap();
    write.insert(&first, Uuid::from_u128(1)).await.unwrap();
    let next = first.changed(None, None, Some(false), Timestamp::from_millis(9));
    // the stored row is at version 1, the caller claims 4
    let error = write
        .update(&next, Version::from_stored(4))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    let id = SnippetId::parse("one").unwrap();
    let error = write
        .delete(&id, Version::from_stored(4), Timestamp::from_millis(9))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    let other = SnippetId::parse("two").unwrap();
    assert!(
        write
            .delete(&other, Version::INITIAL, Timestamp::from_millis(9))
            .await
            .unwrap()
            .is_none()
    );
    let error = write
        .update(
            &snippet("two", "b{}", 2).changed(None, None, Some(false), Timestamp::from_millis(9)),
            Version::INITIAL,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), "snippet-missing");
}
