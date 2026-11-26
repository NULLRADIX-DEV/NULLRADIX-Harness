#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;

use nullradix_app::theme_catalog::{CatalogService, NoNetwork, SOURCES_FEED_ID, SourcesRepository};
use nullradix_app::themes::{ThemeService, UnavailableFiles};
use nullradix_app::{ChangeFeed, ChangeOp, ChangeSignal, Entity, SystemClock, UuidV7Generator};
use nullradix_domain::{CatalogSource, CatalogUrl, Reach, Timestamp, Version};
use nullradix_store::Store;

fn source(host: &str, at: i64) -> CatalogSource {
    let url = CatalogUrl::parse(&format!("https://{host}.example.com/i.json"), Reach::Public);
    CatalogSource::new(url.unwrap(), Timestamp::from_millis(at))
}

fn service(store: &Store) -> CatalogService {
    let signal = ChangeSignal::new();
    let themes = Arc::new(ThemeService::new(
        Arc::new(store.clone()),
        Arc::new(UnavailableFiles),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    CatalogService::new(
        Arc::new(store.clone()),
        Arc::new(NoNetwork),
        themes,
        Arc::new(SystemClock),
        signal,
        Reach::Public,
    )
}

#[tokio::test]
async fn sources_round_trip_in_the_settings_table_with_their_versions() {
    let store = Store::in_memory().await.unwrap();
    assert!(SourcesRepository::sources(&store).await.unwrap().is_none());

    let first = vec![source("a", 1)];
    let mut write = SourcesRepository::begin(&store).await.unwrap();
    let (version, change) = write
        .put(&first, None, Timestamp::from_millis(1))
        .await
        .unwrap();
    write.commit().await.unwrap();
    assert_eq!(version, Version::INITIAL);
    assert_eq!(
        (change.entity, change.op, change.entity_id),
        (Entity::Setting, ChangeOp::Insert, SOURCES_FEED_ID)
    );

    let both = vec![source("a", 1), source("b", 2)];
    let mut write = SourcesRepository::begin(&store).await.unwrap();
    let (version, change) = write
        .put(&both, Some(Version::INITIAL), Timestamp::from_millis(2))
        .await
        .unwrap();
    write.commit().await.unwrap();
    assert_eq!(
        (version, change.op),
        (Version::from_stored(2), ChangeOp::Update)
    );

    let stored = SourcesRepository::sources(&store).await.unwrap().unwrap();
    assert_eq!(
        (stored.sources, stored.version),
        (both, Version::from_stored(2))
    );
}

#[tokio::test]
async fn a_stale_or_repeated_first_write_is_a_conflict_and_changes_nothing() {
    let store = Store::in_memory().await.unwrap();
    let mut write = SourcesRepository::begin(&store).await.unwrap();
    write
        .put(&[source("a", 1)], None, Timestamp::from_millis(1))
        .await
        .unwrap();
    write.commit().await.unwrap();

    let mut write = SourcesRepository::begin(&store).await.unwrap();
    let again = write
        .put(&[source("b", 2)], None, Timestamp::from_millis(2))
        .await;
    assert_eq!(again.unwrap_err().code(), "version-conflict");
    let stale = write
        .put(
            &[source("b", 2)],
            Some(Version::from_stored(7)),
            Timestamp::from_millis(2),
        )
        .await;
    assert_eq!(stale.unwrap_err().code(), "version-conflict");
    drop(write);

    let stored = SourcesRepository::sources(&store).await.unwrap().unwrap();
    assert_eq!(stored.sources, vec![source("a", 1)]);
    assert_eq!(store.changes_since(0, 100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn parallel_additions_all_land() {
    let store = Store::in_memory().await.unwrap();
    let service = Arc::new(service(&store));
    let mut tasks = Vec::new();
    for number in 0..8 {
        let service = service.clone();
        tasks.push(tokio::spawn(async move {
            service
                .add_source(&format!("https://s{number}.example.com/i.json"))
                .await
                .unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let list = service.sources().await.unwrap();
    assert_eq!(list.sources.len(), 8);
    assert_eq!(list.version, Some(Version::from_stored(8)));
}

#[tokio::test]
async fn removing_survives_a_new_service_over_the_same_store() {
    let store = Store::in_memory().await.unwrap();
    let first = service(&store);
    let added = first
        .add_source("https://a.example.com/i.json")
        .await
        .unwrap();
    first
        .add_source("https://b.example.com/i.json")
        .await
        .unwrap();
    let second = service(&store);
    second
        .remove_source(added.id().as_str(), Version::from_stored(2))
        .await
        .unwrap();
    let list = first.sources().await.unwrap();
    assert_eq!(list.sources.len(), 1);
    assert_ne!(list.sources[0].id(), added.id());
}
