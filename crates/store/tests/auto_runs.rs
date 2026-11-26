#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use nullradix_app::dispatch::tracking::{AutoRunRepository, TrackKind, Tracked};
use nullradix_domain::Timestamp;
use nullradix_store::Store;
use uuid::Uuid;

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis)
}

#[tokio::test]
async fn starts_are_counted_and_the_automation_mark_stays_once_set() {
    let store = Store::in_memory().await.unwrap();
    let subject = Uuid::from_u128(7);
    assert_eq!(store.get(TrackKind::Research, subject).await.unwrap(), None);
    store
        .started(TrackKind::Research, subject, at(10), false)
        .await
        .unwrap();
    store
        .started(TrackKind::Research, subject, at(20), true)
        .await
        .unwrap();
    // a later manual start keeps the moment the automation took the question up
    store
        .started(TrackKind::Research, subject, at(30), false)
        .await
        .unwrap();
    let row = store
        .get(TrackKind::Research, subject)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.count, 3);
    assert_eq!(row.last_started_at, Some(at(30)));
    assert_eq!(row.auto_at, Some(at(20)));
    // the kinds are apart
    assert_eq!(
        store
            .get(TrackKind::AutopilotResearch, subject)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_mark_does_not_count_and_a_bump_counts_without_a_start() {
    let store = Store::in_memory().await.unwrap();
    let marked = Uuid::from_u128(1);
    store
        .mark(TrackKind::Research, marked, at(5))
        .await
        .unwrap();
    let row = store
        .get(TrackKind::Research, marked)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.count, row.last_started_at, row.auto_at),
        (0, None, Some(at(5)))
    );
    let bumped = Uuid::from_u128(2);
    assert_eq!(
        store
            .bump(TrackKind::AutopilotResearch, bumped)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .bump(TrackKind::AutopilotResearch, bumped)
            .await
            .unwrap(),
        2
    );
    let all = store.all(TrackKind::AutopilotResearch).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[&bumped].count, 2);
}

#[tokio::test]
async fn a_row_is_written_as_given_and_cleared() {
    let store = Store::in_memory().await.unwrap();
    let subject = Uuid::from_u128(9);
    let row = Tracked {
        count: 4,
        last_started_at: Some(at(40)),
        auto_at: None,
    };
    store.put(TrackKind::Research, subject, row).await.unwrap();
    store
        .put(TrackKind::Research, subject, Tracked { count: 1, ..row })
        .await
        .unwrap();
    let kept = store
        .get(TrackKind::Research, subject)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((kept.count, kept.last_started_at), (1, Some(at(40))));
    store.clear(TrackKind::Research, subject).await.unwrap();
    assert_eq!(store.get(TrackKind::Research, subject).await.unwrap(), None);
    // clearing what is not there is no error
    store.clear(TrackKind::Research, subject).await.unwrap();
}
