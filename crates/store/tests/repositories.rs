#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;

use nullradix_app::boards::{BoardService, BoardSummary, local_person};
use nullradix_app::projects::ProjectService;
use nullradix_app::repositories::{
    CheckoutRepository, RepositorySummary, RunnerRepository, RunnerService,
};
use nullradix_app::{
    ChangeFeed, ChangeOp, ChangeSignal, Entity, ErrorKind, SystemClock, UuidV7Generator,
};
use nullradix_domain::{
    Actor, Checkout, CheckoutPath, Id, Repository, RepositoryName, Runner, RunnerName, RunnerOs,
    Timestamp, Version,
};
use nullradix_store::Store;

struct Fixture {
    boards: BoardService,
    projects: ProjectService,
    runners: Arc<RunnerService>,
    person: Actor,
    store: Arc<Store>,
}

impl Fixture {
    fn local(&self) -> Id<Runner> {
        self.runners.local().id()
    }

    async fn board(&self, key: &str, name: &str) -> BoardSummary {
        let project = match self.projects.create(key, key).await {
            Ok(project) => project.id(),
            Err(_) => self
                .projects
                .list()
                .await
                .unwrap()
                .into_iter()
                .find(|project| project.key().as_str() == key)
                .unwrap()
                .id(),
        };
        self.boards
            .create(project, name, &self.person)
            .await
            .unwrap()
    }

    async fn ops(&self, entity: Entity) -> Vec<ChangeOp> {
        self.store
            .changes_since(0, 100)
            .await
            .unwrap()
            .into_iter()
            .filter(|change| change.entity == entity)
            .map(|change| change.op)
            .collect()
    }
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let person = local_person(store.as_ref(), "anna", &UuidV7Generator, &SystemClock)
        .await
        .unwrap();
    let runners = common::runners(&store, &signal).await;
    Fixture {
        boards: BoardService::new(
            store.clone(),
            store.clone(),
            store.clone(),
            runners.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal.clone(),
        ),
        projects: ProjectService::new(
            store.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            signal,
        ),
        runners,
        person,
        store,
    }
}

#[tokio::test]
async fn the_local_runner_is_recorded_once() {
    let fx = fixture().await;
    assert_eq!(fx.runners.local().name().as_str(), "local");
    assert_eq!(fx.runners.local().os(), RunnerOs::current());
    let again = common::runners(&fx.store, &ChangeSignal::new()).await;
    assert_eq!(again.local(), fx.runners.local());
    let other = Runner::new(
        Id::from_uuid(uuid::Uuid::now_v7()),
        RunnerName::parse("build").unwrap(),
        RunnerOs::Linux,
        Timestamp::from_millis(1),
    );
    let (stored, change) = fx.store.ensure(&other).await.unwrap();
    assert_eq!(stored, other);
    assert!(change.is_some());
    let names: Vec<String> = fx
        .runners
        .list()
        .await
        .unwrap()
        .iter()
        .map(|runner| runner.name().as_str().to_owned())
        .collect();
    assert_eq!(names, ["build", "local"]);
    assert_eq!(
        fx.ops(Entity::Runner).await,
        [ChangeOp::Insert, ChangeOp::Insert]
    );
}

#[tokio::test]
async fn boards_point_to_one_repository_per_checkout() {
    let fx = fixture().await;
    let main = fx.board("ARC", "Main").await;
    let side = fx.board("ARC", "Side").await;
    let local = fx.local();

    let bound = fx
        .boards
        .set_repository(
            main.board.project(),
            main.board.id(),
            local,
            "/work/Arches/src",
            Version::INITIAL,
        )
        .await
        .unwrap();
    assert_eq!(bound.repository_name.as_deref(), Some("Arches"));
    assert_eq!(bound.board.version(), Version::INITIAL.next());
    let repository = bound.board.repository().unwrap();

    // another folder of the same checkout, spelled differently, reuses the repository
    let shared = fx
        .boards
        .set_repository(
            side.board.project(),
            side.board.id(),
            local,
            "/work/ARCHES",
            Version::INITIAL,
        )
        .await
        .unwrap();
    assert_eq!(shared.board.repository(), Some(repository));

    let listed = fx.runners.repositories().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].repository.name().as_str(), "Arches");
    assert_eq!(listed[0].checkouts.len(), 1);
    assert_eq!(listed[0].checkouts[0].path().as_str(), "/work/Arches");
    assert_eq!(listed[0].checkouts[0].runner(), local);
    assert_eq!(fx.ops(Entity::Repository).await, [ChangeOp::Insert]);

    let found = fx
        .boards
        .list_by_checkout(local, "/work/arches/docs")
        .await
        .unwrap();
    let names: Vec<&str> = found
        .iter()
        .map(|summary| summary.board.name().as_str())
        .collect();
    assert_eq!(names, ["Main", "Side"]);
    assert_eq!(
        fx.boards
            .get(main.board.project(), main.board.id())
            .await
            .unwrap(),
        bound
    );
}

#[tokio::test]
async fn clearing_and_repeating_are_versioned_like_renames() {
    let fx = fixture().await;
    let main = fx.board("ARC", "Main").await;
    let (project, id, local) = (main.board.project(), main.board.id(), fx.local());
    let bound = fx
        .boards
        .set_repository(project, id, local, "/work/Arches", Version::INITIAL)
        .await
        .unwrap();
    let version = bound.board.version();

    let same = fx
        .boards
        .set_repository(project, id, local, "/work/Arches/x", version)
        .await
        .unwrap();
    assert_eq!(same, bound);

    let stale = fx
        .boards
        .clear_repository(project, id, Version::INITIAL)
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");

    let cleared = fx
        .boards
        .clear_repository(project, id, version)
        .await
        .unwrap();
    assert_eq!(cleared.board.repository(), None);
    assert_eq!(cleared.repository_name, None);
    assert_eq!(cleared.board.version(), version.next());
    let unchanged = fx
        .boards
        .clear_repository(project, id, cleared.board.version())
        .await
        .unwrap();
    assert_eq!(unchanged, cleared);
    assert_eq!(
        fx.ops(Entity::Board).await,
        [ChangeOp::Insert, ChangeOp::Update, ChangeOp::Update]
    );
    assert_eq!(
        fx.boards
            .list_by_checkout(local, "/work/Arches")
            .await
            .unwrap(),
        Vec::<BoardSummary>::new()
    );
}

#[tokio::test]
async fn refuses_unknown_runners_and_foreign_paths() {
    let fx = fixture().await;
    let main = fx.board("ARC", "Main").await;
    let (project, id, local) = (main.board.project(), main.board.id(), fx.local());
    let set = |runner, path: &'static str| {
        fx.boards
            .set_repository(project, id, runner, path, Version::INITIAL)
    };
    let unknown = Id::from_uuid(uuid::Uuid::from_u128(9));
    let error = set(unknown, "/work/Arches").await.unwrap_err();
    assert_eq!(
        (error.kind(), error.code()),
        (ErrorKind::NotFound, "runner-not-found")
    );
    assert_eq!(set(local, " ").await.unwrap_err().code(), "path-missing");
    assert_eq!(
        set(local, "work/Arches").await.unwrap_err().code(),
        "path-relative"
    );
    assert_eq!(
        set(local, "/elsewhere").await.unwrap_err().code(),
        "not-a-git-project"
    );
    assert_eq!(
        fx.boards
            .list_by_checkout(local, "/elsewhere")
            .await
            .unwrap(),
        Vec::<BoardSummary>::new()
    );
    assert_eq!(
        fx.boards
            .list_by_checkout(unknown, "/work/Arches")
            .await
            .unwrap_err()
            .code(),
        "runner-not-found"
    );
    assert_eq!(
        fx.runners.repositories().await.unwrap(),
        Vec::<RepositorySummary>::new()
    );
}

#[tokio::test]
async fn a_second_checkout_at_the_same_key_is_refused() {
    let fx = fixture().await;
    let now = Timestamp::from_millis(5);
    let path = CheckoutPath::parse("/work/arches").unwrap();
    let record = |name: &str| {
        let repository = Repository::new(
            Id::from_uuid(uuid::Uuid::now_v7()),
            RepositoryName::parse(name).unwrap(),
            now,
        );
        let checkout = Checkout::new(
            Id::from_uuid(uuid::Uuid::now_v7()),
            repository.id(),
            fx.local(),
            path.clone(),
            path.clone(),
            now,
        );
        (repository, checkout)
    };
    let (first, first_checkout) = record("first");
    let (second, second_checkout) = record("second");
    CheckoutRepository::insert(fx.store.as_ref(), &first, &first_checkout)
        .await
        .unwrap();
    let error = CheckoutRepository::insert(fx.store.as_ref(), &second, &second_checkout)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Conflict);
    assert_eq!(error.code(), "checkout-exists");
    let (found, _) = CheckoutRepository::find(fx.store.as_ref(), fx.local(), &path)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found, first);
    assert_eq!(fx.ops(Entity::Repository).await, [ChangeOp::Insert]);
}
