#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use nullradix_app::auth::{AuthService, Principal, Secret};
use nullradix_app::{Clock, ErrorKind, UuidV7Generator};
use nullradix_domain::Timestamp;
use nullradix_store::Store;

#[derive(Default)]
struct TestClock(AtomicI64);

impl TestClock {
    fn advance(&self, by: Duration) {
        let millis = i64::try_from(by.as_millis()).unwrap();
        self.0.fetch_add(millis, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.0.load(Ordering::SeqCst))
    }
}

async fn service() -> (AuthService, Arc<TestClock>) {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let clock = Arc::new(TestClock::default());
    let service = AuthService::new(store, clock.clone(), Arc::new(UuidV7Generator));
    (service, clock)
}

#[tokio::test]
async fn local_token_is_created_once_and_then_kept() {
    let (auth, _) = service().await;
    let first = auth.local_token(None).await.unwrap();
    let kept = auth.local_token(Some(first.clone())).await.unwrap();
    assert_eq!(kept, first);
    let unknown = Secret::parse(&"a".repeat(64)).unwrap();
    let replaced = auth.local_token(Some(unknown.clone())).await.unwrap();
    assert_ne!(replaced, unknown);
    assert!(auth.authenticate_token(&replaced).await.is_ok());
    assert_eq!(
        auth.authenticate_token(&first).await.unwrap_err().code(),
        "token-invalid"
    );
}

#[tokio::test]
async fn tokens_authenticate_only_when_registered() {
    let (auth, _) = service().await;
    let token = auth.local_token(None).await.unwrap();
    assert!(matches!(
        auth.authenticate_token(&token).await.unwrap(),
        Principal::Token { .. }
    ));
    let unknown = Secret::parse(&"b".repeat(64)).unwrap();
    let error = auth.authenticate_token(&unknown).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Unauthorized);
}

#[tokio::test]
async fn a_handoff_code_opens_one_session_and_only_once() {
    let (auth, _) = service().await;
    let code = auth.create_handoff().unwrap();
    let session = auth.redeem_handoff(&code).await.unwrap();
    assert!(matches!(
        auth.authenticate_session(&session).await.unwrap(),
        Principal::Session { .. }
    ));
    let again = auth.redeem_handoff(&code).await.unwrap_err();
    assert_eq!(again.code(), "handoff-invalid");
}

#[tokio::test]
async fn handoff_codes_expire_after_a_minute() {
    let (auth, clock) = service().await;
    let code = auth.create_handoff().unwrap();
    clock.advance(Duration::from_secs(61));
    let error = auth.redeem_handoff(&code).await.unwrap_err();
    assert_eq!(error.code(), "handoff-invalid");
}

#[tokio::test]
async fn sessions_expire_and_can_be_ended() {
    let (auth, clock) = service().await;
    let session = auth
        .redeem_handoff(&auth.create_handoff().unwrap())
        .await
        .unwrap();
    auth.end_session(&session).await.unwrap();
    assert_eq!(
        auth.authenticate_session(&session)
            .await
            .unwrap_err()
            .code(),
        "session-invalid"
    );

    let session = auth
        .redeem_handoff(&auth.create_handoff().unwrap())
        .await
        .unwrap();
    let pruned_early = auth.prune_expired_sessions().await.unwrap();
    assert_eq!(pruned_early, 0);
    clock.advance(Duration::from_hours(31 * 24));
    assert_eq!(
        auth.authenticate_session(&session)
            .await
            .unwrap_err()
            .code(),
        "session-expired"
    );
    assert_eq!(
        auth.authenticate_session(&session)
            .await
            .unwrap_err()
            .code(),
        "session-invalid"
    );
}

#[tokio::test]
async fn expired_sessions_are_pruned() {
    let (auth, clock) = service().await;
    let session = auth
        .redeem_handoff(&auth.create_handoff().unwrap())
        .await
        .unwrap();
    clock.advance(Duration::from_hours(31 * 24));
    assert_eq!(auth.prune_expired_sessions().await.unwrap(), 1);
    assert_eq!(
        auth.authenticate_session(&session)
            .await
            .unwrap_err()
            .code(),
        "session-invalid"
    );
}
