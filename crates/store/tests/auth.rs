#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use nullradix_app::auth::{
    AuthService, CommandRefusal, CommandWriter, Principal, Secret, SessionSecrets,
};
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

async fn open(auth: &AuthService) -> SessionSecrets {
    auth.redeem_handoff(&auth.create_handoff().unwrap())
        .await
        .unwrap()
}

async fn code_of(auth: &AuthService, secrets: &SessionSecrets) -> String {
    match auth
        .authenticate_session(&secrets.session, Some(&secrets.key))
        .await
    {
        Ok(_) => String::from("ok"),
        Err(error) => error.code().to_owned(),
    }
}

async fn elevated_until(auth: &AuthService, secrets: &SessionSecrets) -> Option<Timestamp> {
    match auth
        .authenticate_session(&secrets.session, Some(&secrets.key))
        .await
        .unwrap()
    {
        Principal::Session { elevated_until, .. } => elevated_until,
        Principal::Token { .. } => unreachable!(),
    }
}

#[tokio::test]
async fn a_handoff_code_opens_one_session_and_only_once() {
    let (auth, _) = service().await;
    let code = auth.create_handoff().unwrap();
    let session = auth.redeem_handoff(&code).await.unwrap();
    assert!(matches!(
        auth.authenticate_session(&session.session, Some(&session.key))
            .await
            .unwrap(),
        Principal::Session {
            elevated_until: None,
            ..
        }
    ));
    let again = auth.redeem_handoff(&code).await.unwrap_err();
    assert_eq!(again.code(), "handoff-invalid");
}

#[tokio::test]
async fn handoff_codes_expire_after_fifteen_seconds() {
    let (auth, clock) = service().await;
    let code = auth.create_handoff().unwrap();
    clock.advance(Duration::from_secs(14));
    assert!(auth.redeem_handoff(&code).await.is_ok());
    let code = auth.create_handoff().unwrap();
    clock.advance(Duration::from_secs(16));
    let error = auth.redeem_handoff(&code).await.unwrap_err();
    assert_eq!(error.code(), "handoff-invalid");
}

#[tokio::test]
async fn a_session_cookie_needs_the_key_of_its_tab() {
    let (auth, _) = service().await;
    let mine = open(&auth).await;
    let other = open(&auth).await;
    let missing = auth
        .authenticate_session(&mine.session, None)
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "session-key-missing");
    assert_eq!(missing.kind(), ErrorKind::Unauthorized);
    let wrong = Secret::parse(&"d".repeat(64)).unwrap();
    for key in [&wrong, &other.key] {
        let error = auth
            .authenticate_session(&mine.session, Some(key))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "session-key-invalid");
    }
    // a refused key does not end the session of the real tab
    assert_eq!(code_of(&auth, &mine).await, "ok");
}

#[tokio::test]
async fn a_session_ends_after_twelve_idle_hours_but_activity_keeps_it() {
    let (auth, clock) = service().await;
    let session = open(&auth).await;
    for _ in 0..4 {
        clock.advance(Duration::from_hours(11));
        assert_eq!(code_of(&auth, &session).await, "ok");
    }
    clock.advance(Duration::from_hours(12));
    assert_eq!(code_of(&auth, &session).await, "session-expired");
    assert_eq!(code_of(&auth, &session).await, "session-invalid");
}

#[tokio::test]
async fn a_busy_session_lasts_until_the_seventh_day_and_not_beyond() {
    let (auth, clock) = service().await;
    let session = open(&auth).await;
    for _ in 0..(7 * 24 - 1) {
        clock.advance(Duration::from_hours(1));
        assert_eq!(code_of(&auth, &session).await, "ok");
    }
    clock.advance(Duration::from_hours(1));
    assert_eq!(code_of(&auth, &session).await, "session-expired");
    assert_eq!(code_of(&auth, &session).await, "session-invalid");
}

#[tokio::test]
async fn sessions_can_be_ended() {
    let (auth, _) = service().await;
    let session = open(&auth).await;
    auth.end_session(&session.session).await.unwrap();
    assert_eq!(code_of(&auth, &session).await, "session-invalid");
}

#[tokio::test]
async fn expired_and_idle_sessions_are_pruned() {
    let (auth, clock) = service().await;
    let idle = open(&auth).await;
    clock.advance(Duration::from_hours(13));
    let busy = open(&auth).await;
    assert_eq!(auth.prune_expired_sessions().await.unwrap(), 1);
    assert_eq!(code_of(&auth, &idle).await, "session-invalid");
    assert_eq!(code_of(&auth, &busy).await, "ok");
    clock.advance(Duration::from_hours(7 * 24));
    assert_eq!(auth.prune_expired_sessions().await.unwrap(), 1);
}

#[tokio::test]
async fn assets_need_only_a_live_cookie_and_do_not_keep_it_alive() {
    let (auth, clock) = service().await;
    let session = open(&auth).await;
    clock.advance(Duration::from_hours(11));
    let principal = auth
        .authenticate_asset_session(&session.session)
        .await
        .unwrap();
    assert!(matches!(
        principal,
        Principal::Session {
            elevated_until: None,
            ..
        }
    ));
    clock.advance(Duration::from_hours(2));
    assert_eq!(
        auth.authenticate_asset_session(&session.session)
            .await
            .unwrap_err()
            .code(),
        "session-expired"
    );
}

#[tokio::test]
async fn a_step_up_code_confirms_one_session_for_ten_minutes_once() {
    let (auth, clock) = service().await;
    let session = open(&auth).await;
    let other = open(&auth).await;
    let code = auth.create_step_up_code().unwrap();
    let Principal::Session { id, .. } = auth
        .authenticate_session(&session.session, Some(&session.key))
        .await
        .unwrap()
    else {
        unreachable!()
    };
    let until = auth.redeem_step_up(id, &code).await.unwrap();
    assert_eq!(until.as_millis(), 600_000);
    assert_eq!(elevated_until(&auth, &session).await, Some(until));
    assert_eq!(elevated_until(&auth, &other).await, None);
    let again = auth.redeem_step_up(id, &code).await.unwrap_err();
    assert_eq!(again.code(), "step-up-code-invalid");
    clock.advance(Duration::from_mins(9));
    assert_eq!(elevated_until(&auth, &session).await, Some(until));
    clock.advance(Duration::from_secs(61));
    assert_eq!(elevated_until(&auth, &session).await, None);
}

#[tokio::test]
async fn step_up_codes_expire_and_a_handoff_code_is_not_one() {
    let (auth, clock) = service().await;
    let session = open(&auth).await;
    let Principal::Session { id, .. } = auth
        .authenticate_session(&session.session, Some(&session.key))
        .await
        .unwrap()
    else {
        unreachable!()
    };
    let handoff = auth.create_handoff().unwrap();
    assert_eq!(
        auth.redeem_step_up(id, &handoff).await.unwrap_err().code(),
        "step-up-code-invalid"
    );
    let code = auth.create_step_up_code().unwrap();
    clock.advance(Duration::from_secs(121));
    assert_eq!(
        auth.redeem_step_up(id, &code).await.unwrap_err().code(),
        "step-up-code-invalid"
    );
}

#[tokio::test]
async fn pending_codes_are_bounded() {
    let (auth, clock) = service().await;
    let first = auth.create_handoff().unwrap();
    for _ in 0..70 {
        clock.advance(Duration::from_millis(10));
        auth.create_handoff().unwrap();
    }
    assert_eq!(
        auth.redeem_handoff(&first).await.unwrap_err().code(),
        "handoff-invalid"
    );
}

#[tokio::test]
async fn command_fields_follow_the_switch_and_the_caller() {
    let (auth, _) = service().await;
    let writers = [
        CommandWriter::Person { elevated: false },
        CommandWriter::Person { elevated: true },
        CommandWriter::Other,
    ];
    let mut found = Vec::new();
    assert!(auth.step_up_required().await.unwrap());
    for writer in writers {
        found.push(auth.command_refusal(writer).await.unwrap());
    }
    assert_eq!(
        found,
        [
            Some(CommandRefusal::NeedsStepUp),
            None,
            Some(CommandRefusal::NeedsPerson)
        ]
    );
    assert!(auth.set_step_up_required(false).await.unwrap().is_some());
    assert!(auth.set_step_up_required(false).await.unwrap().is_none());
    let mut found = Vec::new();
    for writer in writers {
        found.push(auth.command_refusal(writer).await.unwrap());
    }
    assert_eq!(found, [None, None, Some(CommandRefusal::NeedsPerson)]);
    assert!(auth.set_step_up_required(true).await.unwrap().is_some());
    assert!(auth.step_up_required().await.unwrap());
}
