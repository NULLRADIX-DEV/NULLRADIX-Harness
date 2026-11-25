#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;

use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::projects::ProjectService;
use nullradix_app::repositories::CheckoutRepository;
use nullradix_app::tickets::{NewTicket, TicketPatch, TicketService, WriteOptions};
use nullradix_app::worktrees::{WorktreeEntry, WorktreeRepository};
use nullradix_app::{
    AppError, Change, ChangeFeed, ChangeOp, ChangeSignal, Clock, Entity, IdGenerator, SystemClock,
    UuidV7Generator,
};
use nullradix_domain::{
    Actor, Board, Checkout, CheckoutPath, Id, Repository, RepositoryName, Runner, Ticket,
    TicketRecord, Version, Worktree, WorktreeBranch, WorktreeDraft, WorktreePath, WorktreePurpose,
};
use nullradix_store::Store;

const AGENT: WriteOptions = WriteOptions {
    by_person: false,
    force: false,
    allow_duplicate: false,
};

struct Fixture {
    tickets: TicketService,
    person: Actor,
    store: Arc<Store>,
    board: Board,
    repository: Id<Repository>,
    runner: Id<Runner>,
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let person = local_person(store.as_ref(), "anna", &UuidV7Generator, &SystemClock)
        .await
        .unwrap();
    let clock = Arc::new(SystemClock);
    let ids = Arc::new(UuidV7Generator);
    let runners = common::runners(&store, &signal).await;
    let projects = ProjectService::new(store.clone(), clock.clone(), ids.clone(), signal.clone());
    let boards = BoardService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        runners.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    );
    let project = projects.create("ARC", "Arches").await.unwrap();
    let board = boards
        .create(project.id(), "Main", &person)
        .await
        .unwrap()
        .board;
    let now = clock.now();
    let repository = Repository::new(
        Id::from_uuid(ids.next_uuid()),
        RepositoryName::parse("arches").unwrap(),
        now,
    );
    let runner = runners.local().id();
    let path = CheckoutPath::parse("/work/arches").unwrap();
    let checkout = Checkout::new(
        Id::from_uuid(ids.next_uuid()),
        repository.id(),
        runner,
        path.clone(),
        path,
        now,
    );
    CheckoutRepository::insert(store.as_ref(), &repository, &checkout)
        .await
        .unwrap();
    Fixture {
        tickets: TicketService::new(store.clone(), store.clone(), clock, ids, signal),
        person,
        store,
        board,
        repository: repository.id(),
        runner,
    }
}

impl Fixture {
    async fn ticket(&self, title: &str) -> Ticket {
        let input = NewTicket {
            title: title.to_owned(),
            ..NewTicket::default()
        };
        self.tickets
            .create(
                self.board.project(),
                self.board.id(),
                &input,
                &self.person,
                AGENT,
            )
            .await
            .unwrap()
            .ticket
    }

    fn worktree(&self, branch: &str) -> Worktree {
        let draft = WorktreeDraft {
            repository: self.repository,
            runner: self.runner,
            branch: WorktreeBranch::parse(branch).unwrap(),
            target_branch: None,
            path: WorktreePath::parse(&format!("/work/arches-worktrees/{branch}")).unwrap(),
            purpose: WorktreePurpose::Work,
        };
        Worktree::new(
            Id::from_uuid(uuid::Uuid::now_v7()),
            draft,
            SystemClock.now(),
        )
    }

    async fn record(&self, key: &str) -> TicketRecord {
        self.tickets
            .get(key)
            .await
            .unwrap()
            .view
            .ticket
            .record()
            .clone()
    }

    async fn update(&self, key: &str, patch: TicketPatch) -> Result<TicketRecord, String> {
        let version = self.record(key).await.version;
        self.tickets
            .update(key, &patch, &self.person, version, AGENT)
            .await
            .map(|detail| detail.view.ticket.record().clone())
            .map_err(|error| error.code().to_owned())
    }

    async fn put(&self, worktree: &Worktree, owner: Id<Ticket>) -> Result<Change, AppError> {
        WorktreeRepository::insert(self.store.as_ref(), worktree, owner).await
    }

    async fn entry(&self, worktree: &Worktree) -> WorktreeEntry {
        self.store.get(worktree.id()).await.unwrap().unwrap()
    }
}

/// Keys and whether each link is still live, in list order.
fn links(entry: &WorktreeEntry) -> Vec<(String, u32, bool)> {
    entry
        .tickets
        .iter()
        .map(|link| (link.key.to_string(), link.position, link.live()))
        .collect()
}

fn clear() -> TicketPatch {
    TicketPatch {
        clear_link: true,
        ..TicketPatch::default()
    }
}

fn target(name: &str) -> TicketPatch {
    TicketPatch {
        target_branch: Some(Some(name.to_owned())),
        ..TicketPatch::default()
    }
}

#[tokio::test]
async fn linked_tickets_show_the_branch_and_folder_of_their_worktree() {
    let fx = fixture().await;
    let owner = fx.ticket("Owner").await;
    let guest = fx.ticket("Guest").await;
    fx.ticket("Alone").await;
    let worktree = fx.worktree("ticket/ARC-0001-owner");
    fx.put(&worktree, owner.id()).await.unwrap();
    fx.store
        .join(worktree.id(), guest.id(), SystemClock.now())
        .await
        .unwrap();

    let record = fx.record("ARC-1").await;
    assert_eq!(record.branch.as_deref(), Some("ticket/ARC-0001-owner"));
    assert_eq!(
        record.worktree.as_deref(),
        Some("/work/arches-worktrees/ticket/ARC-0001-owner")
    );
    assert_eq!(record.version, Version::INITIAL);
    let listed: Vec<(String, Option<String>)> = fx
        .tickets
        .list(&nullradix_app::tickets::TicketFilter::default())
        .await
        .unwrap()
        .into_iter()
        .map(|view| {
            let record = view.ticket.record();
            (record.key.to_string(), record.branch.clone())
        })
        .collect();
    let branch = Some(String::from("ticket/ARC-0001-owner"));
    assert_eq!(
        listed,
        [
            (String::from("ARC-0001"), branch.clone()),
            (String::from("ARC-0002"), branch),
            (String::from("ARC-0003"), None),
        ]
    );
    let entry = fx.store.of_ticket(guest.id()).await.unwrap().unwrap();
    assert_eq!(entry.worktree, worktree);
    assert_eq!(entry.owner().map(|link| link.ticket), Some(owner.id()));
    assert_eq!(
        links(&entry),
        [
            (String::from("ARC-0001"), 0, true),
            (String::from("ARC-0002"), 1, true)
        ]
    );
    let ops: Vec<ChangeOp> = fx
        .store
        .changes_since(0, 100)
        .await
        .unwrap()
        .into_iter()
        .filter(|change| change.entity == Entity::Worktree)
        .map(|change| change.op)
        .collect();
    assert_eq!(ops, [ChangeOp::Insert, ChangeOp::Update]);
}

#[tokio::test]
async fn clearing_ends_the_link_with_history_and_keeps_the_list() {
    let fx = fixture().await;
    let owner = fx.ticket("Owner").await;
    let worktree = fx.worktree("ticket/ARC-0001-owner");
    fx.put(&worktree, owner.id()).await.unwrap();
    let head = fx.store.head().await.unwrap();

    let cleared = fx.update("ARC-1", clear()).await.unwrap();
    assert_eq!((cleared.branch, cleared.worktree), (None, None));
    assert_eq!(cleared.version, Version::INITIAL.next());
    let history = fx.tickets.get("ARC-1").await.unwrap().history;
    let lines: Vec<(&str, Option<&str>, Option<&str>)> = history
        .iter()
        .skip(1)
        .map(|line| {
            (
                line.field.as_str(),
                line.from.as_deref(),
                line.to.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        lines,
        [
            ("branch", Some("ticket/ARC-0001-owner"), None),
            (
                "worktree",
                Some("/work/arches-worktrees/ticket/ARC-0001-owner"),
                None
            ),
        ]
    );
    let changed: Vec<Entity> = fx
        .store
        .changes_since(head, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|change| change.entity)
        .collect();
    assert_eq!(changed, [Entity::Worktree, Entity::Ticket]);

    // the registry entry and its list stay; only the link ended
    let entry = fx.entry(&worktree).await;
    assert_eq!(entry.worktree, worktree);
    assert_eq!(links(&entry), [(String::from("ARC-0001"), 0, false)]);
    assert!(fx.store.of_ticket(owner.id()).await.unwrap().is_none());

    // clearing again finds nothing to end but still bumps the version, as in legacy
    let again = fx.update("ARC-1", clear()).await.unwrap();
    assert_eq!(again.version, Version::INITIAL.next().next());
    assert_eq!(fx.tickets.get("ARC-1").await.unwrap().history.len(), 3);
}

#[tokio::test]
async fn the_target_branch_locks_while_the_ticket_is_linked() {
    let fx = fixture().await;
    let owner = fx.ticket("Owner").await;
    let worktree = fx.worktree("ticket/ARC-0001-owner");
    assert_eq!(
        fx.update("ARC-1", target("release/1"))
            .await
            .unwrap()
            .target_branch
            .map(|name| name.as_str().to_owned())
            .as_deref(),
        Some("release/1")
    );
    fx.put(&worktree, owner.id()).await.unwrap();

    assert_eq!(
        fx.update("ARC-1", target("release/2")).await.unwrap_err(),
        "target-branch-locked"
    );
    let both = TicketPatch {
        clear_link: true,
        ..target("release/2")
    };
    assert_eq!(
        fx.update("ARC-1", both).await.unwrap_err(),
        "target-branch-locked"
    );
    // the same target is no change and passes
    assert!(fx.update("ARC-1", target("release/1")).await.is_ok());
    assert!(fx.store.of_ticket(owner.id()).await.unwrap().is_some());

    fx.update("ARC-1", clear()).await.unwrap();
    let moved = fx.update("ARC-1", target("release/2")).await.unwrap();
    assert_eq!(
        moved.target_branch.map(|name| name.as_str().to_owned()),
        Some(String::from("release/2"))
    );
}

#[tokio::test]
async fn deleting_a_ticket_drops_its_links() {
    let fx = fixture().await;
    let owner = fx.ticket("Owner").await;
    let guest = fx.ticket("Guest").await;
    let worktree = fx.worktree("ticket/ARC-0001-owner");
    fx.put(&worktree, owner.id()).await.unwrap();
    fx.store
        .join(worktree.id(), guest.id(), SystemClock.now())
        .await
        .unwrap();

    fx.tickets.delete("ARC-2", &fx.person, None).await.unwrap();
    assert_eq!(
        links(&fx.entry(&worktree).await),
        [(String::from("ARC-0001"), 0, true)]
    );
    assert!(fx.store.of_ticket(guest.id()).await.unwrap().is_none());
}

#[tokio::test]
async fn a_ticket_has_at_most_one_live_link() {
    let fx = fixture().await;
    let first = fx.ticket("First").await;
    let second = fx.ticket("Second").await;
    let one = fx.worktree("ticket/ARC-0001-first");
    let two = fx.worktree("ticket/ARC-0002-second");
    fx.put(&one, first.id()).await.unwrap();

    let taken = fx.put(&two, first.id()).await.unwrap_err();
    assert_eq!(taken.code(), "already-started");
    assert!(fx.store.get(two.id()).await.unwrap().is_none());

    fx.put(&two, second.id()).await.unwrap();
    let now = SystemClock.now();
    let joined = fx.store.join(two.id(), first.id(), now).await.unwrap_err();
    assert_eq!(joined.code(), "already-started");

    // once released the ticket may join elsewhere and later come back to its old place
    assert!(fx.store.release(first.id(), now).await.unwrap().is_some());
    assert!(fx.store.release(first.id(), now).await.unwrap().is_none());
    fx.store.join(two.id(), first.id(), now).await.unwrap();
    assert_eq!(
        links(&fx.entry(&two).await),
        [
            (String::from("ARC-0002"), 0, true),
            (String::from("ARC-0001"), 1, true)
        ]
    );
    fx.store.release(first.id(), now).await.unwrap();
    fx.store.join(one.id(), first.id(), now).await.unwrap();
    assert_eq!(
        links(&fx.entry(&one).await),
        [(String::from("ARC-0001"), 0, true)]
    );
    assert_eq!(
        fx.record("ARC-1").await.branch.as_deref(),
        Some("ticket/ARC-0001-first")
    );
}

#[tokio::test]
async fn live_branches_are_unique_per_repository_and_runner() {
    let fx = fixture().await;
    let first = fx.ticket("First").await;
    let second = fx.ticket("Second").await;
    fx.put(&fx.worktree("ticket/x"), first.id()).await.unwrap();
    let twin = fx
        .put(&fx.worktree("ticket/x"), second.id())
        .await
        .unwrap_err();
    assert_eq!(twin.code(), "branch-exists");
    let ghost = fx.worktree("ticket/ghost");
    let missing = fx
        .store
        .join(ghost.id(), second.id(), SystemClock.now())
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "worktree-not-found");
}
