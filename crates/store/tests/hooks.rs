#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use nullradix_app::Clock;
use nullradix_app::hooks::{HookRepository, HookService};
use nullradix_domain::{HookDetails, HookEvent, HookKind, Id, SessionRef, Timestamp};
use nullradix_store::Store;

/// Clock the test moves by hand.
#[derive(Default)]
struct ManualClock(AtomicI64);

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.0.load(Ordering::SeqCst))
    }
}

fn event(n: u128, kind: HookKind) -> HookEvent {
    HookEvent::new(
        Id::from_uuid(uuid::Uuid::from_u128(n)),
        kind,
        SessionRef::parse("session-1").unwrap(),
        "/repo",
        Timestamp::from_millis(10),
        HookDetails {
            prompt: Some(String::from("fix the login")),
            ticket: Some(String::from("ARC-0007")),
            ..HookDetails::default()
        },
    )
    .unwrap()
}

#[tokio::test]
async fn resent_events_are_recorded_once_and_read_back_whole() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let clock = Arc::new(ManualClock::default());
    let hooks = HookService::new(store.clone(), clock.clone());

    let first = event(1, HookKind::UserPrompt);
    assert!(
        store
            .record(&first, Timestamp::from_millis(20))
            .await
            .unwrap()
    );
    assert!(
        !store
            .record(&first, Timestamp::from_millis(21))
            .await
            .unwrap()
    );
    assert_eq!(
        hooks.receive(&first).await.unwrap().additional_context,
        None
    );
    clock.0.store(30, Ordering::SeqCst);
    hooks.receive(&event(2, HookKind::Stop)).await.unwrap();

    let recent = hooks.recent(10).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].kind(), HookKind::Stop);
    assert_eq!(recent[1], first);
}

#[tokio::test]
async fn pruning_forgets_old_events_only() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let clock = Arc::new(ManualClock::default());
    let hooks = HookService::new(store.clone(), clock.clone());
    store
        .record(&event(1, HookKind::Stop), Timestamp::from_millis(1_000))
        .await
        .unwrap();
    store
        .record(&event(2, HookKind::Stop), Timestamp::from_millis(5_000))
        .await
        .unwrap();

    clock.0.store(6_000, Ordering::SeqCst);
    assert_eq!(hooks.prune(Duration::from_secs(2)).await.unwrap(), 1);
    let left = hooks.recent(10).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id(), Id::from_uuid(uuid::Uuid::from_u128(2)));
}
