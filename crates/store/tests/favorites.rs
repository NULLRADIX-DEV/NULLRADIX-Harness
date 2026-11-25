#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use nullradix_app::favorites::FavoriteService;
use nullradix_app::hooks::HookService;
use nullradix_app::sessions::SessionService;
use nullradix_app::{ChangeFeed, ChangeSignal, Clock, Entity, UuidV7Generator};
use nullradix_domain::{
    DisplayState, HookDetails, HookEvent, HookKind, Id, RunnerOs, SessionRef, SessionState,
    Timestamp,
};
use nullradix_store::Store;

const START: i64 = 1_791_626_400_000;
const DAY: i64 = 24 * 60 * 60 * 1000;

#[derive(Default)]
struct ManualClock(AtomicI64);

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.0.load(Ordering::SeqCst))
    }
}

struct Fixture {
    store: Arc<Store>,
    clock: Arc<ManualClock>,
    hooks: HookService,
    favorites: Arc<FavoriteService>,
    serial: AtomicU64,
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let runners = common::runners(&store, &signal).await;
    let clock = Arc::new(ManualClock(AtomicI64::new(START)));
    let favorites = Arc::new(FavoriteService::new(
        store.clone(),
        runners.clone(),
        clock.clone(),
        Arc::new(UuidV7Generator),
        signal.clone(),
    ));
    let sessions = Arc::new(SessionService::new(
        store.clone(),
        runners,
        clock.clone(),
        Arc::new(UuidV7Generator),
        signal,
    ));
    let hooks = HookService::new(store.clone(), clock.clone()).with_sessions(sessions);
    Fixture {
        store,
        clock,
        hooks,
        favorites,
        serial: AtomicU64::new(0),
    }
}

impl Fixture {
    async fn hook(&self, kind: HookKind, session: &str, cwd: &str, details: HookDetails) {
        let serial = u128::from(self.serial.fetch_add(1, Ordering::SeqCst) + 1);
        let event = HookEvent::new(
            Id::from_uuid(uuid::Uuid::from_u128(serial)),
            kind,
            SessionRef::parse(session).unwrap(),
            cwd,
            self.clock.now(),
            details,
        )
        .unwrap();
        self.hooks.receive(&event).await.unwrap();
    }

    fn advance(&self, millis: i64) {
        self.clock.0.fetch_add(millis, Ordering::SeqCst);
    }

    async fn ids(&self) -> Vec<String> {
        self.favorites
            .list(None)
            .await
            .unwrap()
            .into_iter()
            .map(|line| line.favorite.record().session.as_str().to_owned())
            .collect()
    }
}

fn prompt(text: &str) -> HookDetails {
    HookDetails {
        prompt: Some(text.to_owned()),
        ..HookDetails::default()
    }
}

fn resume() -> HookDetails {
    HookDetails {
        source: Some(String::from("resume")),
        ..HookDetails::default()
    }
}

#[tokio::test]
async fn marking_keeps_a_snapshot_and_a_note_newest_first() {
    let f = fixture().await;
    f.hook(
        HookKind::SessionStart,
        "s1",
        "/x/one",
        HookDetails::default(),
    )
    .await;
    f.hook(
        HookKind::UserPrompt,
        "s1",
        "/x/one",
        prompt("Fix the login"),
    )
    .await;
    f.hook(
        HookKind::SessionStart,
        "s2",
        "/x/two",
        HookDetails::default(),
    )
    .await;
    let line = f.favorites.mark("s1", Some("Important")).await.unwrap();
    let record = line.favorite.record();
    assert_eq!(record.note, "Important");
    assert_eq!(record.title.as_deref(), Some("Fix the login"));
    assert_eq!(record.last_state, SessionState::Running);
    assert!(line.live.is_some());
    assert!(!line.cwd_exists, "the folder runner knows no folder");
    assert_eq!(line.favorite.command(), "cd '/x/one' && claude --resume s1");
    f.advance(1000);
    f.favorites.mark("s2", None).await.unwrap();
    assert_eq!(f.ids().await, ["s2", "s1"]);

    let again = f.favorites.mark("s1", None).await.unwrap();
    assert_eq!(again.favorite.record().note, "Important", "a note stays");
    let changed = f.favorites.mark("s1", Some("New")).await.unwrap();
    assert_eq!(changed.favorite.record().note, "New");
    let feed = f.store.changes_since(0, 1000).await.unwrap();
    assert!(feed.iter().any(|change| change.entity == Entity::Favorite));

    let error = f.favorites.mark("nope", None).await.unwrap_err();
    assert_eq!(error.code(), "session-not-found");
    let error = f.favorites.mark("", None).await.unwrap_err();
    assert_eq!(error.code(), "session-id-invalid");
}

#[tokio::test]
async fn the_snapshot_follows_hooks_and_outlives_the_session() {
    let f = fixture().await;
    f.hook(
        HookKind::SessionStart,
        "s1",
        "/x/one",
        HookDetails::default(),
    )
    .await;
    f.favorites.mark("s1", Some("Keep")).await.unwrap();
    f.hook(HookKind::UserPrompt, "s1", "/x/one", prompt("Late title"))
        .await;
    f.advance(1000);
    f.hook(HookKind::SessionEnd, "s1", "/x/one", HookDetails::default())
        .await;
    let line = f.favorites.list(None).await.unwrap().remove(0);
    assert_eq!(line.favorite.record().title.as_deref(), Some("Late title"));
    assert_eq!(line.favorite.record().last_state, SessionState::Ended);
    assert_eq!(line.live.unwrap().view.display_state, DisplayState::Ended);

    f.advance(8 * DAY);
    f.hooks
        .prune(std::time::Duration::from_secs(1))
        .await
        .unwrap();
    let line = f.favorites.list(None).await.unwrap().remove(0);
    assert_eq!(line.live, None, "the session is gone");
    assert_eq!(line.favorite.record().note, "Keep");
    assert_eq!(line.favorite.record().title.as_deref(), Some("Late title"));
    // a gone session only takes a note, an empty one when none is given
    let noted = f.favorites.mark("s1", Some("Later")).await.unwrap();
    assert_eq!(noted.favorite.record().note, "Later");
    assert_eq!(
        f.favorites
            .mark("s1", None)
            .await
            .unwrap()
            .favorite
            .record()
            .note,
        ""
    );
}

#[tokio::test]
async fn a_resumed_session_joins_the_only_favorite_in_its_folder() {
    let f = fixture().await;
    for (id, cwd) in [("a", "/x/one"), ("b", "/x/two"), ("c", "/x/two")] {
        f.hook(HookKind::SessionStart, id, cwd, HookDetails::default())
            .await;
        f.favorites.mark(id, None).await.unwrap();
    }
    f.advance(1000);
    f.hook(HookKind::SessionStart, "a2", "/x/one/", resume())
        .await;
    f.hook(HookKind::SessionStart, "a2", "/x/one", resume())
        .await;
    f.hook(HookKind::SessionStart, "b2", "/x/two", resume())
        .await;
    f.hook(
        HookKind::SessionStart,
        "fresh",
        "/x/one",
        HookDetails::default(),
    )
    .await;
    f.hook(HookKind::SessionStart, "a", "/x/one", resume())
        .await;
    let resumed = |id: &'static str| {
        let favorites = f.favorites.clone();
        async move {
            favorites
                .list(None)
                .await
                .unwrap()
                .into_iter()
                .find(|line| line.favorite.record().session.as_str() == id)
                .unwrap()
                .favorite
                .record()
                .resumed_as
                .iter()
                .map(|resumption| resumption.session.as_str().to_owned())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(resumed("a").await, ["a2"]);
    assert_eq!(resumed("b").await, Vec::<String>::new());
    assert_eq!(resumed("c").await, Vec::<String>::new());
}

#[tokio::test]
async fn unmarking_forgets_the_favorite_and_its_resumptions() {
    let f = fixture().await;
    f.hook(
        HookKind::SessionStart,
        "s1",
        "/x/one",
        HookDetails::default(),
    )
    .await;
    f.favorites.mark("s1", None).await.unwrap();
    f.hook(HookKind::SessionStart, "s2", "/x/one", resume())
        .await;
    f.favorites.unmark("s1").await.unwrap();
    assert_eq!(f.ids().await, Vec::<String>::new());
    f.favorites.unmark("s1").await.unwrap();
    let again = f.favorites.mark("s1", None).await.unwrap();
    assert_eq!(again.favorite.record().resumed_as, Vec::new());
    assert_eq!(again.favorite.record().note, "");
}

#[tokio::test]
async fn a_hook_writes_its_session_and_favorite_in_one_commit() {
    let f = fixture().await;
    f.hook(
        HookKind::SessionStart,
        "s1",
        "/x/one",
        HookDetails::default(),
    )
    .await;
    f.hook(
        HookKind::SessionStart,
        "plain",
        "/x/two",
        HookDetails::default(),
    )
    .await;
    f.favorites.mark("s1", None).await.unwrap();
    let before = f.store.changes_since(0, 1000).await.unwrap().len();
    f.advance(1000);
    f.hook(
        HookKind::UserPrompt,
        "plain",
        "/x/two",
        prompt("No favorite"),
    )
    .await;
    let feed = f.store.changes_since(0, 1000).await.unwrap();
    assert_eq!(
        feed.len(),
        before + 1,
        "a session without favorite adds no favorite change"
    );
    f.hook(HookKind::UserPrompt, "s1", "/x/one", prompt("Followed"))
        .await;
    let feed = f.store.changes_since(0, 1000).await.unwrap();
    let [.., session, favorite] = feed.as_slice() else {
        panic!("too few changes");
    };
    assert_eq!(session.entity, Entity::Session);
    assert_eq!(favorite.entity, Entity::Favorite);
    assert_eq!(
        favorite.seq,
        session.seq + 1,
        "one transaction, in commit order"
    );
    let line = f.favorites.list(None).await.unwrap().remove(0);
    assert_eq!(line.favorite.record().title.as_deref(), Some("Followed"));
}

#[tokio::test]
async fn hooks_keep_their_order_in_the_favorite() {
    let f = fixture().await;
    f.hook(
        HookKind::SessionStart,
        "s1",
        "/x/one",
        HookDetails::default(),
    )
    .await;
    f.favorites.mark("s1", None).await.unwrap();
    for round in 0..20 {
        f.advance(1000);
        tokio::join!(
            f.hook(HookKind::UserPrompt, "s1", "/x/one", prompt("First")),
            f.hook(HookKind::Stop, "s1", "/x/one", HookDetails::default()),
            f.hook(HookKind::UserPrompt, "s1", "/x/one", prompt("Second")),
        );
        let line = f.favorites.list(None).await.unwrap().remove(0);
        let stored = line.live.unwrap();
        let record = line.favorite.record();
        assert_eq!(
            record.last_state,
            stored.session.record().state,
            "round {round}"
        );
        assert_eq!(
            record.last_event_at,
            stored.session.record().last_event_at,
            "round {round}"
        );
        assert_eq!(record.title, stored.session.record().title, "round {round}");
    }
}

#[tokio::test]
async fn a_resumed_session_joins_its_favorite_whatever_the_case_of_the_folder() {
    let f = fixture().await;
    f.hook(
        HookKind::SessionStart,
        "a",
        "/Work/One",
        HookDetails::default(),
    )
    .await;
    f.favorites.mark("a", None).await.unwrap();
    f.hook(HookKind::SessionStart, "a2", "/work/one", resume())
        .await;
    let line = f.favorites.list(None).await.unwrap().remove(0);
    let resumed = line.favorite.record().resumed_as.len();
    assert_eq!(
        resumed,
        usize::from(RunnerOs::current().ignores_path_case())
    );
}
