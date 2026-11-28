#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::auth::{
    ApiToken, AuthRepository, AuthService, CommandRefusal, CommandWriter, Principal, Secret,
    SecretHash, Session, SessionSecrets, StepUpChange,
};
use nullradix_app::{Clock, ErrorKind, UuidV7Generator};
use nullradix_domain::Id;
use nullradix_domain::Timestamp;
use nullradix_store::Store;
use tokio::sync::Notify;

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
    let code = auth.create_turn_off_code().unwrap();
    let person = CommandWriter::Person { elevated: true };
    let off = auth.set_step_up(false, person, Some(code.expose())).await;
    assert!(off.unwrap().is_some());
    assert!(
        auth.set_step_up(false, person, None)
            .await
            .unwrap()
            .is_none()
    );
    let mut found = Vec::new();
    for writer in writers {
        found.push(auth.command_refusal(writer).await.unwrap());
    }
    assert_eq!(found, [None, None, Some(CommandRefusal::NeedsPerson)]);
    assert!(
        auth.set_step_up(true, CommandWriter::Other, None)
            .await
            .unwrap()
            .is_some()
    );
    assert!(auth.step_up_required().await.unwrap());
}

const PERSON: CommandWriter = CommandWriter::Person { elevated: true };

#[tokio::test]
async fn the_store_decides_inside_its_write_whether_the_switch_may_go_off() {
    let store = Store::in_memory().await.unwrap();
    let at = Timestamp::from_millis(5);
    let refused = store.set_step_up_required(false, false, at).await.unwrap();
    assert_eq!(refused, StepUpChange::TurnOffRefused);
    assert!(store.step_up_required().await.unwrap());
    let off = store.set_step_up_required(false, true, at).await.unwrap();
    assert!(matches!(off, StepUpChange::Changed(_)));
    let again = store.set_step_up_required(false, false, at).await.unwrap();
    assert_eq!(again, StepUpChange::Unchanged);
    let on = store.set_step_up_required(true, false, at).await.unwrap();
    assert!(matches!(on, StepUpChange::Changed(_)));
}

#[tokio::test]
async fn a_turn_off_code_is_spent_only_when_the_switch_goes_off() {
    let (auth, _) = service().await;
    let first = auth.create_turn_off_code().unwrap();
    let second = auth.create_turn_off_code().unwrap();
    // a token is refused before its code is looked at
    let refused = auth
        .set_step_up(false, CommandWriter::Other, Some(first.expose()))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "command-change-needs-person");
    assert!(
        auth.set_step_up(false, PERSON, Some(first.expose()))
            .await
            .unwrap()
            .is_some()
    );
    // already off: the code stays valid
    assert!(
        auth.set_step_up(false, PERSON, Some(second.expose()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        auth.set_step_up(true, PERSON, None)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        auth.set_step_up(false, PERSON, Some(second.expose()))
            .await
            .unwrap()
            .is_some()
    );
    // used up
    assert!(
        auth.set_step_up(true, PERSON, None)
            .await
            .unwrap()
            .is_some()
    );
    let spent = auth
        .set_step_up(false, PERSON, Some(second.expose()))
        .await
        .unwrap_err();
    assert_eq!(spent.code(), "step-up-code-invalid");
    let missing = auth.set_step_up(false, PERSON, None).await.unwrap_err();
    assert_eq!(missing.code(), "step-up-code-required");
    let malformed = auth
        .set_step_up(false, PERSON, Some("nope"))
        .await
        .unwrap_err();
    assert_eq!(malformed.code(), "step-up-code-invalid");
    assert!(auth.step_up_required().await.unwrap());
}

#[tokio::test]
async fn turn_off_codes_and_confirmation_codes_do_not_fit_each_other() {
    let (auth, _) = service().await;
    let session = open(&auth).await;
    let Principal::Session { id, .. } = auth
        .authenticate_session(&session.session, Some(&session.key))
        .await
        .unwrap()
    else {
        unreachable!()
    };
    let confirmation = auth.create_step_up_code().unwrap();
    let turn_off = auth.create_turn_off_code().unwrap();
    // each code is refused in the other place and is not used up there
    let wrong = auth
        .set_step_up(false, PERSON, Some(confirmation.expose()))
        .await
        .unwrap_err();
    assert_eq!(wrong.code(), "step-up-code-invalid");
    let wrong = auth.redeem_step_up(id, &turn_off).await.unwrap_err();
    assert_eq!(wrong.code(), "step-up-code-invalid");
    assert!(auth.redeem_step_up(id, &confirmation).await.is_ok());
    assert!(
        auth.set_step_up(false, PERSON, Some(turn_off.expose()))
            .await
            .unwrap()
            .is_some()
    );
}

/// A store whose switch write waits once at a gate, like a slow request.
struct Gated {
    store: Arc<Store>,
    reached: Notify,
    open: Notify,
    armed: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl AuthRepository for Gated {
    async fn token_by_hash(&self, hash: &SecretHash) -> Result<Option<ApiToken>, AppError> {
        self.store.token_by_hash(hash).await
    }
    async fn replace_tokens(&self, token: &ApiToken) -> Result<(), AppError> {
        self.store.replace_tokens(token).await
    }
    async fn session_by_hash(&self, hash: &SecretHash) -> Result<Option<Session>, AppError> {
        self.store.session_by_hash(hash).await
    }
    async fn insert_session(&self, session: &Session) -> Result<(), AppError> {
        self.store.insert_session(session).await
    }
    async fn delete_session(&self, id: Id<Session>) -> Result<(), AppError> {
        self.store.delete_session(id).await
    }
    async fn delete_expired_sessions(
        &self,
        now: Timestamp,
        idle_before: Timestamp,
    ) -> Result<u64, AppError> {
        self.store.delete_expired_sessions(now, idle_before).await
    }
    async fn touch_session(&self, id: Id<Session>, at: Timestamp) -> Result<(), AppError> {
        self.store.touch_session(id, at).await
    }
    async fn elevate_session(&self, id: Id<Session>, until: Timestamp) -> Result<bool, AppError> {
        self.store.elevate_session(id, until).await
    }
    async fn step_up_required(&self) -> Result<bool, AppError> {
        self.store.step_up_required().await
    }
    async fn set_step_up_required(
        &self,
        required: bool,
        turn_off_allowed: bool,
        at: Timestamp,
    ) -> Result<StepUpChange, AppError> {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.reached.notify_one();
            self.open.notified().await;
        }
        self.store
            .set_step_up_required(required, turn_off_allowed, at)
            .await
    }
}

#[tokio::test]
async fn a_token_cannot_turn_the_check_off_by_racing_the_person_turning_it_on() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let gated = Arc::new(Gated {
        store: store.clone(),
        reached: Notify::new(),
        open: Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(false),
    });
    let clock = Arc::new(TestClock::default());
    let auth = Arc::new(AuthService::new(
        gated.clone(),
        clock,
        Arc::new(UuidV7Generator),
    ));
    let code = auth.create_turn_off_code().unwrap();
    auth.set_step_up(false, PERSON, Some(code.expose()))
        .await
        .unwrap();
    assert!(!auth.step_up_required().await.unwrap());

    // the token saw the switch off and is held right before its write
    gated.armed.store(true, Ordering::SeqCst);
    let token = tokio::spawn({
        let auth = auth.clone();
        async move { auth.set_step_up(false, CommandWriter::Other, None).await }
    });
    gated.reached.notified().await;
    auth.set_step_up(true, PERSON, None).await.unwrap();
    gated.open.notify_one();

    let outcome = token.await.unwrap().unwrap_err();
    assert_eq!(outcome.code(), "command-change-needs-person");
    assert!(auth.step_up_required().await.unwrap());
}
